//! Android phones over a USB cable.
//!
//! Android exposes its storage over MTP, which macOS does not speak — Finder
//! never shows the phone. `mtp-rs` talks MTP directly over USB in pure Rust,
//! so there is no libmtp or driver for the user to install.
//!
//! Everything here is reachable only from the local window (see
//! `commands.rs`); no network route touches a USB device. Paths on the Mac side
//! come from the user's own file pickers, and names coming *from* the phone are
//! sanitised before they become part of a local path.

use bytes::Bytes;
use mtp_rs::{
    ByteRange, DateTime, MtpDevice, NewObjectInfo, ObjectHandle, ObjectInfo, StorageId, UtcOffset,
};
use serde::Serialize;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Apple's USB vendor id. iPhones present as PTP cameras, which would match
/// the MTP scan, but they cannot be browsed this way — leave them out rather
/// than offer something that fails.
const APPLE_VENDOR_ID: u16 = 0x05ac;
const UPLOAD_CHUNK: usize = 256 * 1024;
/// How often a running transfer reports progress.
const REPORT_EVERY: Duration = Duration::from_millis(120);

// ----------------------------------------------------------------- views

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsbDevice {
    /// The USB location id, as a string: it is a u64 and JavaScript numbers
    /// are not.
    pub id: String,
    pub name: String,
    pub serial: Option<String>,
    /// "USB 2", "USB 3 (5 Gbps)" … — a slow cable is the usual cause of a slow
    /// copy, so it is worth showing.
    pub speed: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsbStorage {
    pub id: String,
    pub name: String,
    pub total: u64,
    pub free: u64,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsbConnection {
    pub id: String,
    pub name: String,
    pub storages: Vec<UsbStorage>,
    pub can_upload: bool,
    pub can_delete: bool,
    pub thumbnails: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsbEntry {
    pub handle: String,
    pub name: String,
    pub size: u64,
    pub folder: bool,
    pub modified_ms: Option<i64>,
    /// `folder` | `image` | `video` | `audio` | `file`
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsbError {
    /// Stable, for the UI to switch on: `exclusive`, `locked`, `disconnected`,
    /// `full`, `cancelled`, `gone`, `disk`, `invalid`, `other` …
    pub code: &'static str,
    pub message: String,
}

impl UsbError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<mtp_rs::Error> for UsbError {
    fn from(e: mtp_rs::Error) -> Self {
        use mtp_rs::Error as E;
        if e.is_exclusive_access() {
            return Self::new("exclusive", "Another app is using the phone.");
        }
        match e {
            E::PermissionDenied => Self::new("permission", "The system would not let ShareLocal open the phone."),
            E::Disconnected => Self::new("disconnected", "The phone was disconnected."),
            E::StorageFull => Self::new("full", "There is not enough free space on the phone."),
            E::Cancelled => Self::new("cancelled", "Cancelled."),
            E::Busy => Self::new("busy", "The phone is busy. Try again in a moment."),
            E::AccessDenied => Self::new(
                "denied",
                "The phone refused. Unlock it and check that “File transfer” is selected.",
            ),
            E::NotFound | E::StaleHandle => {
                Self::new("gone", "That item is no longer on the phone. Refresh and try again.")
            }
            E::Unsupported => Self::new("unsupported", "This phone does not support that."),
            other => Self::new("other", other.to_string()),
        }
    }
}

impl From<mtp_rs::UploadError> for UsbError {
    fn from(e: mtp_rs::UploadError) -> Self {
        e.source.into()
    }
}

fn disk_error(e: std::io::Error, path: &Path) -> UsbError {
    UsbError::new("disk", format!("Could not write {}: {e}", path.display()))
}

/// Progress of one download or upload, sent to the UI as `usb-progress`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsbProgress {
    pub job: String,
    /// `download` (phone → Mac) or `upload` (Mac → phone).
    pub direction: &'static str,
    /// The file being copied right now.
    pub current: String,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: f64,
    pub finished: bool,
    pub error: Option<UsbError>,
    /// Downloads: the top-level files and folders created on the Mac.
    pub saved: Vec<String>,
}

impl UsbProgress {
    fn new(job: &str, direction: &'static str) -> Self {
        Self { job: job.to_string(), direction, ..Default::default() }
    }

    fn fail(mut self, error: UsbError) -> Self {
        self.finished = true;
        self.error = Some(error);
        self
    }
}

// ------------------------------------------------------------- discovery

/// Phones plugged in right now. Cheap enough to poll every couple of seconds.
pub fn list_devices() -> Vec<UsbDevice> {
    MtpDevice::list_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.vendor_id != APPLE_VENDOR_ID)
        .map(|d| UsbDevice {
            id: d.location_id.to_string(),
            name: display_name(d.manufacturer.as_deref(), d.product.as_deref()),
            serial: d.serial_number.clone(),
            speed: d.speed.map(speed_label),
        })
        .collect()
}

fn display_name(manufacturer: Option<&str>, product: Option<&str>) -> String {
    let manufacturer = manufacturer.map(str::trim).filter(|s| !s.is_empty());
    let product = product.map(str::trim).filter(|s| !s.is_empty());
    match (manufacturer, product) {
        (Some(m), Some(p)) if p.to_lowercase().contains(&m.to_lowercase()) => p.to_string(),
        (Some(m), Some(p)) => format!("{m} {p}"),
        (None, Some(p)) => p.to_string(),
        (Some(m), None) => format!("{m} phone"),
        (None, None) => "Android phone".into(),
    }
}

fn speed_label(speed: mtp_rs::UsbSpeed) -> &'static str {
    use mtp_rs::UsbSpeed as S;
    match speed {
        S::Low | S::Full => "USB 1",
        S::High => "USB 2",
        S::Super => "USB 3 (5 Gbps)",
        S::SuperPlus => "USB 3 (10 Gbps)",
    }
}

/// Open a phone. With `release`, first stop the macOS services that grab
/// phones as soon as they are plugged in — see [`release_holders`].
pub async fn open(id: &str, release: bool) -> Result<MtpDevice, UsbError> {
    let location: u64 = id
        .parse()
        .map_err(|_| UsbError::new("invalid", "Unknown USB device."))?;
    let attempts = if release { 8 } else { 1 };
    let mut last = None;

    for attempt in 0..attempts {
        if release {
            release_holders().await;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        match MtpDevice::builder().open_by_location(location).await {
            Ok(device) => return Ok(device),
            Err(e) => {
                // The holder can be relaunched by launchd in the gap between
                // stopping it and claiming the phone, so keep trying briefly.
                let retry = e.is_exclusive_access() && attempt + 1 < attempts;
                last = Some(e);
                if !retry {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
        }
    }

    let mut error = UsbError::from(last.expect("at least one attempt"));
    if error.code == "exclusive" {
        if let Some(owner) = exclusive_owner().await {
            error.message = format!("{owner} is using the phone.");
        }
    }
    Err(error)
}

/// What the UI needs once a phone is open.
pub async fn describe(device: &MtpDevice, id: &str) -> Result<UsbConnection, UsbError> {
    let info = device.device_info();
    let caps = device.capabilities();
    let storages = storages(device).await?;
    if storages.is_empty() {
        // A locked Android phone answers MTP but lists no storage at all.
        return Err(UsbError::new(
            "locked",
            "Unlock the phone. If it asks to allow access to phone data, tap Allow.",
        ));
    }
    Ok(UsbConnection {
        id: id.to_string(),
        name: display_name(Some(&info.manufacturer), Some(&info.model)),
        storages,
        can_upload: caps.can_upload,
        can_delete: caps.can_delete,
        thumbnails: caps.supports_thumbnails,
    })
}

pub async fn storages(device: &MtpDevice) -> Result<Vec<UsbStorage>, UsbError> {
    Ok(device
        .storages()
        .await?
        .iter()
        .map(|s| {
            let info = s.info();
            UsbStorage {
                id: info.id.0.to_string(),
                name: if info.description.trim().is_empty() {
                    "Phone storage".into()
                } else {
                    info.description.clone()
                },
                total: info.total_capacity,
                free: info.free_space,
                writable: info.is_writable,
            }
        })
        .collect())
}

/// macOS starts `ptpcamerad` (Image Capture / Photos) the moment a phone is
/// plugged in, and it takes the USB interface for itself. Android File
/// Transfer, if installed, does the same. Both run as the user and are
/// relaunched on demand, so stopping them needs no admin rights and costs
/// nothing — this is what OpenMTP and MacDroid do too. Only ever called when
/// the user presses the button that says so.
pub async fn release_holders() {
    #[cfg(target_os = "macos")]
    {
        for args in [
            ["-x", "ptpcamerad"],
            ["-f", "Android File Transfer Agent"],
        ] {
            let _ = tokio::process::Command::new("pkill")
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }
    }
}

/// Name the app holding a phone, when it is one we recognise. IORegistry
/// records the claiming process as `UsbExclusiveOwner = "pid 607, ptpcamerad"`.
async fn exclusive_owner() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("ioreg")
            .args(["-l", "-w0"])
            .output()
            .await
            .ok()?;
        return owner_from_ioreg(&String::from_utf8_lossy(&out.stdout));
    }
    #[allow(unreachable_code)]
    None
}

pub fn owner_from_ioreg(text: &str) -> Option<String> {
    text.lines()
        .filter(|l| l.contains("UsbExclusiveOwner"))
        .find_map(|line| {
            let lower = line.to_lowercase();
            if lower.contains("ptpcamerad") {
                Some("macOS Image Capture".to_string())
            } else if lower.contains("android file transfer") {
                Some("Android File Transfer".to_string())
            } else if lower.contains("openmtp") {
                Some("OpenMTP".to_string())
            } else if lower.contains("macdroid") {
                Some("MacDroid".to_string())
            } else {
                None
            }
        })
}

// -------------------------------------------------------------- browsing

fn parse_id(value: &str, what: &str) -> Result<u64, UsbError> {
    value
        .parse()
        .map_err(|_| UsbError::new("invalid", format!("Invalid {what}.")))
}

async fn storage(device: &MtpDevice, id: &str) -> Result<mtp_rs::Storage, UsbError> {
    Ok(device.storage(StorageId(parse_id(id, "storage")?)).await?)
}

fn parent_handle(parent: Option<&str>) -> Result<Option<ObjectHandle>, UsbError> {
    parent
        .map(|p| parse_id(p, "folder").map(ObjectHandle))
        .transpose()
}

/// One folder's contents, folders first, then by name. Dot-files are left out
/// unless `show_hidden`.
pub async fn list(
    device: &MtpDevice,
    storage_id: &str,
    parent: Option<&str>,
    show_hidden: bool,
) -> Result<Vec<UsbEntry>, UsbError> {
    let storage = storage(device, storage_id).await?;
    let objects = storage.list_objects(parent_handle(parent)?).await?;
    let offset = local_offset();
    // Hidden like Finder hides them: `.thumbnails`, `.pmtemp`, and the
    // `.trashed-…` files Android keeps for 30 days after a photo is deleted.
    let mut entries: Vec<UsbEntry> = objects
        .iter()
        .filter(|o| show_hidden || !o.filename.starts_with('.'))
        .map(|o| entry(o, offset))
        .collect();
    entries.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn entry(object: &ObjectInfo, offset: UtcOffset) -> UsbEntry {
    UsbEntry {
        handle: object.handle.0.to_string(),
        name: object.filename.clone(),
        size: if object.is_folder() { 0 } else { object.size },
        folder: object.is_folder(),
        modified_ms: object
            .modified
            .and_then(|d| d.to_unix_seconds_with_fallback(offset))
            .map(|s| s * 1000),
        kind: kind_of(object),
    }
}

fn kind_of(object: &ObjectInfo) -> &'static str {
    if object.is_folder() {
        return "folder";
    }
    // Android reports most files as "undefined" format, so the extension is
    // the more reliable signal; the MTP format is the fallback.
    if let Some(mime) = mime_guess::from_path(&object.filename).first() {
        match mime.type_().as_str() {
            "image" => return "image",
            "video" => return "video",
            "audio" => return "audio",
            _ => {}
        }
    }
    if object.format.is_image() {
        "image"
    } else if object.format.is_video() {
        "video"
    } else if object.format.is_audio() {
        "audio"
    } else {
        "file"
    }
}

/// The phone's own thumbnail for a photo, as a data URL.
pub async fn thumbnail(device: &MtpDevice, storage_id: &str, handle: &str) -> Result<String, UsbError> {
    use base64::Engine;
    let storage = storage(device, storage_id).await?;
    let bytes = storage.thumbnail(ObjectHandle(parse_id(handle, "file")?)).await?;
    if bytes.is_empty() {
        return Err(UsbError::new("unsupported", "No thumbnail."));
    }
    let mime = if bytes.starts_with(b"\x89PNG") { "image/png" } else { "image/jpeg" };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

pub async fn create_folder(
    device: &MtpDevice,
    storage_id: &str,
    parent: Option<&str>,
    name: &str,
) -> Result<String, UsbError> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) || name.len() > 255 {
        return Err(UsbError::new("invalid", "That is not a valid folder name."));
    }
    let storage = storage(device, storage_id).await?;
    let handle = storage.create_folder(parent_handle(parent)?, name).await?;
    Ok(handle.0.to_string())
}

pub async fn delete(device: &MtpDevice, storage_id: &str, handles: &[String]) -> Result<(), UsbError> {
    let storage = storage(device, storage_id).await?;
    for handle in handles {
        storage.delete(ObjectHandle(parse_id(handle, "file")?)).await?;
    }
    Ok(())
}

// ------------------------------------------------------------- transfers

/// Throttles progress reports and keeps the speed figure.
struct Reporter<'a, F: FnMut(&UsbProgress)> {
    progress: UsbProgress,
    started: Instant,
    last: Instant,
    emit: &'a mut F,
}

