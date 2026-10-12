//! The data plane: writing incoming streams to disk and reading outgoing files
//! off it. Both directions are strictly chunked — no operation in this module
//! ever holds a whole file in memory.

pub mod paths;
pub mod send;
pub mod session;

use crate::config::PART_DIR;
use crate::events::AppEvent;
use crate::history::entry_from_session;
use crate::model::{SessionStatus, TransferError};
use crate::state::AppState;
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// How often progress is pushed to the UI while bytes are flowing. Fast enough
/// to look live, slow enough that a 20 GB transfer does not spam the bus.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

pub fn io_error(err: &std::io::Error, what: &str) -> TransferError {
    use std::io::ErrorKind::*;
    match err.kind() {
        PermissionDenied => TransferError::PermissionDenied(format!("{what}: permission denied")),
        NotFound => TransferError::FileMissing(format!("{what}: file no longer exists")),
        StorageFull => TransferError::NotEnoughSpace(format!("{what}: not enough disk space")),
        _ => {
            // `StorageFull` is not surfaced on every platform; fall back to the
            // raw errno so ENOSPC is still reported correctly.
            #[cfg(unix)]
            if err.raw_os_error() == Some(libc::ENOSPC) {
                return TransferError::NotEnoughSpace(format!("{what}: not enough disk space"));
            }
            TransferError::Other(format!("{what}: {err}"))
        }
    }
}

/// Free bytes on the volume holding `path`, when the platform can tell us.
pub fn available_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        // Walk up to the first directory that exists.
        let mut probe = path;
        while !probe.exists() {
            probe = probe.parent()?;
        }
        let c = CString::new(probe.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is a valid NUL-terminated path and `stat` is fully
        // initialised by the call before we read it.
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut stat) != 0 {
                return None;
            }
            Some(stat.f_bavail as u64 * stat.f_frsize as u64)
        }
    }
    #[cfg(not(unix))]
    {
        // Windows reports ENOSPC at write time, which we map in `io_error`.
        let _ = path;
        None
    }
}

/// Bytes already received for a file, used by the sender to resume.
pub async fn received_bytes(root: &Path, session_id: &str, index: usize) -> u64 {
    let part = paths::part_path(root, session_id, index);
    tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0)
}

/// Stream an incoming body into the session's `.part` file, then finalise it.
///
/// `offset` lets an interrupted transfer resume: the part file is truncated to
/// that length and the stream is appended from there.
pub async fn receive_file<S, E>(
    state: &Arc<AppState>,
    session_id: &str,
    index: usize,
    offset: u64,
    mut stream: S,
) -> Result<PathBuf, TransferError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let (root, expected_size, rel_path, control, conflict) = {
        let sessions = state.sessions.read().await;
        let s = sessions
            .get(session_id)
            .ok_or_else(|| TransferError::InvalidToken("unknown transfer session".into()))?;
        let file = s
            .files
            .get(index)
            .ok_or_else(|| TransferError::Other("file index out of range".into()))?;
        (
            s.dest_root.clone(),
            file.size,
            file.rel_path.clone(),
            s.control.clone(),
            s.conflict,
        )
    };

    // Validate the destination *before* accepting a single byte, so a hostile
    // path never even creates a part file.
    let rel = paths::sanitize_rel_path(&rel_path)?;
    let planned = paths::resolve_dest(&root, &rel)?;

    if let Some(free) = available_space(&root) {
        let needed = expected_size.saturating_sub(offset);
        if free < needed {
            return Err(TransferError::NotEnoughSpace(format!(
                "{} needs {} but only {} is free",
                rel_path,
                crate::format_bytes(needed),
                crate::format_bytes(free)
            )));
        }
    }

    let part = paths::part_path(&root, session_id, index);
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| io_error(&e, "creating the download folder"))?;
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        // Explicitly NOT truncating: an existing part file holds the bytes a
        // resume continues from. The length is set to `offset` below, which is
        // the only truncation that may happen here.
        .truncate(false)
        .write(true)
        .read(true)
        .open(&part)
        .await
        .map_err(|e| io_error(&e, "opening the incoming file"))?;
    // Truncate to the resume point so a retry can never interleave old bytes.
    file.set_len(offset)
        .await
        .map_err(|e| io_error(&e, "preparing the incoming file"))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| io_error(&e, "seeking the incoming file"))?;

    // Deliberately no `BufWriter`. When the sending side disappears, hyper
    // drops this handler mid-await and any buffered bytes would vanish with
    // it — taking the resume point with them. Writing each chunk straight
    // through means whatever arrived is already on disk.
    let mut writer = file;
    let mut written = offset;
    let mut last_publish = Instant::now();

    set_progress(state, session_id, index, written).await;

    // Every exit from this loop must flush, otherwise the bytes still sitting
    // in the buffer are lost and the transfer cannot be resumed from them.
    let outcome: Result<(), TransferError> = loop {
        let Some(chunk) = stream.next().await else {
            break Ok(());
        };
        if control.is_cancelled() {
            break Err(TransferError::Interrupted("transfer cancelled".into()));
        }
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(err) => break Err(TransferError::Interrupted(err.to_string())),
        };
        if chunk.is_empty() {
            continue;
        }
        // Refuse to write past the declared size — a peer must not be able to
        // fill the disk by sending more than it announced.
        if written.saturating_add(chunk.len() as u64) > expected_size {
            break Err(TransferError::Other(
                "sender exceeded the announced file size".into(),
            ));
        }

        if let Err(err) = writer.write_all(&chunk).await {
            break Err(io_error(&err, "writing the incoming file"));
        }
        written += chunk.len() as u64;

        record_bytes(state, session_id, chunk.len() as u64, written).await;
        if last_publish.elapsed() >= PROGRESS_INTERVAL {
            last_publish = Instant::now();
            state.publish_session(session_id).await;
        }
    };

    let flushed = writer.flush().await;
    drop(writer);
    outcome?;
    flushed.map_err(|e| io_error(&e, "flushing the incoming file"))?;

    if written < expected_size {
        // A short body means the connection dropped; keep the part file so the
        // sender can resume from exactly here.
        return Err(TransferError::Interrupted(format!(
            "connection closed after {} of {}",
            crate::format_bytes(written),
            crate::format_bytes(expected_size)
        )));
    }

    let final_path = finalize(&part, &planned, conflict).await?;
    Ok(final_path)
}

