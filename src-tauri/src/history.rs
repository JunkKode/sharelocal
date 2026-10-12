//! Local, append-only transfer history.
//!
//! Only metadata is recorded — never file contents. The file lives beside the
//! settings and is capped so it cannot grow without bound.

use crate::model::{HistoryEntry, Role, SessionStatus};
use crate::transfer::session::{now_ms, Session};
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 500;

#[derive(Debug)]
pub struct History {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl History {
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<HistoryEntry>>(&b).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
        self.persist();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.persist();
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
        self.persist();
    }

    fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.entries) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// Build a history row from a finished session.
pub fn entry_from_session(session: &Session, status: SessionStatus) -> HistoryEntry {
    let duration_ms = session.meter.elapsed().as_millis() as u64;
    HistoryEntry {
        id: session.id.clone(),
        at_ms: now_ms(),
        peer_name: session.peer.name.clone(),
        peer_id: session.peer.id.clone(),
        role: session.role,
        file_count: session.files.len(),
        display_name: session.display_name(),
        total_bytes: session.total_bytes,
        duration_ms,
        avg_speed_bps: session.meter.average_bps(),
        status,
        saved_paths: match session.role {
            Role::Receiving => session.saved_paths.clone(),
            Role::Sending => session.source_paths.clone(),
        },
        kind: session.kind,
        text: session.text.clone(),
    }
}

pub fn history_path(dir: &Path) -> PathBuf {
    dir.join("history.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_history() -> (History, PathBuf) {
        let dir = std::env::temp_dir().join(format!("sl-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = history_path(&dir);
        (History::load(path.clone()), dir)
    }

    fn sample(id: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            at_ms: now_ms(),
            peer_name: "Windows PC".into(),
            peer_id: "peer-1".into(),
            role: Role::Sending,
            file_count: 1,
            display_name: "movie.mp4".into(),
            total_bytes: 8_400_000_000,
            duration_ms: 97_000,
            avg_speed_bps: 86_000_000.0,
            status: SessionStatus::Completed,
            saved_paths: vec![],
            kind: crate::model::PayloadKind::Files,
            text: None,
        }
    }

    #[test]
    fn newest_entry_comes_first_and_survives_a_reload() {
        let (mut h, dir) = tmp_history();
        h.push(sample("a"));
        h.push(sample("b"));
        assert_eq!(h.entries()[0].id, "b");

        let reloaded = History::load(history_path(&dir));
        assert_eq!(reloaded.entries().len(), 2);
        assert_eq!(reloaded.entries()[0].id, "b");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_is_capped() {
        let (mut h, dir) = tmp_history();
        for i in 0..(MAX_ENTRIES + 25) {
            h.push(sample(&format!("e{i}")));
        }
        assert_eq!(h.entries().len(), MAX_ENTRIES);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entries_carry_no_file_contents() {
        let e = sample("a");
        let json = serde_json::to_string(&e).unwrap();
        // The schema is metadata-only; assert the shape so a future field
        // cannot quietly start persisting payload bytes.
        assert!(json.contains("total_bytes"));
        assert!(!json.contains("content"));
        assert!(!json.contains("data"));
    }

    #[test]
    fn remove_and_clear() {
        let (mut h, dir) = tmp_history();
        h.push(sample("a"));
        h.push(sample("b"));
        h.remove("a");
        assert_eq!(h.entries().len(), 1);
        h.clear();
        assert!(h.entries().is_empty());
        assert!(History::load(history_path(&dir)).entries().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreadable_history_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("sl-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = history_path(&dir);
        std::fs::write(&path, b"garbage").unwrap();
        assert!(History::load(path).entries().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