impl<'a, F: FnMut(&UsbProgress)> Reporter<'a, F> {
    fn new(progress: UsbProgress, emit: &'a mut F) -> Self {
        let now = Instant::now();
        Self { progress, started: now, last: now - REPORT_EVERY, emit }
    }

    fn tick(&mut self, force: bool) {
        if !force && self.last.elapsed() < REPORT_EVERY {
            return;
        }
        let secs = self.started.elapsed().as_secs_f64();
        self.progress.speed_bps = if secs > 0.3 { self.progress.bytes_done as f64 / secs } else { 0.0 };
        self.last = Instant::now();
        (self.emit)(&self.progress);
    }

    fn finish(mut self, error: Option<UsbError>) -> UsbProgress {
        self.progress.finished = true;
        self.progress.error = error;
        self.tick(true);
        self.progress
    }
}

/// A file on the phone, and where it lands on the Mac.
struct Planned {
    handle: ObjectHandle,
    target: PathBuf,
    size: u64,
    modified: Option<DateTime>,
}

/// Copy files and folders from the phone into `dest`, keeping folder
/// structure. Never overwrites: a name already taken gets ` (2)`.
pub async fn download<F: FnMut(&UsbProgress) + Send>(
    device: &MtpDevice,
    storage_id: &str,
    handles: &[String],
    dest: &Path,
    job: &str,
    cancel: &AtomicBool,
    mut emit: F,
) -> UsbProgress {
    let progress = UsbProgress::new(job, "download");
    let storage = match storage(device, storage_id).await {
        Ok(s) => s,
        Err(e) => return progress.fail(e),
    };
    if let Err(e) = tokio::fs::create_dir_all(dest).await {
        return progress.fail(disk_error(e, dest));
    }

    let mut reporter = Reporter::new(progress, &mut emit);
    reporter.progress.current = "Looking through the selection…".into();
    reporter.tick(true);

    // Plan first, so the totals are known before any bytes move.
    let mut plan = Vec::new();
    for handle in handles {
        let handle = match parse_id(handle, "file") {
            Ok(h) => ObjectHandle(h),
            Err(e) => return reporter.finish(Some(e)),
        };
        let info = match storage.get_object_info(handle).await {
            Ok(i) => i,
            Err(e) => return reporter.finish(Some(e.into())),
        };
        let root = crate::transfer::paths::unique_path(&dest.join(safe_name(&info.filename)));
        reporter.progress.saved.push(root.to_string_lossy().to_string());
        if info.is_folder() {
            if let Err(e) = plan_folder(&storage, handle, &root, &mut plan, cancel).await {
                return reporter.finish(Some(e));
            }
        } else {
            plan.push(Planned { handle, target: root, size: info.size, modified: info.modified });
        }
    }
    reporter.progress.files_total = plan.len();
    reporter.progress.bytes_total = plan.iter().map(|p| p.size).sum();
    reporter.tick(true);

    let offset = local_offset();
    for item in plan {
        if cancel.load(Ordering::SeqCst) {
            return reporter.finish(Some(mtp_rs::Error::Cancelled.into()));
        }
        if let Err(e) = download_one(&storage, &item, offset, cancel, &mut reporter).await {
            return reporter.finish(Some(e));
        }
        reporter.progress.files_done += 1;
        reporter.tick(true);
    }
    reporter.finish(None)
}

