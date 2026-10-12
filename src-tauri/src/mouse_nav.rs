//! Mouse side buttons (back / forward), as in a browser or Finder.
//!
//! macOS reports them as "other mouse" buttons 3 and 4, which the web view
//! does not reliably pass on to the page. A local event monitor catches them
//! first and forwards them to the UI as a `mouse-nav` event ("back" or
//! "forward"); the page decides what that means where it is.

/// Install the monitor. Call once, on the main thread, during setup.
#[cfg(target_os = "macos")]
pub fn install(app: &tauri::AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
    use std::ptr::NonNull;
    use tauri::Emitter;

    let handle = app.clone();
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the call's duration.
        let e = unsafe { event.as_ref() };
        let direction = match e.buttonNumber() {
            3 => "back",
            4 => "forward",
            _ => return event.as_ptr(),
        };
        // Act on the press; swallow both press and release so the web view
        // never sees half of a click.
        if e.r#type() == NSEventType::OtherMouseDown {
            let _ = handle.emit("mouse-nav", direction);
        }
        std::ptr::null_mut()
    });

    // SAFETY: the block returns the event it was given or null, as required.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::OtherMouseDown | NSEventMask::OtherMouseUp,
            &block,
        )
    };
    // The monitor lives as long as the app does.
    std::mem::forget(monitor);

}

/// Elsewhere the web view delivers these buttons to the page itself.
#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}
