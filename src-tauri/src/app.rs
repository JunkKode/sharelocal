//! Tauri wiring: window, tray, notifications and the bridge that turns core
//! events into UI events.

use crate::events::{AppEvent, Audience, NoticeLevel};
use crate::state::AppState;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // Launching at login should be unobtrusive: come up in the tray.
            Some(vec!["--start-minimized"]),
        ))
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("sharelocal"));
            std::fs::create_dir_all(&data_dir).ok();

            let state = AppState::new(data_dir);
            app.manage(state.clone());
            app.manage(crate::usb::UsbState::default());
            crate::mouse_nav::install(app.handle());

            build_tray(app.handle(), state.clone())?;

            // Everything below runs on Tauri's tokio runtime.
            let bridge_state = state.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                bridge_events(handle, bridge_state).await;
            });

            let core_state = state.clone();
            tauri::async_runtime::spawn(async move {
                crate::spawn_core(core_state);
            });

            sync_autostart(app.handle(), &state);

            // Hide on launch when the user asked for it, or when the OS
            // started us at login (the plugin passes the flag through).
            let launched_at_login = std::env::args().any(|a| a == "--start-minimized");
            let start_minimized = tauri::async_runtime::block_on(async {
                state.settings.read().await.start_minimized
            });
            if start_minimized || launched_at_login {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window leaves ShareLocal running in the tray, which
            // is what makes it available to receive files.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_snapshot,
            crate::commands::list_peers,
            crate::commands::refresh_peers,
            crate::commands::list_interfaces,
            crate::commands::get_settings,
            crate::commands::update_settings,
            crate::commands::set_download_dir,
            crate::commands::send_files,
            crate::commands::send_text,
            crate::commands::send_clipboard,
            crate::commands::copy_to_clipboard,
            crate::commands::send_files_to_web,
            crate::commands::session_token,
            crate::commands::accept_offer,
            crate::commands::reject_offer,
            crate::commands::cancel_transfer,
            crate::commands::pause_transfer,
            crate::commands::list_sessions,
            crate::commands::clear_finished,
            crate::commands::get_history,
            crate::commands::clear_history,
            crate::commands::remove_history_entry,
            crate::commands::create_pairing,
            crate::commands::revoke_pairing,
            crate::commands::open_path,
            crate::commands::reveal_path,
            crate::commands::open_download_dir,
            crate::commands::show_window,
            crate::commands::wifi_info,
            crate::commands::wifi_password,
            crate::commands::wifi_password_access,
            crate::commands::wifi_remember,
            crate::commands::wifi_forget,
            crate::commands::save_png,
            crate::commands::copy_image_to_clipboard,
            crate::commands::usb_devices,
            crate::commands::usb_connect,
            crate::commands::usb_disconnect,
            crate::commands::usb_storages,
            crate::commands::usb_list,
            crate::commands::usb_thumbnail,
            crate::commands::usb_create_folder,
            crate::commands::usb_delete,
            crate::commands::usb_download,
            crate::commands::usb_upload,
            crate::commands::usb_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start ShareLocal")
        .run(|_app, _event| {
            // Closing the window only hides it (the app keeps receiving in the
            // background), so clicking the Dock icon has to bring it back —
            // otherwise the app looks dead until the user finds the menu bar.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows: false, .. } = _event {
                show_main_window(_app);
            }
        });
}

