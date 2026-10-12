//! Throughput benchmarks.
//!
//! These are `#[ignore]`d because they write multi-gigabyte fixtures. Run them
//! deliberately:
//!
//! ```sh
//! # the default sweep: 100 MB and 1 GB
//! cargo test --release --test perf -- --ignored --nocapture
//!
//! # a specific size, in megabytes (5 GB, 20 GB, …)
//! SHARELOCAL_PERF_MB=20480 cargo test --release --test perf -- --ignored --nocapture large_file_throughput
//! ```
//!
//! The transfer runs between two real instances over loopback, so the numbers
//! measure the streaming pipeline rather than the network.

mod harness;

use harness::*;
use sharelocal::model::SessionStatus;
use sharelocal::transfer::send::start_send;
use std::time::{Duration, Instant};

/// Write a fixture without holding it in memory — the point of the exercise.
fn write_fixture(path: &std::path::Path, bytes: u64) {
    use std::io::Write;
    let mut file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path).unwrap());
    let block: Vec<u8> = (0..(1 << 20)).map(|i| (i % 251) as u8).collect();
    let mut written = 0u64;
    while written < bytes {
        let take = ((bytes - written) as usize).min(block.len());
        file.write_all(&block[..take]).unwrap();
        written += take as u64;
    }
    file.flush().unwrap();
}

async fn run_one(megabytes: u64) {
    let bytes = megabytes * 1024 * 1024;
    let sender = start_node("PerfSender", false).await;
    let receiver = start_node("PerfReceiver", true).await;

    let source = sender.dir.join("perf.bin");
    eprintln!("preparing a {megabytes} MB fixture…");
    write_fixture(&source, bytes);

    let peer = receiver.as_peer().await;
    let started = Instant::now();
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    // Generous ceiling: even a slow disk should manage 10 MB/s.
    let budget = Duration::from_secs(60 + megabytes / 10);
    let finished = await_terminal(&sender, &id, budget).await;
    let elapsed = started.elapsed();

    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);

    let landed = receiver.downloads.join("perf.bin");
    let landed_len = std::fs::metadata(&landed).unwrap().len();
    assert_eq!(landed_len, bytes, "the whole file must arrive");

    let mb_per_sec = (bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    eprintln!(
        "{megabytes:>6} MB in {:>6.2}s  =  {mb_per_sec:>7.1} MB/s",
        elapsed.as_secs_f64()
    );

    // Tidy up immediately; these fixtures are large.
    std::fs::remove_file(&landed).ok();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes a 100 MB fixture"]
async fn throughput_100mb() {
    run_one(100).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes a 1 GB fixture"]
async fn throughput_1gb() {
    run_one(1024).await;
}

/// Size comes from `SHARELOCAL_PERF_MB` so 5 GB and 20 GB runs need no rebuild.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "size is set by SHARELOCAL_PERF_MB; defaults to 5 GB"]
async fn large_file_throughput() {
    let megabytes: u64 = std::env::var("SHARELOCAL_PERF_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5 * 1024);

    if let Some(free) = sharelocal::transfer::available_space(&std::env::temp_dir()) {
        // The fixture plus the received copy both live on the temp volume.
        let needed = megabytes * 1024 * 1024 * 2 + (1 << 30);
        if free < needed {
            eprintln!(
                "skipping: {} free, {} needed for a {megabytes} MB round trip",
                sharelocal::format_bytes(free),
                sharelocal::format_bytes(needed)
            );
            return;
        }
    }
    run_one(megabytes).await;
}

/// The memory guarantee: a file far larger than any buffer must transfer with
/// the process's resident set staying flat.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes a 2 GB fixture"]
async fn a_large_transfer_does_not_grow_the_resident_set() {
    let before = resident_bytes();
    run_one(2048).await;
    let after = resident_bytes();

    if let (Some(before), Some(after)) = (before, after) {
        let growth = after.saturating_sub(before);
        eprintln!(
            "resident set: {} → {} (+{})",
            sharelocal::format_bytes(before),
            sharelocal::format_bytes(after),
            sharelocal::format_bytes(growth)
        );
        assert!(
            growth < 256 * 1024 * 1024,
            "moving 2 GB grew memory by {} — the file is being buffered, not streamed",
            sharelocal::format_bytes(growth)
        );
    } else {
        eprintln!("skipping the memory assertion: this platform reports no RSS");
    }
}

/// Resident set size of this process, in bytes.
fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
                return Some(kb * 1024);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}
