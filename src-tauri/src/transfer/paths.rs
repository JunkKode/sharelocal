//! Turning a peer-supplied relative path into a safe local path.
//!
//! Everything a remote device sends is hostile until proven otherwise. This
//! module is the single choke point: no other code may join a network-supplied
//! string onto a filesystem path.

use crate::model::{ConflictPolicy, TransferError};
use std::path::{Component, Path, PathBuf};

/// Names Windows refuses to create, checked on every platform so a folder
/// transferred from macOS to Windows cannot produce an unopenable tree.
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

const MAX_COMPONENT_LEN: usize = 200;
const MAX_DEPTH: usize = 32;
const MAX_TOTAL_LEN: usize = 1_024;

fn invalid(msg: impl Into<String>) -> TransferError {
    TransferError::InvalidPath(msg.into())
}

/// Sanitise one path component. Returns `None` for components that should be
/// dropped entirely (`.`), and an error for anything hostile.
fn clean_component(raw: &str) -> Result<Option<String>, TransferError> {
    if raw.is_empty() || raw == "." {
        return Ok(None);
    }
    if raw == ".." {
        return Err(invalid("path escapes the download folder"));
    }
    if raw.len() > MAX_COMPONENT_LEN {
        return Err(invalid("a name in the path is too long"));
    }
    if raw
        .chars()
        .any(|c| c == '\0' || (c.is_control() && c != '\t'))
    {
        return Err(invalid("path contains control characters"));
    }
    // Reject the separators and Windows-illegal characters outright rather
    // than silently rewriting them, so the receiver never invents a filename
    // the sender did not mean.
    if raw
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(invalid(
            "path contains characters that are not allowed in a file name",
        ));
    }

    // Windows silently strips trailing dots and spaces, which would let
    // `evil.` and `evil` collide. Normalise instead.
    let trimmed = raw.trim_end_matches([' ', '.']);
    let trimmed = if trimmed.is_empty() {
        raw.trim()
    } else {
        trimmed
    };
    if trimmed.is_empty() {
        return Err(invalid("path contains an empty name"));
    }

    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        return Ok(Some(format!("_{trimmed}")));
    }
    Ok(Some(trimmed.to_string()))
}

/// Convert a peer-supplied `rel_path` into a relative `PathBuf` that is safe to
/// join onto the download root.
///
/// Rejects: absolute paths, UNC and drive-letter prefixes, `..` traversal,
/// NUL/control bytes, over-deep or over-long paths, and empty results.
pub fn sanitize_rel_path(input: &str) -> Result<PathBuf, TransferError> {
    if input.trim().is_empty() {
        return Err(invalid("empty file name"));
    }
    if input.len() > MAX_TOTAL_LEN {
        return Err(invalid("path is too long"));
    }
    if input.contains('\0') {
        return Err(invalid("path contains a NUL byte"));
    }
    // Normalise Windows separators before inspecting, so `..\\..\\x` is caught.
    let normalised = input.replace('\\', "/");
    if normalised.starts_with('/') {
        return Err(invalid("absolute paths are not accepted"));
    }
    // `C:/x`, `C:x` and UNC `//host/share`.
    let first = normalised.split('/').next().unwrap_or("");
    if first.len() >= 2 && first.as_bytes()[1] == b':' {
        return Err(invalid("drive-qualified paths are not accepted"));
    }
    if normalised.starts_with("//") {
        return Err(invalid("network paths are not accepted"));
    }

    let mut parts: Vec<String> = Vec::new();
    for raw in normalised.split('/') {
        if let Some(clean) = clean_component(raw)? {
            parts.push(clean);
        }
    }
    if parts.is_empty() {
        return Err(invalid("path resolves to nothing"));
    }
    if parts.len() > MAX_DEPTH {
        return Err(invalid("folder nesting is too deep"));
    }

    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    Ok(out)
}