fn build_tray(app: &tauri::AppHandle, state: Arc<AppState>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open ShareLocal", true, None::<&str>)?;
    let sharing = MenuItem::with_id(app, "sharing", "Pause sharing", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open download folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit ShareLocal", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &sharing,
            &folder,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let menu_state = state.clone();
    let sharing_item = sharing.clone();

    // A dedicated monochrome glyph: macOS menu-bar icons are "template"
    // images, which the system recolours for light and dark bars and inverts
    // on highlight. Feeding it the full-colour app icon produces a smudge.
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
        .or_else(|_| {
            app.default_window_icon()
                .cloned()
                .ok_or(tauri::Error::AssetNotFound("no tray icon available".into()))
        })?;

    let mut builder = TrayIconBuilder::with_id("sharelocal")
        .icon(tray_icon)
        .tooltip("ShareLocal")
        .menu(&menu)
        // Left-click opens the menu, which is what a menu-bar icon is expected
        // to do; without this a plain click does nothing at all.
        .show_menu_on_left_click(true);

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    builder
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "folder" => {
                let app = app.clone();
                let state = menu_state.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri_plugin_opener::OpenerExt;
                    let dir = state.download_dir().await;
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = app
                        .opener()
                        .open_path(dir.to_string_lossy().to_string(), None::<&str>);
                });
            }
            "sharing" => {
                let on = !menu_state.sharing();
                menu_state.set_sharing(on);
                let _ = sharing_item.set_text(if on { "Pause sharing" } else { "Resume sharing" });
                let state = menu_state.clone();
                tauri::async_runtime::spawn(async move {
                    state.publish_network().await;
                    state.publish(AppEvent::notice(
                        if on { NoticeLevel::Info } else { NoticeLevel::Warn },
                        if on { "sharing-on" } else { "sharing-off" },
                        if on {
                            "Sharing resumed — this device is visible again."
                        } else {
                            "Sharing paused — this device is hidden and will refuse transfers."
                        },
                    ));
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // With the menu on left-click, only a double-click is left as a
            // shortcut straight to the window.
            if let TrayIconEvent::DoubleClick { .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Make the OS login item match the stored setting.
fn sync_autostart(app: &tauri::AppHandle, state: &Arc<AppState>) {
    use tauri_plugin_autostart::ManagerExt;
    let wanted = tauri::async_runtime::block_on(async { state.settings.read().await.autostart });
    let manager = app.autolaunch();
    match manager.is_enabled() {
        Ok(current) if current == wanted => {}
        Ok(_) => {
            let result = if wanted { manager.enable() } else { manager.disable() };
            if let Err(err) = result {
                eprintln!("[autostart] could not update the login item: {err}");
            }
        }
        Err(err) => eprintln!("[autostart] could not read the login item: {err}"),
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Forward core events to the UI, and raise a desktop notification for the
/// things a user needs to see when the window is hidden.
async fn bridge_events(app: tauri::AppHandle, state: Arc<AppState>) {
    use tauri_plugin_notification::NotificationExt;

    let mut rx = state.events.subscribe();
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return,
        };

        let _ = app.emit(event.tauri_name(), &event);

        let notify = state.settings.read().await.notifications;
        if !notify {
            continue;
        }
        // Only surface events that need a human; progress is far too chatty.
        match &event {
            AppEvent::IncomingOffer { session } => {
                let _ = app
                    .notification()
                    .builder()
                    .title(format!("{} wants to send you files", session.peer.name))
                    .body(format!(
                        "{} · {}",
                        session.files.len(),
                        crate::format_bytes(session.total_bytes)
                    ))
                    .show();
                show_main_window(&app);
            }
            AppEvent::SessionUpdated { session }
                if session.status == crate::model::SessionStatus::Completed =>
            {
                let verb = match session.role {
                    crate::model::Role::Receiving => "Received from",
                    crate::model::Role::Sending => "Sent to",
                };
                let _ = app
                    .notification()
                    .builder()
                    .title("Transfer complete")
                    .body(format!("{verb} {}", session.peer.name))
                    .show();
            }
            AppEvent::Notice { level: NoticeLevel::Error, message, .. } => {
                let _ = app
                    .notification()
                    .builder()
                    .title("ShareLocal")
                    .body(message.clone())
                    .show();
            }
            _ => {}
        }
        debug_assert!(matches!(event.audience(), Audience::Local | Audience::Shared));
        let _ = state.server_ready.load(Ordering::SeqCst);
    }
}