/// Walk a folder on the phone, creating its local twin as we go.
async fn plan_folder(
    storage: &mtp_rs::Storage,
    folder: ObjectHandle,
    local: &Path,
    plan: &mut Vec<Planned>,
    cancel: &AtomicBool,
) -> Result<(), UsbError> {
    let mut stack = vec![(folder, local.to_path_buf())];
    while let Some((handle, dir)) = stack.pop() {
        if cancel.load(Ordering::SeqCst) {
            return Err(mtp_rs::Error::Cancelled.into());
        }
        tokio::fs::create_dir_all(&dir).await.map_err(|e| disk_error(e, &dir))?;
        for child in storage.list_objects(Some(handle)).await? {
            if is_android_scratch(&child.filename) {
                continue;
            }
            let path = dir.join(safe_name(&child.filename));
            if child.is_folder() {
                stack.push((child.handle, path));
            } else {
                plan.push(Planned {
                    handle: child.handle,
                    target: path,
                    size: child.size,
                    modified: child.modified,
                });
            }
        }
    }
    Ok(())
}

async fn download_one<F: FnMut(&UsbProgress) + Send>(
    storage: &mtp_rs::Storage,
    item: &Planned,
    offset: UtcOffset,
    cancel: &AtomicBool,
    reporter: &mut Reporter<'_, F>,
) -> Result<(), UsbError> {
    let name = item
        .target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    reporter.progress.current = name.clone();
    reporter.tick(true);

    // Bytes land in a hidden part file and are renamed into place at the end,
    // so a cancelled copy never leaves a truncated photo behind.
    let part = item.target.with_file_name(format!(".{name}.sharelocal-part"));
    let file = tokio::fs::File::create(&part).await.map_err(|e| disk_error(e, &part))?;
    let mut writer = tokio::io::BufWriter::with_capacity(1024 * 1024, file);
    let started_at = reporter.progress.bytes_done;

    let result: Result<(), UsbError> = async {
        let mut stream = storage.download(item.handle, ByteRange::Full).await?;
        while let Some(chunk) = stream.next_chunk().await {
            if cancel.load(Ordering::SeqCst) {
                // Tell the phone to stop sending, or the session stays wedged.
                let _ = stream.cancel(mtp_rs::DEFAULT_CANCEL_TIMEOUT).await;
                return Err(mtp_rs::Error::Cancelled.into());
            }
            let chunk = chunk?;
            writer.write_all(&chunk).await.map_err(|e| disk_error(e, &part))?;
            reporter.progress.bytes_done += chunk.len() as u64;
            reporter.tick(false);
        }
        writer.flush().await.map_err(|e| disk_error(e, &part))?;
        Ok(())
    }
    .await;

    drop(writer);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        reporter.progress.bytes_done = started_at;
        return Err(e);
    }

    let target = crate::transfer::paths::unique_path(&item.target);
    tokio::fs::rename(&part, &target).await.map_err(|e| disk_error(e, &target))?;
    // Keep the photo's own date, so Finder sorts it where it belongs.
    if let Some(secs) = item.modified.and_then(|d| d.to_unix_seconds_with_fallback(offset)) {
        if let Ok(secs) = u64::try_from(secs) {
            if let Ok(file) = std::fs::File::options().write(true).open(&target) {
                let _ = file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
            }
        }
    }
    Ok(())
}

