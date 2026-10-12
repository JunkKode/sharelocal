//! ShareLocal — direct, cloud-free file transfer over the local network.
//!
//! The core is deliberately independent of Tauri: `AppState`, the discovery
//! backends, the HTTP server and the transfer engine can all be driven from a
//! plain tokio runtime, which is what the integration tests do.

pub mod app;
pub mod commands;
pub mod config;
pub mod discovery;
pub mod events;
pub mod history;
pub mod model;
pub mod mouse_nav;
pub mod net;
pub mod server;
pub mod state;
pub mod transfer;
pub mod usb;
pub mod wifi;

use state::AppState;
use std::sync::Arc;

/// Desktop entry point.
pub fn run() {
    app::run();
}

/// Human-readable byte size, matching the UI's formatting.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Start the server, discovery and the housekeeping tasks against an existing
/// state. Shared by the desktop app and the integration tests.
pub fn spawn_core(state: Arc<AppState>) {
    let server_state = state.clone();
    tokio::spawn(async move { server::run(server_state).await });
    discovery::spawn(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting_matches_the_ui() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(20 * 1024 * 1024 * 1024), "20.0 GB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
        // Three significant figures throughout, so the width stays stable.
        assert_eq!(format_bytes(150 * 1024 * 1024), "150 MB");
    }
}