/// Join a sanitised relative path onto `root` and verify the result is still
/// inside `root` even after symlink resolution of the existing prefix.
///
/// This is belt-and-braces on top of [`sanitize_rel_path`]: if the download
/// folder itself contains a symlink planted by something else, we still refuse
/// to write outside it.
pub fn resolve_dest(root: &Path, rel: &Path) -> Result<PathBuf, TransferError> {
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            _ => return Err(invalid("unexpected path component")),
        }
    }
    let candidate = root.join(rel);

    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Canonicalise the deepest existing ancestor; the tail does not exist yet.
    let mut existing = candidate.as_path();
    let real_existing = loop {
        match existing.canonicalize() {
            Ok(p) => break p,
            Err(_) => match existing.parent() {
                Some(parent) => existing = parent,
                None => return Err(invalid("cannot resolve destination")),
            },
        }
    };
    if !real_existing.starts_with(&real_root) {
        return Err(invalid("path escapes the download folder"));
    }
    Ok(candidate)
}

/// `report.pdf` → `report (2).pdf` → `report (3).pdf`, skipping names already
/// taken. Returns the original path when it is free.
pub fn unique_path(dest: &Path) -> PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = dest.extension().map(|s| s.to_string_lossy().to_string());

    for n in 2..10_000u32 {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological directory; fall back to something unambiguous.
    let unique = uuid::Uuid::new_v4();
    match &ext {
        Some(ext) => parent.join(format!("{stem}-{unique}.{ext}")),
        None => parent.join(format!("{stem}-{unique}")),
    }
}

/// Decide the final destination for an incoming file given the session's
/// conflict policy. `Ok(None)` means "the user chose to cancel this file".
pub fn apply_conflict_policy(
    dest: &Path,
    policy: ConflictPolicy,
) -> Result<Option<PathBuf>, TransferError> {
    if !dest.exists() {
        return Ok(Some(dest.to_path_buf()));
    }
    match policy {
        ConflictPolicy::Replace => Ok(Some(dest.to_path_buf())),
        ConflictPolicy::KeepBoth => Ok(Some(unique_path(dest))),
        ConflictPolicy::Cancel => Ok(None),
    }
}

