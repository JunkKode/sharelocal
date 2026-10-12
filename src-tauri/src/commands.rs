//! Commands the desktop UI invokes. These are the *only* way the front end can
//! reach the core, and none of them accept a path from a remote device.

use crate::config::Settings;
use crate::model::{
    ConflictPolicy, HistoryEntry, NetworkStatus, Peer, SessionStatus, SessionView, TransferError,
};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Manager, State};

/// Commands return a plain string error; the UI already receives structured
/// failures through `session-updated` events.
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub device: crate::model::DeviceInfo,
    pub network: NetworkStatus,
    pub peers: Vec<Peer>,
    pub sessions: Vec<SessionView>,
    pub settings: SettingsView,
}

/// Settings as the UI sees them. The PIN is reported as a boolean plus its
/// value only for the *local* window, which is trusted; remote clients never
/// receive this struct.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SettingsView {
    pub device_name: String,
    pub download_dir: String,
    pub auto_accept: bool,
    pub pin: Option<String>,
    pub max_concurrent: usize,
    pub conflict_policy: ConflictPolicy,
    pub port: u16,
    pub preferred_interface: Option<String>,
    pub discovery_mdns: bool,
    pub discovery_udp: bool,
    pub tls: bool,
    pub theme: String,
    pub autostart: bool,
    pub start_minimized: bool,
    pub notifications: bool,
}

impl From<&Settings> for SettingsView {
    fn from(s: &Settings) -> Self {
        Self {
            device_name: s.device_name.clone(),
            download_dir: s.download_dir.to_string_lossy().to_string(),
            auto_accept: s.auto_accept,
            pin: s.pin.clone(),
            max_concurrent: s.max_concurrent,
            conflict_policy: s.conflict_policy,
            port: s.port,
            preferred_interface: s.preferred_interface.clone(),
            discovery_mdns: s.discovery_mdns,
            discovery_udp: s.discovery_udp,
            tls: s.tls,
            theme: s.theme.clone(),
            autostart: s.autostart,
            start_minimized: s.start_minimized,
            notifications: s.notifications,
        }
    }
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, Arc<AppState>>) -> CmdResult<Snapshot> {
    let settings = { SettingsView::from(&*state.settings.read().await) };
    Ok(Snapshot {
        device: state.device_info().await,
        network: state.network_status().await,
        peers: state.peers.read().await.list(),
        sessions: state.sessions.read().await.views(),
        settings,
    })
}