/// Copy files and folders from the Mac into `parent` on the phone. Folders
/// that already exist are merged into; files whose name is taken get ` (2)`.
#[allow(clippy::too_many_arguments)]
pub async fn upload<F: FnMut(&UsbProgress) + Send>(
    device: &MtpDevice,
    storage_id: &str,
    parent: Option<&str>,
    inputs: &[PathBuf],
    job: &str,
    cancel: &AtomicBool,
    mut emit: F,
) -> UsbProgress {
    let progress = UsbProgress::new(job, "upload");
    let (storage, parent) = match (storage(device, storage_id).await, parent_handle(parent)) {
        (Ok(s), Ok(p)) => (s, p),
        (Err(e), _) | (_, Err(e)) => return progress.fail(e),
    };
    let (metas, sources) = match crate::transfer::collect_files(inputs) {
        Ok(found) => found,
        Err(e) => return progress.fail(UsbError::new("disk", e.message().to_string())),
    };

    let mut reporter = Reporter::new(progress, &mut emit);
    reporter.progress.files_total = metas.len();
    reporter.progress.bytes_total = metas.iter().map(|m| m.size).sum();
    let free = storage.info().free_space;
    if free > 0 && reporter.progress.bytes_total > free {
        return reporter.finish(Some(UsbError::new(
            "full",
            "There is not enough free space on the phone for all of that.",
        )));
    }
    reporter.tick(true);

    let mut tree = RemoteTree::new(parent);
    let offset = local_offset();

    for (meta, source) in metas.iter().zip(&sources) {
        if cancel.load(Ordering::SeqCst) {
            return reporter.finish(Some(mtp_rs::Error::Cancelled.into()));
        }
        let (dir, name) = match meta.rel_path.rsplit_once('/') {
            Some((dir, name)) => (dir, name),
            None => ("", meta.rel_path.as_str()),
        };
        let result = async {
            let folder = tree.folder(&storage, dir).await?;
            let name = tree.free_name(&storage, folder, name).await?;
            reporter.progress.current = name.clone();
            reporter.tick(true);
            let handle = upload_one(&storage, folder, &name, source, meta.size, offset, cancel, &mut reporter).await?;
            tree.claim(folder, name, handle, false);
            Ok::<(), UsbError>(())
        }
        .await;
        if let Err(e) = result {
            return reporter.finish(Some(e));
        }
        reporter.progress.files_done += 1;
        reporter.tick(true);
    }
    reporter.finish(None)
}

