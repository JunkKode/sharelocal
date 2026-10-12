//! The USB (MTP) path, against `mtp-rs`'s virtual device: a fake phone whose
//! storage is a temporary folder. Same protocol code as a real phone, minus
//! the cable.

use mtp_rs::{MtpDevice, VirtualDeviceConfig, VirtualStorageConfig};
use sharelocal::usb::{self, UsbEntry, UsbProgress};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

struct Phone {
    device: MtpDevice,
    storage: String,
    /// What the phone's "internal storage" holds, on disk.
    root: PathBuf,
    /// Where the Mac side of each test lives.
    mac: PathBuf,
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

async fn phone() -> Phone {
    let base = std::env::temp_dir().join(format!("sl-usb-{}", uuid::Uuid::new_v4()));
    let root = base.join("phone");
    let mac = base.join("mac");
    write(&root.join("DCIM/Camera/IMG_0001.jpg"), &vec![1u8; 300_000]);
    write(&root.join("DCIM/Camera/IMG_0002.jpg"), b"second photo");
    write(&root.join("DCIM/Camera/Burst/IMG_0003.jpg"), b"burst");
    write(&root.join("Download/report.pdf"), b"%PDF-1.7 old");
    write(&root.join("notes.txt"), b"hello from the phone");
    // Android's trash and hidden folders: never listed, never copied.
    write(&root.join("DCIM/Camera/.trashed-1793893362-IMG_0000.jpg"), b"deleted");
    write(&root.join(".thumbnails/x.jpg"), b"cache");
    std::fs::create_dir_all(&mac).unwrap();

    let device = MtpDevice::builder()
        .open_virtual(VirtualDeviceConfig {
            manufacturer: "Google".into(),
            model: "Pixel 9".into(),
            serial: format!("virtual-{}", uuid::Uuid::new_v4()),
            storages: vec![VirtualStorageConfig {
                description: "Internal shared storage".into(),
                capacity: 64 * 1024 * 1024 * 1024,
                backing_dir: root.clone(),
                read_only: false,
            }],
            watch_backing_dirs: false,
            ..Default::default()
        })
        .await
        .expect("virtual phone opens");

    let connection = usb::describe(&device, "virtual").await.expect("describe");
    assert_eq!(connection.name, "Google Pixel 9");
    assert_eq!(connection.storages.len(), 1);
    let storage = connection.storages[0].id.clone();
    Phone { device, storage, root, mac }
}

async fn list(phone: &Phone, parent: Option<&str>) -> Vec<UsbEntry> {
    usb::list(&phone.device, &phone.storage, parent, false).await.expect("list")
}

async fn find(phone: &Phone, parent: Option<&str>, name: &str) -> UsbEntry {
    list(phone, parent)
        .await
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} not found"))
}

async fn download(phone: &Phone, handles: Vec<String>, dest: &Path) -> (UsbProgress, Vec<UsbProgress>) {
    let cancel = AtomicBool::new(false);
    let mut reports = Vec::new();
    let done = usb::download(&phone.device, &phone.storage, &handles, dest, "job", &cancel, |p| {
        reports.push(p.clone())
    })
    .await;
    (done, reports)
}