#[tauri::command]
pub async fn list_peers(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Peer>> {
    Ok(state.peers.read().await.list())
}

/// Ask every device on the LAN to announce itself now.
#[tauri::command]
pub async fn refresh_peers(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let state = state.inner().clone();
    crate::discovery::udp::probe(&state).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InterfaceView {
    pub name: String,
    pub ip: String,
}

#[tauri::command]
pub async fn list_interfaces() -> CmdResult<Vec<serde_json::Value>> {
    Ok(crate::net::usable_interfaces()
        .into_iter()
        .map(|i| {
            serde_json::json!({
                "name": i.name,
                "ip": i.ip.to_string(),
                "link": i.link,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> CmdResult<SettingsView> {
    Ok(SettingsView::from(&*state.settings.read().await))
}

#[tauri::command]
pub async fn update_settings(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    patch: SettingsView,
) -> CmdResult<SettingsView> {
    let autostart_changed = state.settings.read().await.autostart != patch.autostart;
    let want_autostart = patch.autostart;
    let (needs_restart, view) = {
        let mut s = state.settings.write().await;
        let restart = s.port != patch.port || s.tls != patch.tls;

        s.device_name = patch.device_name;
        s.download_dir = PathBuf::from(patch.download_dir);
        s.auto_accept = patch.auto_accept;
        s.pin = patch.pin.filter(|p| !p.trim().is_empty());
        s.max_concurrent = patch.max_concurrent;
        s.conflict_policy = patch.conflict_policy;
        s.port = patch.port;
        s.preferred_interface = patch.preferred_interface.filter(|i| !i.trim().is_empty());
        s.discovery_mdns = patch.discovery_mdns;
        s.discovery_udp = patch.discovery_udp;
        s.tls = patch.tls;
        s.theme = patch.theme;
        s.autostart = patch.autostart;
        s.start_minimized = patch.start_minimized;
        s.notifications = patch.notifications;
        s.normalise();
        (restart, SettingsView::from(&*s))
    };

    state.save_settings().await.map_err(err)?;

    if autostart_changed {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        let result = if want_autostart { manager.enable() } else { manager.disable() };
        if let Err(e) = result {
            // The preference is saved either way; tell the user the OS refused.
            return Err(format!("Saved, but the login item could not be updated: {e}"));
        }
    }

    state.publish(crate::events::AppEvent::SettingsChanged);
    // A renamed device must re-advertise so peers see the new name.
    state.network_changed.notify_waiters();
    if needs_restart {
        state.request_restart();
    }
    state.publish_network().await;
    Ok(view)
}

/// Create the download folder if it does not exist yet, and confirm it is
/// writable — a bad folder is much better caught here than mid-transfer.
#[tauri::command]
pub async fn set_download_dir(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<String> {
    let dir = PathBuf::from(&path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot use that folder: {e}"))?;
    let probe = dir.join(".sharelocal-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| format!("That folder is not writable: {e}"))?;
    let _ = std::fs::remove_file(&probe);

    state.settings.write().await.download_dir = dir.clone();
    state.save_settings().await.map_err(err)?;
    state.publish(crate::events::AppEvent::SettingsChanged);
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn send_files(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    paths: Vec<String>,
    pin: Option<String>,
) -> CmdResult<String> {
    if paths.is_empty() {
        return Err("No files selected.".into());
    }
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let state = state.inner().clone();

    // A browser that joined by QR has no server of its own — it is a client,
    // and dialling it would try to connect to port 0. Those transfers are
    // turned around: we publish the files and the browser fetches them.
    if state.web_clients.read().await.contains_key(&peer_id) {
        return crate::transfer::send::offer_to_web(&state, &peer_id, inputs, None)
            .await
            .map_err(|e| e.message().to_string());
    }

    let peer = state
        .peers
        .read()
        .await
        .get(&peer_id)
        .ok_or_else(|| "That device is no longer available.".to_string())?;

    crate::transfer::send::start_send(&state, peer, inputs, pin)
        .await
        .map_err(|e| e.message().to_string())
}

/// Offer files to a browser client, which then downloads them.
///
/// `send_files` routes here automatically for a paired browser; this remains
/// for callers that already know the destination is one.
#[tauri::command]
pub async fn send_files_to_web(
    state: State<'_, Arc<AppState>>,
    client_id: String,
    paths: Vec<String>,
) -> CmdResult<String> {
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let state = state.inner().clone();
    crate::transfer::send::offer_to_web(&state, &client_id, inputs, None)
        .await
        .map_err(|e| e.message().to_string())
}

/// The token a browser client needs to fetch a `Pull` session's files.
#[tauri::command]
pub async fn session_token(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<String> {
    let sessions = state.sessions.read().await;
    sessions
        .get(&session_id)
        .map(|s| s.token().to_string())
        .ok_or_else(|| "Unknown transfer.".to_string())
}

/// Send a text snippet to a peer. Small enough to travel with the offer, so
/// it lands the moment it is accepted.
#[tauri::command]
pub async fn send_text(
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    text: String,
    pin: Option<String>,
) -> CmdResult<String> {
    let trimmed = text.trim_end_matches(['\n', '\r']).to_string();
    if trimmed.trim().is_empty() {
        return Err("There is nothing to send.".into());
    }
    if trimmed.len() > crate::model::MAX_TEXT_BYTES {
        return Err(format!(
            "Text is limited to {} KB. Save it as a file and send that instead.",
            crate::model::MAX_TEXT_BYTES / 1024
        ));
    }

    let state = state.inner().clone();

    // Same reasoning as `send_files`: a browser cannot be dialled.
    if state.web_clients.read().await.contains_key(&peer_id) {
        return crate::transfer::send::offer_to_web(&state, &peer_id, Vec::new(), Some(trimmed))
            .await
            .map_err(|e| e.message().to_string());
    }

    let peer = state
        .peers
        .read()
        .await
        .get(&peer_id)
        .ok_or_else(|| "That device is no longer available.".to_string())?;

    crate::transfer::send::start_text_send(&state, peer, trimmed, pin)
        .await
        .map_err(|e| e.message().to_string())
}

/// Read the clipboard and share it, so "copy here, paste there" is one click.
#[tauri::command]
pub async fn send_clipboard(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_id: String,
    pin: Option<String>,
) -> CmdResult<String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let text = app
        .clipboard()
        .read_text()
        .map_err(|_| "Nothing readable on the clipboard — copy some text first.".to_string())?;
    send_text(state, peer_id, text, pin).await
}

/// Put a received snippet back on the clipboard.
#[tauri::command]
pub async fn copy_to_clipboard(app: tauri::AppHandle, text: String) -> CmdResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(err)
}

#[tauri::command]
pub async fn accept_offer(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    conflict: Option<ConflictPolicy>,
) -> CmdResult<()> {
    let kind = {
        let mut sessions = state.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| "That transfer request has expired.".to_string())?;
        if session.status != SessionStatus::Pending {
            return Err("That transfer is no longer waiting for an answer.".into());
        }
        if let Some(policy) = conflict {
            session.conflict = policy;
        }
        session.set_status(SessionStatus::Accepted);
        session.kind
    };

    if kind == crate::model::PayloadKind::Text {
        // A text share arrives with the offer, so accepting it *is* receiving
        // it: write it out and finish, rather than waiting for a stream that
        // will never come.
        let state = state.inner().clone();
        crate::transfer::finish_text_receive(&state, &session_id).await;
        return Ok(());
    }

    state.publish_session(&session_id).await;
    Ok(())
}

#[tauri::command]
pub async fn reject_offer(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    {
        let sessions = state.sessions.read().await;
        if let Some(s) = sessions.get(&session_id) {
            s.control.cancel();
        }
    }
    let state = state.inner().clone();
    crate::transfer::conclude(
        &state,
        &session_id,
        SessionStatus::Rejected,
        Some(TransferError::Rejected("declined".into())),
    )
    .await;
    Ok(())
}

#[tauri::command]
pub async fn cancel_transfer(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    {
        let sessions = state.sessions.read().await;
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| "Unknown transfer.".to_string())?;
        session.control.cancel();
    }
    let state = state.inner().clone();
    crate::transfer::conclude(&state, &session_id, SessionStatus::Cancelled, None).await;
    Ok(())
}

#[tauri::command]
pub async fn pause_transfer(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    paused: bool,
) -> CmdResult<()> {
    {
        let mut sessions = state.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| "Unknown transfer.".to_string())?;
        session.control.set_paused(paused);
        session.set_status(if paused {
            SessionStatus::Paused
        } else {
            SessionStatus::Transferring
        });
    }
    state.publish_session(&session_id).await;
    Ok(())
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SessionView>> {
    Ok(state.sessions.read().await.views())
}

#[tauri::command]
pub async fn clear_finished(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let removed: Vec<String> = {
        let mut sessions = state.sessions.write().await;
        let ids: Vec<String> = sessions
            .views()
            .into_iter()
            .filter(|v| v.status.is_terminal())
            .map(|v| v.id)
            .collect();
        for id in &ids {
            sessions.remove(id);
        }
        ids
    };
    for id in removed {
        state.publish(crate::events::AppEvent::SessionRemoved { id });
    }
    Ok(())
}

#[tauri::command]
pub async fn get_history(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<HistoryEntry>> {
    Ok(state.history.lock().await.entries().to_vec())
}

#[tauri::command]
pub async fn clear_history(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.history.lock().await.clear();
    state.publish(crate::events::AppEvent::HistoryChanged);
    Ok(())
}

#[tauri::command]
pub async fn remove_history_entry(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.history.lock().await.remove(&id);
    state.publish(crate::events::AppEvent::HistoryChanged);
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct PairingView {
    pub url: String,
    pub expires_in_secs: u64,
}

#[tauri::command]
pub async fn create_pairing(state: State<'_, Arc<AppState>>) -> CmdResult<PairingView> {
    let (url, _token) = state.issue_pairing().await;
    Ok(PairingView {
        url,
        expires_in_secs: crate::state::PAIRING_TTL.as_secs(),
    })
}

#[tauri::command]
pub async fn revoke_pairing(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.revoke_pairing().await;
    Ok(())
}

/// Open a received file with the system handler.
#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, path: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err("That file is no longer there.".into());
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(err)
}

/// Reveal in Finder / Explorer / the Linux file manager.
#[tauri::command]
pub async fn reveal_path(app: tauri::AppHandle, path: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = PathBuf::from(path);
    let target = if path.exists() {
        path
    } else {
        // Fall back to the containing folder if the file has been moved.
        path.parent()
            .map(|p| p.to_path_buf())
            .ok_or("That location no longer exists.")?
    };
    app.opener().reveal_item_in_dir(&target).map_err(err)
}

#[tauri::command]
pub async fn open_download_dir(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = state.download_dir().await;
    std::fs::create_dir_all(&dir).map_err(err)?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(err)
}

/// The Wi-Fi network this computer is on, for the "share Wi-Fi" QR code.
#[tauri::command]
pub async fn wifi_info() -> CmdResult<crate::wifi::WifiInfo> {
    Ok(crate::wifi::current().await)
}

/// The saved password for a network. On macOS this raises the system's own
/// Keychain prompt, so it only ever runs when the user presses the button.
#[tauri::command]
pub async fn wifi_password(ssid: String) -> CmdResult<String> {
    crate::wifi::password(&ssid).await
}

/// Whether the password button means Touch ID or the first-time admin dialog.
#[tauri::command]
pub async fn wifi_password_access(ssid: String) -> CmdResult<crate::wifi::PasswordAccess> {
    Ok(crate::wifi::access(&ssid))
}

/// Keep a password the user typed, so next time it is one Touch ID away.
#[tauri::command]
pub async fn wifi_remember(ssid: String, password: String) -> CmdResult<()> {
    crate::wifi::remember(&ssid, &password);
    Ok(())
}

#[tauri::command]
pub async fn wifi_forget(ssid: String) -> CmdResult<()> {
    crate::wifi::forget(&ssid);
    Ok(())
}

/// PNGs the UI draws (QR codes) are a few hundred KB at most.
const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

fn decode_png(png_base64: &str) -> CmdResult<Vec<u8>> {
    use base64::Engine;
    let data = png_base64
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(png_base64);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "That image could not be read.".to_string())?;
    if bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("That image could not be read.".into());
    }
    Ok(bytes)
}

/// Save a PNG the UI drew into the download folder and return where it went.
///
/// Deliberately takes a *name*, not a path: the file can only land in the
/// download folder, under a sanitised name that never overwrites anything.
#[tauri::command]
pub async fn save_png(
    state: State<'_, Arc<AppState>>,
    name: String,
    png_base64: String,
) -> CmdResult<String> {
    let bytes = decode_png(&png_base64)?;
    let stem: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '-' })
        .collect::<String>()
        .trim_matches(['.', ' '])
        .chars()
        .take(80)
        .collect();
    let stem = if stem.is_empty() { "ShareLocal".to_string() } else { stem };

    let dir = state.download_dir().await;
    std::fs::create_dir_all(&dir).map_err(err)?;
    let dest = crate::transfer::paths::unique_path(&dir.join(format!("{stem}.png")));
    tokio::fs::write(&dest, bytes).await.map_err(err)?;
    Ok(dest.to_string_lossy().to_string())
}

/// Put a PNG on the clipboard as an image, ready to paste into a chat.
#[tauri::command]
pub async fn copy_image_to_clipboard(app: tauri::AppHandle, png_base64: String) -> CmdResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let bytes = decode_png(&png_base64)?;
    let image = tauri::image::Image::from_bytes(&bytes).map_err(err)?;
    app.clipboard().write_image(&image).map_err(err)
}

// ------------------------------------------------------------------- USB
//
// An Android phone on a cable, over MTP. These return a structured
// `UsbError` rather than a string, so the UI can tell "macOS is holding the
// phone" from "the phone is locked" and offer the right fix.

use crate::usb::{UsbConnection, UsbDevice, UsbEntry, UsbError, UsbState, UsbStorage};

type UsbResult<T> = Result<T, UsbError>;

/// Run a phone operation, forgetting the phone if it turns out to be gone.
async fn with_phone<T, F, Fut>(usb: &UsbState, op: F) -> UsbResult<T>
where
    F: FnOnce(mtp_rs::MtpDevice) -> Fut,
    Fut: std::future::Future<Output = UsbResult<T>>,
{
    let device = usb.device().await?;
    let result = op(device).await;
    if let Err(e) = &result {
        usb.forget_if_gone(e).await;
    }
    result
}

#[tauri::command]
pub async fn usb_devices() -> UsbResult<Vec<UsbDevice>> {
    tokio::task::spawn_blocking(crate::usb::list_devices)
        .await
        .map_err(|e| UsbError::new("other", e.to_string()))
}

/// Open a phone. `release` first frees it from macOS Image Capture /
/// Android File Transfer — only when the user pressed the button that says so.
#[tauri::command]
pub async fn usb_connect(
    usb: State<'_, UsbState>,
    id: String,
    release: bool,
) -> UsbResult<UsbConnection> {
    usb.close().await;
    let device = crate::usb::open(&id, release).await?;
    match crate::usb::describe(&device, &id).await {
        Ok(view) => {
            usb.set(id, device).await;
            Ok(view)
        }
        Err(e) => {
            let _ = device.close().await;
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn usb_disconnect(usb: State<'_, UsbState>) -> UsbResult<()> {
    usb.cancel_job();
    usb.close().await;
    Ok(())
}

#[tauri::command]
pub async fn usb_storages(usb: State<'_, UsbState>) -> UsbResult<Vec<UsbStorage>> {
    with_phone(&usb, |d| async move { crate::usb::storages(&d).await }).await
}

#[tauri::command]
pub async fn usb_list(
    usb: State<'_, UsbState>,
    storage: String,
    parent: Option<String>,
    hidden: Option<bool>,
) -> UsbResult<Vec<UsbEntry>> {
    with_phone(&usb, |d| async move {
        crate::usb::list(&d, &storage, parent.as_deref(), hidden.unwrap_or(false)).await
    })
    .await
}

#[tauri::command]
pub async fn usb_thumbnail(
    usb: State<'_, UsbState>,
    storage: String,
    handle: String,
) -> UsbResult<String> {
    with_phone(&usb, |d| async move { crate::usb::thumbnail(&d, &storage, &handle).await }).await
}

#[tauri::command]
pub async fn usb_create_folder(
    usb: State<'_, UsbState>,
    storage: String,
    parent: Option<String>,
    name: String,
) -> UsbResult<String> {
    with_phone(&usb, |d| async move {
        crate::usb::create_folder(&d, &storage, parent.as_deref(), &name).await
    })
    .await
}

#[tauri::command]
pub async fn usb_delete(
    usb: State<'_, UsbState>,
    storage: String,
    handles: Vec<String>,
) -> UsbResult<()> {
    with_phone(&usb, |d| async move { crate::usb::delete(&d, &storage, &handles).await }).await
}

/// Copy from the phone into `dest` (the download folder when `None`). Runs in
/// the background; progress arrives as `usb-progress` events. Returns the job id.
#[tauri::command]
pub async fn usb_download(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    usb: State<'_, UsbState>,
    storage: String,
    handles: Vec<String>,
    dest: Option<String>,
) -> UsbResult<String> {
    if handles.is_empty() {
        return Err(UsbError::new("invalid", "Nothing is selected."));
    }
    let device = usb.device().await?;
    let dest = match dest {
        Some(d) => PathBuf::from(d),
        None => state.download_dir().await,
    };
    let (job, cancel) = usb.begin_job()?;
    let id = job.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Emitter;
        let emitter = app.clone();
        let done = crate::usb::download(&device, &storage, &handles, &dest, &job, &cancel, |p| {
            let _ = emitter.emit("usb-progress", p);
        })
        .await;
        let usb = app.state::<UsbState>();
        if let Some(e) = &done.error {
            usb.forget_if_gone(e).await;
        }
        usb.end_job(&job);
    });
    Ok(id)
}

/// Copy files and folders from this computer into `parent` on the phone.
#[tauri::command]
pub async fn usb_upload(
    app: tauri::AppHandle,
    usb: State<'_, UsbState>,
    storage: String,
    parent: Option<String>,
    paths: Vec<String>,
) -> UsbResult<String> {
    if paths.is_empty() {
        return Err(UsbError::new("invalid", "Nothing is selected."));
    }
    let device = usb.device().await?;
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (job, cancel) = usb.begin_job()?;
    let id = job.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Emitter;
        let emitter = app.clone();
        let done = crate::usb::upload(&device, &storage, parent.as_deref(), &inputs, &job, &cancel, |p| {
            let _ = emitter.emit("usb-progress", p);
        })
        .await;
        let usb = app.state::<UsbState>();
        if let Some(e) = &done.error {
            usb.forget_if_gone(e).await;
        }
        usb.end_job(&job);
    });
    Ok(id)
}

#[tauri::command]
pub async fn usb_cancel(usb: State<'_, UsbState>) -> UsbResult<()> {
    usb.cancel_job();
    Ok(())
}

#[tauri::command]
pub async fn show_window(app: tauri::AppHandle) -> CmdResult<()> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_view_round_trips_through_json() {
        let settings = Settings::default();
        let view = SettingsView::from(&settings);
        let json = serde_json::to_string(&view).unwrap();
        let back: SettingsView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.port, settings.port);
        assert_eq!(back.device_name, settings.device_name);
        assert_eq!(back.max_concurrent, settings.max_concurrent);
    }

    #[test]
    fn settings_view_reports_the_download_dir_as_a_string() {
        let settings = Settings {
            download_dir: PathBuf::from("/tmp/ShareLocal"),
            ..Default::default()
        };
        let view = SettingsView::from(&settings);
        assert_eq!(view.download_dir, "/tmp/ShareLocal");
    }
}