#[allow(clippy::too_many_arguments)]
async fn upload_one<F: FnMut(&UsbProgress) + Send>(
    storage: &mtp_rs::Storage,
    parent: Option<ObjectHandle>,
    name: &str,
    source: &Path,
    size: u64,
    offset: UtcOffset,
    cancel: &AtomicBool,
    reporter: &mut Reporter<'_, F>,
) -> Result<ObjectHandle, UsbError> {
    let file = tokio::fs::File::open(source)
        .await
        .map_err(|e| UsbError::new("disk", format!("Could not read {}: {e}", source.display())))?;
    let modified = std::fs::metadata(source)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| DateTime::from_unix_seconds(d.as_secs() as i64, offset));
    let mut info = NewObjectInfo::file(name, size);
    if let Some(modified) = modified {
        info = info.with_modified(modified);
    }

    // Stream from disk: a 4 GB video never sits in memory.
    let stream = futures_util::stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let mut buf = vec![0u8; UPLOAD_CHUNK];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), Some(file)))
            }
            Err(e) => Some((Err(e), None)),
        }
    });

    let base = reporter.progress.bytes_done;
    let result = storage
        .upload_with_progress(parent, info, Box::pin(stream), |p| {
            if cancel.load(Ordering::SeqCst) {
                return ControlFlow::Break(());
            }
            reporter.progress.bytes_done = base + p.bytes_transferred;
            reporter.tick(false);
            ControlFlow::Continue(())
        })
        .await;

    match result {
        Ok(handle) => {
            reporter.progress.bytes_done = base + size;
            Ok(handle)
        }
        Err(e) => {
            // The phone may hold a half-written file; do not leave it there.
            if let Some(partial) = e.partial {
                let _ = storage.delete(partial).await;
            }
            reporter.progress.bytes_done = base;
            Err(e.into())
        }
    }
}