async fn upload(phone: &Phone, parent: Option<&str>, inputs: Vec<PathBuf>) -> UsbProgress {
    let cancel = AtomicBool::new(false);
    usb::upload(&phone.device, &phone.storage, parent, &inputs, "job", &cancel, |_| {}).await
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_folders_first_with_kinds() {
    let phone = phone().await;
    let root = list(&phone, None).await;
    let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["DCIM", "Download", "notes.txt"]);
    // Hidden files are there on request.
    let all = usb::list(&phone.device, &phone.storage, None, true).await.unwrap();
    assert_eq!(all[0].name, ".thumbnails");
    assert!(root[0].folder);
    assert_eq!(root[2].kind, "file");
    assert_eq!(root[2].size, 20);

    let dcim = find(&phone, None, "DCIM").await;
    let camera = find(&phone, Some(&dcim.handle), "Camera").await;
    let photos = list(&phone, Some(&camera.handle)).await;
    assert_eq!(photos[0].name, "Burst");
    assert_eq!(photos[1].kind, "image");
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_folders_with_their_structure_and_never_overwrites() {
    let phone = phone().await;
    let dcim = find(&phone, None, "DCIM").await;
    let notes = find(&phone, None, "notes.txt").await;
    let dest = phone.mac.join("Downloads");

    let (done, reports) = download(&phone, vec![dcim.handle.clone(), notes.handle.clone()], &dest).await;
    assert!(done.finished && done.error.is_none(), "{:?}", done.error);
    assert_eq!(done.files_total, 4);
    assert_eq!(done.files_done, 4);
    assert_eq!(done.bytes_done, done.bytes_total);
    assert!(reports.len() >= 2, "progress is reported along the way");
    assert_eq!(std::fs::read(dest.join("DCIM/Camera/IMG_0001.jpg")).unwrap().len(), 300_000);
    assert_eq!(std::fs::read(dest.join("DCIM/Camera/Burst/IMG_0003.jpg")).unwrap(), b"burst");
    assert_eq!(std::fs::read(dest.join("notes.txt")).unwrap(), b"hello from the phone");
    assert!(!dest.join("DCIM/Camera/.trashed-1793893362-IMG_0000.jpg").exists());

    // No part files are left behind.
    let leftovers: Vec<_> = walk(&dest).into_iter().filter(|p| p.to_string_lossy().contains("sharelocal-part")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // A second copy lands beside the first instead of replacing it.
    let (again, _) = download(&phone, vec![notes.handle.clone()], &dest).await;
    assert!(again.error.is_none());
    assert_eq!(again.saved, [dest.join("notes (2).txt").to_string_lossy().to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_merge_into_folders_and_rename_clashes() {
    let phone = phone().await;
    let trip = phone.mac.join("Trip");
    write(&trip.join("beach.jpg"), b"beach");
    write(&trip.join("day2/cave.jpg"), b"cave");
    let report = phone.mac.join("report.pdf");
    write(&report, b"%PDF-1.7 new");

    let download_dir = find(&phone, None, "Download").await;
    let done = upload(&phone, Some(&download_dir.handle), vec![trip.clone(), report.clone()]).await;
    assert!(done.finished && done.error.is_none(), "{:?}", done.error);
    assert_eq!(done.files_done, 3);

    let phone_download = phone.root.join("Download");
    assert_eq!(std::fs::read(phone_download.join("Trip/beach.jpg")).unwrap(), b"beach");
    assert_eq!(std::fs::read(phone_download.join("Trip/day2/cave.jpg")).unwrap(), b"cave");
    // The phone's own report.pdf is untouched; ours arrives under a new name.
    assert_eq!(std::fs::read(phone_download.join("report.pdf")).unwrap(), b"%PDF-1.7 old");
    assert_eq!(std::fs::read(phone_download.join("report (2).pdf")).unwrap(), b"%PDF-1.7 new");

    // Sending the folder again merges into it.
    let again = upload(&phone, Some(&download_dir.handle), vec![trip]).await;
    assert!(again.error.is_none(), "{:?}", again.error);
    assert!(phone_download.join("Trip/beach (2).jpg").exists());
    assert!(!phone_download.join("Trip (2)").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_and_deletes_folders() {
    let phone = phone().await;
    let handle = usb::create_folder(&phone.device, &phone.storage, None, "From Mac").await.unwrap();
    assert!(phone.root.join("From Mac").is_dir());

    let bad = usb::create_folder(&phone.device, &phone.storage, None, "../escape").await;
    assert_eq!(bad.unwrap_err().code, "invalid");

    usb::delete(&phone.device, &phone.storage, &[handle]).await.unwrap();
    assert!(!phone.root.join("From Mac").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_copy_stops_and_says_so() {
    let phone = phone().await;
    let dcim = find(&phone, None, "DCIM").await;
    let cancel = AtomicBool::new(true);
    let done = usb::download(
        &phone.device,
        &phone.storage,
        &[dcim.handle],
        &phone.mac.join("Downloads"),
        "job",
        &cancel,
        |_| {},
    )
    .await;
    assert_eq!(done.error.map(|e| e.code), Some("cancelled"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_mid_download_leaves_no_partial_file() {
    let phone = phone().await;
    write(&phone.root.join("big.mov"), &vec![7u8; 8 * 1024 * 1024]);
    let big = find(&phone, None, "big.mov").await;
    let dest = phone.mac.join("Downloads");
    let cancel = AtomicBool::new(false);
    let done = usb::download(&phone.device, &phone.storage, &[big.handle], &dest, "job", &cancel, |p| {
        if p.current == "big.mov" {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;
    assert_eq!(done.error.map(|e| e.code), Some("cancelled"));
    assert!(walk(&dest).is_empty(), "{:?}", walk(&dest));

    // The phone is still usable afterwards.
    assert_eq!(list(&phone, None).await.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_mid_upload_removes_the_half_written_file() {
    let phone = phone().await;
    let big = phone.mac.join("big.mov");
    write(&big, &vec![7u8; 8 * 1024 * 1024]);
    let cancel = AtomicBool::new(false);
    let done = usb::upload(&phone.device, &phone.storage, None, &[big], "job", &cancel, |p| {
        if p.current == "big.mov" {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;
    assert_eq!(done.error.map(|e| e.code), Some("cancelled"));
    assert!(!phone.root.join("big.mov").exists());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}