/// Move a completed `.part` into place, applying the conflict policy.
async fn finalize(
    part: &Path,
    planned: &Path,
    conflict: crate::model::ConflictPolicy,
) -> Result<PathBuf, TransferError> {
    if let Some(parent) = planned.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| io_error(&e, "creating the destination folder"))?;
    }

    let Some(dest) = paths::apply_conflict_policy(planned, conflict)? else {
        let _ = tokio::fs::remove_file(part).await;
        return Err(TransferError::DuplicateName(format!(
            "{} already exists",
            planned.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        )));
    };

    match tokio::fs::rename(part, &dest).await {
        Ok(()) => Ok(dest),
        Err(_) => {
            // Rename fails across filesystems; fall back to a streamed copy
            // rather than reading the file into memory.
            let mut src = tokio::fs::File::open(part)
                .await
                .map_err(|e| io_error(&e, "opening the finished file"))?;
            let mut out = tokio::fs::File::create(&dest)
                .await
                .map_err(|e| io_error(&e, "creating the destination file"))?;
            tokio::io::copy(&mut src, &mut out)
                .await
                .map_err(|e| io_error(&e, "moving the finished file"))?;
            out.flush().await.map_err(|e| io_error(&e, "finishing the file"))?;
            drop(out);
            let _ = tokio::fs::remove_file(part).await;
            Ok(dest)
        }
    }
}

async fn set_progress(state: &Arc<AppState>, session_id: &str, index: usize, done: u64) {
    let mut sessions = state.sessions.write().await;
    if let Some(s) = sessions.get_mut(session_id) {
        s.file_index = index;
        s.file_bytes_done = done;
        s.bytes_before_current = s.files.iter().take(index).map(|f| f.size).sum();
        if s.status == SessionStatus::Accepted {
            s.set_status(SessionStatus::Transferring);
        }
    }
}

async fn record_bytes(state: &Arc<AppState>, session_id: &str, delta: u64, total_for_file: u64) {
    let mut sessions = state.sessions.write().await;
    if let Some(s) = sessions.get_mut(session_id) {
        s.meter.record(delta);
        s.file_bytes_done = total_for_file;
    }
}