/// What is already on the phone, looked up lazily per folder, so an upload
/// can merge into existing folders and avoid name clashes.
struct RemoteTree {
    root: Option<ObjectHandle>,
    folders: HashMap<String, Option<ObjectHandle>>,
    children: HashMap<Option<u64>, HashMap<String, (ObjectHandle, bool)>>,
}

impl RemoteTree {
    fn new(root: Option<ObjectHandle>) -> Self {
        let mut folders = HashMap::new();
        folders.insert(String::new(), root);
        Self { root, folders, children: HashMap::new() }
    }

    async fn names(
        &mut self,
        storage: &mtp_rs::Storage,
        folder: Option<ObjectHandle>,
    ) -> Result<&mut HashMap<String, (ObjectHandle, bool)>, UsbError> {
        let key = folder.map(|h| h.0);
        if !self.children.contains_key(&key) {
            let listing = storage
                .list_objects(folder)
                .await?
                .into_iter()
                .map(|o| (o.filename.to_lowercase(), (o.handle, o.is_folder())))
                .collect();
            self.children.insert(key, listing);
        }
        Ok(self.children.get_mut(&key).expect("just inserted"))
    }

    /// The handle of `rel_dir` under the upload root, creating folders as needed.
    async fn folder(
        &mut self,
        storage: &mtp_rs::Storage,
        rel_dir: &str,
    ) -> Result<Option<ObjectHandle>, UsbError> {
        if let Some(handle) = self.folders.get(rel_dir) {
            return Ok(*handle);
        }
        let mut current = self.root;
        let mut walked = String::new();
        for part in rel_dir.split('/').filter(|p| !p.is_empty()) {
            if !walked.is_empty() {
                walked.push('/');
            }
            walked.push_str(part);
            if let Some(handle) = self.folders.get(&walked) {
                current = *handle;
                continue;
            }
            let existing = self.names(storage, current).await?.get(&part.to_lowercase()).copied();
            let handle = match existing {
                Some((handle, true)) => handle,
                _ => {
                    let created = storage.create_folder(current, part).await?;
                    self.claim(current, part.to_string(), created, true);
                    created
                }
            };
            self.folders.insert(walked.clone(), Some(handle));
            current = Some(handle);
        }
        Ok(current)
    }

