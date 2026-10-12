//! A ShareLocal node with no window.
//!
//! Useful for trying the browser client, for running a second instance on a
//! machine that already has the app open, and for manual testing on a headless
//! box. It auto-accepts nothing unless told to — the same rules as the app.
//!
//! ```sh
//! cargo run --example headless
//! cargo run --example headless -- --name "Test Box" --port 47700 --auto-accept
//! ```

use sharelocal::state::AppState;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut name = "ShareLocal (headless)".to_string();
    let mut port: u16 = 47_700;
    let mut auto_accept = false;
    let mut dir = std::env::temp_dir().join("sharelocal-headless");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = args.next().unwrap_or(name),
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--dir" => dir = args.next().map(Into::into).unwrap_or(dir),
            "--auto-accept" => auto_accept = true,
            "--help" | "-h" => {
                println!("usage: headless [--name NAME] [--port PORT] [--dir PATH] [--auto-accept]");
                return;
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    std::fs::create_dir_all(&dir).expect("could not create the data directory");
    let downloads = dir.join("Downloads");
    std::fs::create_dir_all(&downloads).expect("could not create the download directory");

    let state = AppState::new(dir.clone());
    {
        let mut settings = state.settings.write().await;
        settings.device_name = name.clone();
        settings.port = port;
        settings.auto_accept = auto_accept;
        settings.download_dir = downloads.clone();
    }

    sharelocal::spawn_core(state.clone());

    // Wait for the listener before printing an address people will paste.
    for _ in 0..100 {
        if state.server_ready.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let (pairing_url, _token) = state.issue_pairing().await;
    let network = state.network_status().await;

    println!("ShareLocal headless node");
    println!("  name          {name}");
    println!("  address       http://{}:{}", network.ipv4.unwrap_or_else(|| "127.0.0.1".into()), state.port());
    println!("  downloads     {}", downloads.display());
    println!("  auto-accept   {auto_accept}");
    println!();
    println!("Open this on a phone or in a browser:");
    println!("  {pairing_url}");
    println!();
    println!("Ctrl-C to stop.");

    // Report incoming offers and, when auto-accept is off, accept from stdin
    // would need a UI — so just surface them.
    let mut events = state.events.subscribe();
    loop {
        match events.recv().await {
            Ok(sharelocal::events::AppEvent::PeersChanged { peers }) => {
                if peers.is_empty() {
                    println!("peers: (none)");
                } else {
                    println!("peers:");
                    for p in peers {
                        println!(
                            "  {:<24} {:<16} {:?} via {:?}{}",
                            p.device.name,
                            p.primary.to_string(),
                            p.status,
                            p.source,
                            if p.same_subnet { "" } else { "  [different subnet]" }
                        );
                    }
                }
            }
            Ok(sharelocal::events::AppEvent::IncomingOffer { session }) => {
                println!(
                    "offer from {}: {} file(s), {}",
                    session.peer.name,
                    session.files.len(),
                    sharelocal::format_bytes(session.total_bytes)
                );
                if !auto_accept {
                    println!("  (start with --auto-accept to receive it)");
                }
            }
            Ok(sharelocal::events::AppEvent::SessionUpdated { session })
                if session.status == sharelocal::model::SessionStatus::Completed =>
            {
                println!(
                    "completed: {} ({})",
                    session
                        .files
                        .first()
                        .map(|f| f.rel_path.as_str())
                        .unwrap_or("transfer"),
                    sharelocal::format_bytes(session.total_bytes)
                );
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(_) => break,
        }
    }
}