/// Where the in-progress `.part` file for a session/file pair lives. Kept in a
/// hidden sibling folder so partial data never looks like a finished download.
pub fn part_path(root: &Path, session_id: &str, index: usize) -> PathBuf {
    let safe_session: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    root.join(crate::config::PART_DIR)
        .join(format!("{safe_session}-{index}.part"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names_and_folders() {
        assert_eq!(
            sanitize_rel_path("movie.mp4").unwrap(),
            PathBuf::from("movie.mp4")
        );
        assert_eq!(
            sanitize_rel_path("Trip/2024/IMG_0001.HEIC").unwrap(),
            PathBuf::from("Trip").join("2024").join("IMG_0001.HEIC")
        );
        // Unicode, spaces and dots inside the name all survive intact.
        assert_eq!(
            sanitize_rel_path("Ünïcode note v1.2.txt").unwrap(),
            PathBuf::from("Ünïcode note v1.2.txt")
        );
    }

    #[test]
    fn rejects_directory_traversal() {
        for evil in [
            "../etc/passwd",
            "..",
            "a/../../b",
            "a/b/../../../../c",
            "..\\..\\windows\\system32\\cmd.exe",
            "docs/../../.ssh/id_rsa",
        ] {
            assert!(
                sanitize_rel_path(evil).is_err(),
                "traversal not rejected: {evil}"
            );
        }
    }

    #[test]
    fn rejects_absolute_and_drive_qualified_paths() {
        for evil in [
            "/etc/passwd",
            "/",
            "C:/Windows/system32/config",
            "C:file.txt",
            "\\\\server\\share\\x",
            "//server/share/x",
        ] {
            assert!(
                sanitize_rel_path(evil).is_err(),
                "absolute not rejected: {evil}"
            );
        }
    }

    #[test]
    fn rejects_control_bytes_and_separators_in_names() {
        assert!(sanitize_rel_path("a\0b").is_err());
        assert!(sanitize_rel_path("a\nb.txt").is_err());
        assert!(sanitize_rel_path("re:port.txt").is_err());
        assert!(sanitize_rel_path("what?.txt").is_err());
        assert!(sanitize_rel_path("pipe|name").is_err());
    }

    #[test]
    fn rejects_oversized_paths() {
        let long_component = "x".repeat(300);
        assert!(sanitize_rel_path(&long_component).is_err());

        let deep = (0..40).map(|_| "d").collect::<Vec<_>>().join("/") + "/f.txt";
        assert!(sanitize_rel_path(&deep).is_err());

        let huge = "a/".repeat(600) + "f.txt";
        assert!(sanitize_rel_path(&huge).is_err());
    }

    #[test]
    fn drops_redundant_components() {
        assert_eq!(
            sanitize_rel_path("./a/./b.txt").unwrap(),
            PathBuf::from("a").join("b.txt")
        );
        assert_eq!(
            sanitize_rel_path("a//b.txt").unwrap(),
            PathBuf::from("a").join("b.txt")
        );
    }

    #[test]
    fn neutralises_windows_reserved_names() {
        assert_eq!(sanitize_rel_path("NUL").unwrap(), PathBuf::from("_NUL"));
        assert_eq!(
            sanitize_rel_path("com1.txt").unwrap(),
            PathBuf::from("_com1.txt")
        );
        // A name that merely starts with a reserved word is untouched.
        assert_eq!(
            sanitize_rel_path("console.log").unwrap(),
            PathBuf::from("console.log")
        );
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(
            sanitize_rel_path("report.txt.").unwrap(),
            PathBuf::from("report.txt")
        );
        assert_eq!(
            sanitize_rel_path("report.txt  ").unwrap(),
            PathBuf::from("report.txt")
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert!(sanitize_rel_path("").is_err());
        assert!(sanitize_rel_path("   ").is_err());
        assert!(sanitize_rel_path("./.").is_err());
    }

    #[test]
    fn resolve_dest_keeps_files_inside_root() {
        let root = std::env::temp_dir().join(format!("sl-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("sub")).unwrap();

        let ok = resolve_dest(&root, Path::new("sub/file.bin")).unwrap();
        assert!(ok.starts_with(&root));

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dest_refuses_symlink_escape() {
        let base = std::env::temp_dir().join(format!("sl-sym-{}", uuid::Uuid::new_v4()));
        let root = base.join("downloads");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        // The relative path itself is perfectly innocent...
        let rel = sanitize_rel_path("escape/loot.txt").unwrap();
        // ...but resolving it lands outside the root, so it must be refused.
        assert!(resolve_dest(&root, &rel).is_err());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn unique_path_keeps_both_copies() {
        let dir = std::env::temp_dir().join(format!("sl-uniq-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("photo.zip");

        assert_eq!(unique_path(&target), target, "free name is used as-is");

        std::fs::write(&target, b"x").unwrap();
        let second = unique_path(&target);
        assert_eq!(second.file_name().unwrap(), "photo (2).zip");

        std::fs::write(&second, b"x").unwrap();
        let third = unique_path(&target);
        assert_eq!(third.file_name().unwrap(), "photo (3).zip");

        // Extension-less files get the counter appended too.
        let noext = dir.join("Makefile");
        std::fs::write(&noext, b"x").unwrap();
        assert_eq!(unique_path(&noext).file_name().unwrap(), "Makefile (2)");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_policies_behave() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.txt");
        std::fs::write(&target, b"old").unwrap();

        assert_eq!(
            apply_conflict_policy(&target, ConflictPolicy::Replace).unwrap(),
            Some(target.clone())
        );
        assert_eq!(
            apply_conflict_policy(&target, ConflictPolicy::KeepBoth)
                .unwrap()
                .unwrap()
                .file_name()
                .unwrap(),
            "a (2).txt"
        );
        assert_eq!(
            apply_conflict_policy(&target, ConflictPolicy::Cancel).unwrap(),
            None
        );

        let free = dir.join("b.txt");
        assert_eq!(
            apply_conflict_policy(&free, ConflictPolicy::Cancel).unwrap(),
            Some(free),
            "no conflict means the policy never applies"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn part_path_sanitises_session_id() {
        let p = part_path(Path::new("/tmp/dl"), "../../evil", 0);
        assert_eq!(
            p,
            Path::new("/tmp/dl")
                .join(crate::config::PART_DIR)
                .join("evil-0.part")
        );
    }
}