    /// `name`, or `name (2)` … if the folder already has one.
    async fn free_name(
        &mut self,
        storage: &mtp_rs::Storage,
        folder: Option<ObjectHandle>,
        name: &str,
    ) -> Result<String, UsbError> {
        let taken = self.names(storage, folder).await?;
        Ok(free_name(name, |candidate| taken.contains_key(&candidate.to_lowercase())))
    }

    fn claim(&mut self, folder: Option<ObjectHandle>, name: String, handle: ObjectHandle, is_folder: bool) {
        self.children
            .entry(folder.map(|h| h.0))
            .or_default()
            .insert(name.to_lowercase(), (handle, is_folder));
    }
}

pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    for n in 2..10_000 {
        let candidate = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{stem}-{}", uuid::Uuid::new_v4())
}

/// Deleted photos waiting in Android's trash, and files still being written.
/// Copying a folder should not resurrect them on the Mac.
pub fn is_android_scratch(name: &str) -> bool {
    name.starts_with(".trashed-") || name.starts_with(".pending-")
}

/// A name from the phone, made safe to use as one local path component.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '\0') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() || cleaned == ".." {
        "unnamed".into()
    } else {
        cleaned
    }
}

/// The computer's own UTC offset. Android writes file dates in its local time
/// with no zone, and the phone and the computer are in the same room.
fn local_offset() -> UtcOffset {
    #[cfg(unix)]
    {
        // SAFETY: `localtime_r` only writes into the `tm` we hand it.
        let minutes = unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                0
            } else {
                tm.tm_gmtoff / 60
            }
        };
        if let Some(offset) = i16::try_from(minutes).ok().and_then(UtcOffset::from_minutes) {
            return offset;
        }
    }
    UtcOffset::UTC
}