/// Delete any `.part` files belonging to a session.
pub async fn cleanup_parts(state: &Arc<AppState>, session_id: &str) {
    let root = {
        let sessions = state.sessions.read().await;
        sessions
            .get(session_id)
            .map(|s| s.dest_root.clone())
            .unwrap_or_else(|| state.data_dir.clone())
    };
    let dir = root.join(PART_DIR);
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { return };
    let prefix = format!("{session_id}-");
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".part") {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Save an accepted text share to the download folder.
///
/// The snippet arrived inside the offer, so there is no stream to run. The
/// filename is generated locally from the clock — a peer never gets to choose
/// it — which keeps this well clear of the path-handling rules that apply to
/// file transfers.
pub async fn save_shared_text(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<PathBuf, TransferError> {
    let (root, text, conflict) = {
        let sessions = state.sessions.read().await;
        let s = sessions
            .get(session_id)
            .ok_or_else(|| TransferError::Other("unknown transfer session".into()))?;
        (
            s.dest_root.clone(),
            s.text.clone().unwrap_or_default(),
            s.conflict,
        )
    };
    if text.is_empty() {
        return Err(TransferError::Other("the shared text was empty".into()));
    }

    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| io_error(&e, "creating the download folder"))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let planned = root.join(format!("shared-text-{stamp}.txt"));
    let dest = paths::apply_conflict_policy(&planned, conflict)?
        .unwrap_or_else(|| paths::unique_path(&planned));

    tokio::fs::write(&dest, text.as_bytes())
        .await
        .map_err(|e| io_error(&e, "saving the shared text"))?;

    {
        let mut sessions = state.sessions.write().await;
        if let Some(s) = sessions.get_mut(session_id) {
            s.saved_paths.push(dest.clone());
            s.files_done = s.files.len();
            s.bytes_before_current = s.total_bytes;
        }
    }
    Ok(dest)
}

/// Complete an accepted text share: write it out, then conclude.
///
/// Called from every place a text session can be accepted — the accept command
/// and the auto-accept path — so the two cannot drift apart.
pub async fn finish_text_receive(state: &Arc<AppState>, session_id: &str) {
    match save_shared_text(state, session_id).await {
        Ok(_) => conclude(state, session_id, SessionStatus::Completed, None).await,
        Err(err) => conclude(state, session_id, SessionStatus::Failed, Some(err)).await,
    }
}

/// Move a session to a terminal state: record history, notify, tidy up.
pub async fn conclude(
    state: &Arc<AppState>,
    session_id: &str,
    status: SessionStatus,
    error: Option<TransferError>,
) {
    let entry = {
        let mut sessions = state.sessions.write().await;
        let Some(s) = sessions.get_mut(session_id) else { return };
        if let Some(err) = error.clone() {
            s.error = Some(err);
        }
        s.set_status(status);
        if status == SessionStatus::Completed {
            s.files_done = s.files.len();
            s.bytes_before_current = s.total_bytes;
            s.file_bytes_done = 0;
        }
        Some(entry_from_session(s, status))
    };

    if status != SessionStatus::Completed {
        // Keep part files only for interruptions, which are resumable.
        if !matches!(status, SessionStatus::Paused | SessionStatus::Failed) {
            cleanup_parts(state, session_id).await;
        }
    }

    if let Some(entry) = entry {
        let interesting = matches!(
            status,
            SessionStatus::Completed | SessionStatus::Failed | SessionStatus::Cancelled
        );
        if interesting {
            let speed = entry.avg_speed_bps;
            let peer_id = entry.peer_id.clone();
            state.history.lock().await.push(entry);
            state.publish(AppEvent::HistoryChanged);
            if status == SessionStatus::Completed && speed > 0.0 {
                state.peers.write().await.record_speed(&peer_id, speed);
            }
        }
    }

    state.publish_session(session_id).await;
}

/// Build the file manifest for a set of local paths, expanding folders and
/// preserving their structure in `rel_path`.
pub fn collect_files(inputs: &[PathBuf]) -> Result<(Vec<crate::model::FileMeta>, Vec<PathBuf>), TransferError> {
    let mut metas = Vec::new();
    let mut sources = Vec::new();

    for input in inputs {
        let meta = std::fs::metadata(input)
            .map_err(|e| io_error(&e, &format!("reading {}", input.display())))?;
        if meta.is_file() {
            let name = input
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .ok_or_else(|| TransferError::InvalidPath("file has no name".into()))?;
            metas.push(file_meta(name, &meta, input));
            sources.push(input.clone());
        } else if meta.is_dir() {
            let base = input.parent().unwrap_or(Path::new(""));
            walk_dir(input, base, &mut metas, &mut sources)?;
        }
    }

    if metas.is_empty() {
        return Err(TransferError::FileMissing("nothing to send".into()));
    }
    Ok((metas, sources))
}

fn file_meta(rel_path: String, meta: &std::fs::Metadata, path: &Path) -> crate::model::FileMeta {
    crate::model::FileMeta {
        mime: mime_guess::from_path(path).first().map(|m| m.to_string()),
        modified_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        rel_path,
        size: meta.len(),
    }
}

fn walk_dir(
    dir: &Path,
    base: &Path,
    metas: &mut Vec<crate::model::FileMeta>,
    sources: &mut Vec<PathBuf>,
) -> Result<(), TransferError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| io_error(&e, &format!("reading {}", dir.display())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        // Do not follow symlinks out of the selected tree.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_dir(&path, base, metas, sources)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let rel_path = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            metas.push(file_meta(rel_path, &meta, &path));
            sources.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConflictPolicy;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sl-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn collect_files_lists_a_single_file() {
        let dir = tmp_dir("collect");
        let f = dir.join("note.txt");
        std::fs::write(&f, b"hello").unwrap();

        let (metas, sources) = collect_files(std::slice::from_ref(&f)).unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].rel_path, "note.txt");
        assert_eq!(metas[0].size, 5);
        assert_eq!(sources, vec![f.clone()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn collect_files_preserves_folder_structure() {
        let dir = tmp_dir("collect-tree");
        let root = dir.join("Trip");
        std::fs::create_dir_all(root.join("day1").join("raw")).unwrap();
        std::fs::write(root.join("readme.md"), b"x").unwrap();
        std::fs::write(root.join("day1").join("a.jpg"), b"xx").unwrap();
        std::fs::write(root.join("day1").join("raw").join("b.dng"), b"xxx").unwrap();

        let (metas, sources) = collect_files(std::slice::from_ref(&root)).unwrap();
        assert_eq!(metas.len(), 3);
        assert_eq!(sources.len(), 3);

        let mut rels: Vec<String> = metas.iter().map(|m| m.rel_path.clone()).collect();
        rels.sort();
        assert_eq!(
            rels,
            vec!["Trip/day1/a.jpg", "Trip/day1/raw/b.dng", "Trip/readme.md"]
        );
        // Every produced path must survive the receiving side's sanitiser.
        for rel in &rels {
            assert!(paths::sanitize_rel_path(rel).is_ok(), "{rel} must be safe");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn collect_files_skips_symlinks() {
        let dir = tmp_dir("collect-sym");
        let root = dir.join("folder");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("real.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.join("link.txt")).unwrap();

        let (metas, _) = collect_files(&[root]).unwrap();
        assert_eq!(metas.len(), 1, "symlinks are not followed out of the tree");
        assert_eq!(metas[0].rel_path, "folder/real.txt");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn collect_files_reports_a_missing_path() {
        let missing = std::env::temp_dir().join("definitely-not-here-9f1c");
        assert!(matches!(
            collect_files(&[missing]),
            Err(TransferError::FileMissing(_))
        ));
    }

    #[tokio::test]
    async fn finalize_moves_the_part_file_into_place() {
        let dir = tmp_dir("finalize");
        let part = dir.join("x.part");
        std::fs::write(&part, b"payload").unwrap();
        let planned = dir.join("out.bin");

        let final_path = finalize(&part, &planned, ConflictPolicy::KeepBoth).await.unwrap();
        assert_eq!(final_path, planned);
        assert_eq!(std::fs::read(&planned).unwrap(), b"payload");
        assert!(!part.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn finalize_keeps_both_on_conflict() {
        let dir = tmp_dir("finalize-keep");
        let planned = dir.join("out.bin");
        std::fs::write(&planned, b"original").unwrap();
        let part = dir.join("x.part");
        std::fs::write(&part, b"new").unwrap();

        let final_path = finalize(&part, &planned, ConflictPolicy::KeepBoth).await.unwrap();
        assert_eq!(final_path.file_name().unwrap(), "out (2).bin");
        assert_eq!(std::fs::read(&planned).unwrap(), b"original");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn finalize_replaces_when_asked() {
        let dir = tmp_dir("finalize-replace");
        let planned = dir.join("out.bin");
        std::fs::write(&planned, b"original").unwrap();
        let part = dir.join("x.part");
        std::fs::write(&part, b"new").unwrap();

        let final_path = finalize(&part, &planned, ConflictPolicy::Replace).await.unwrap();
        assert_eq!(final_path, planned);
        assert_eq!(std::fs::read(&planned).unwrap(), b"new");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn finalize_cancels_and_discards_the_part() {
        let dir = tmp_dir("finalize-cancel");
        let planned = dir.join("out.bin");
        std::fs::write(&planned, b"original").unwrap();
        let part = dir.join("x.part");
        std::fs::write(&part, b"new").unwrap();

        let err = finalize(&part, &planned, ConflictPolicy::Cancel).await.unwrap_err();
        assert!(matches!(err, TransferError::DuplicateName(_)));
        assert_eq!(std::fs::read(&planned).unwrap(), b"original");
        assert!(!part.exists(), "the discarded part file is cleaned up");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn available_space_is_plausible() {
        if let Some(free) = available_space(&std::env::temp_dir()) {
            assert!(free > 0, "a writable temp dir should report free space");
        }
    }

    #[test]
    fn io_errors_map_to_specific_codes() {
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x");
        assert!(matches!(
            io_error(&denied, "writing"),
            TransferError::PermissionDenied(_)
        ));
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "x");
        assert!(matches!(io_error(&missing, "reading"), TransferError::FileMissing(_)));
        #[cfg(unix)]
        {
            let full = std::io::Error::from_raw_os_error(libc::ENOSPC);
            assert!(matches!(
                io_error(&full, "writing"),
                TransferError::NotEnoughSpace(_)
            ));
        }
    }
}