// ------------------------------------------------------------ app state

/// The open phone and the transfer running against it, held by the Tauri app.
#[derive(Default)]
pub struct UsbState {
    device: tokio::sync::Mutex<Option<(String, MtpDevice)>>,
    job: std::sync::Mutex<Option<(String, std::sync::Arc<AtomicBool>)>>,
}

impl UsbState {
    pub async fn set(&self, id: String, device: MtpDevice) {
        let previous = self.device.lock().await.replace((id, device));
        if let Some((_, old)) = previous {
            let _ = old.close().await;
        }
    }

    pub async fn device(&self) -> Result<MtpDevice, UsbError> {
        self.device
            .lock()
            .await
            .as_ref()
            .map(|(_, d)| d.clone())
            .ok_or_else(|| UsbError::new("not-connected", "No phone is open. Connect it again."))
    }

    pub async fn close(&self) {
        if let Some((_, device)) = self.device.lock().await.take() {
            let _ = device.close().await;
        }
    }

    /// Forget a phone that has gone away, so the next call says so plainly.
    pub async fn forget_if_gone(&self, error: &UsbError) {
        if error.code == "disconnected" {
            self.device.lock().await.take();
        }
    }

    /// Start a transfer; only one runs at a time.
    pub fn begin_job(&self) -> Result<(String, std::sync::Arc<AtomicBool>), UsbError> {
        let mut job = self.job.lock().expect("usb job lock");
        if job.is_some() {
            return Err(UsbError::new("busy", "A USB copy is already running."));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        *job = Some((id.clone(), flag.clone()));
        Ok((id, flag))
    }

    pub fn end_job(&self, id: &str) {
        let mut job = self.job.lock().expect("usb job lock");
        if job.as_ref().is_some_and(|(current, _)| current == id) {
            *job = None;
        }
    }

    pub fn cancel_job(&self) {
        if let Some((_, flag)) = self.job.lock().expect("usb job lock").as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_phones_sensibly() {
        assert_eq!(display_name(Some("Google"), Some("Pixel 9")), "Google Pixel 9");
        assert_eq!(display_name(Some("samsung"), Some("Samsung Galaxy S24")), "Samsung Galaxy S24");
        assert_eq!(display_name(None, None), "Android phone");
        assert_eq!(display_name(Some(" "), Some("SM-S921B")), "SM-S921B");
    }

    #[test]
    fn picks_a_free_name() {
        let taken = ["photo.jpg", "photo (2).jpg", "notes"];
        let is_taken = |n: &str| taken.contains(&n);
        assert_eq!(free_name("new.jpg", is_taken), "new.jpg");
        assert_eq!(free_name("photo.jpg", is_taken), "photo (3).jpg");
        assert_eq!(free_name("notes", is_taken), "notes (2)");
    }

    #[test]
    fn makes_phone_names_safe_locally() {
        assert_eq!(safe_name("IMG_001.jpg"), "IMG_001.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(".."), "unnamed");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("a\0b:c"), "a_b_c");
    }

    #[test]
    fn skips_androids_trash_and_half_written_files() {
        assert!(is_android_scratch(".trashed-1793893362-Screenshot_20261006.jpg"));
        assert!(is_android_scratch(".pending-1700000000-IMG_1.jpg"));
        assert!(!is_android_scratch("IMG_1.jpg"));
        assert!(!is_android_scratch(".nomedia"));
    }

    #[test]
    fn recognises_who_holds_the_phone() {
        let ioreg = r#"    |   "UsbExclusiveOwner" = "pid 607, ptpcamerad""#;
        assert_eq!(owner_from_ioreg(ioreg).as_deref(), Some("macOS Image Capture"));
        assert_eq!(owner_from_ioreg(r#""UsbExclusiveOwner" = "pid 9, coreaudiod""#), None);
    }
}
