//! Settings persistence (a small JSON file in the app config dir).

use std::fs;
use std::path::{Path, PathBuf};

use crate::models::Settings;

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Load settings, filling in runtime-derived defaults (download dir, name) when
/// they're blank.
pub fn load(config_dir: &Path, default_download: &str, default_name: &str) -> Settings {
    let mut s = match fs::read_to_string(settings_path(config_dir)) {
        Ok(txt) => serde_json::from_str::<Settings>(&txt).unwrap_or_default(),
        Err(_) => Settings::default(),
    };
    if s.download_dir.trim().is_empty() {
        s.download_dir = default_download.to_string();
    }
    if s.display_name.trim().is_empty() {
        s.display_name = default_name.to_string();
    }
    s
}

pub fn save(config_dir: &Path, settings: &Settings) -> Result<(), String> {
    let _ = fs::create_dir_all(config_dir);
    let txt = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(config_dir), txt.as_bytes()).map_err(|e| e.to_string())
}

/// Read a JSON-array file (friends.json / pairs.json) resiliently.
///
/// The plain `read_to_string` + `unwrap_or_default` path was a data-loss vector: a
/// TRANSIENT read failure (Windows AV / Search-Indexer / OneDrive holding the file,
/// or a partial read) returned `[]`, and a subsequent add-then-save then wrote a
/// SHRUNKEN list (e.g. 5 friends → push 1 → save 1) that the empty-clobber guard
/// can't catch because 1 record isn't empty. That is the recurring "I lost my
/// contact after an update" report. So:
///   1. Retry a transient read a few times with a short backoff (rides out the lock).
///   2. `NotFound` is a genuine "no file yet" → empty list (not an error).
///   3. A clean, well-formed array (even `[]`) is authoritative — return it as-is so
///      a legitimate "removed my last friend/folder" still reads empty.
///   4. Only if the primary is corrupt/partial or unreadable after retries do we
///      recover from the last-known-good `.bak` sibling that `write_atomic_with_backup`
///      keeps. The caller still element-wise deserializes (one bad record drops only
///      itself), so this returns the raw `Vec<Value>`.
pub fn read_json_array_resilient(path: &Path) -> Vec<serde_json::Value> {
    for attempt in 0..4u32 {
        match fs::read_to_string(path) {
            Ok(txt) => {
                if let Ok(vals) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) {
                    return vals; // clean array (possibly legitimately empty)
                }
                break; // present but corrupt/partial → try the backup
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(_) => {
                // Transient (lock / IO). Brief backoff, then retry.
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt as u64 + 1)));
            }
        }
    }
    let bak = path.with_extension("bak");
    if let Ok(txt) = fs::read_to_string(&bak) {
        if let Ok(vals) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) {
            if !vals.is_empty() {
                log::warn!(
                    "read_json_array_resilient: recovered {} record(s) from {:?} after primary read failed/corrupt",
                    vals.len(),
                    bak
                );
                return vals;
            }
        }
    }
    Vec::new()
}

/// Atomically write `bytes` to `path`, and — when `keep_backup` is true — mirror it to
/// a `.bak` sibling as a last-known-good copy that `read_json_array_resilient` falls
/// back to. A failed backup write never fails the real save. Callers pass
/// `keep_backup = !list.is_empty() || legit_empty`: a glitch-empty never reaches here
/// (the empty-clobber guard blocks it first), so any empty write IS legitimate (the
/// user removed their last record) and the `.bak` should track it — otherwise a stale
/// `.bak` could resurrect a deliberately-removed contact/folder after a later corrupt
/// primary read.
pub fn write_atomic_with_backup(path: &Path, bytes: &[u8], keep_backup: bool) -> std::io::Result<()> {
    write_atomic(path, bytes)?;
    let bak = path.with_extension("bak");
    if keep_backup {
        // If the backup write fails (transient Windows AV/indexer/OneDrive handle
        // contention — the same fault this feature exists to survive), DELETE the old
        // .bak rather than leave it stale. A stale .bak that predates a removal could
        // otherwise resurrect a deliberately-deleted friend/folder on a later corrupt
        // primary read. Losing recoverability for that window is acceptable;
        // resurrecting removed data is not.
        if write_atomic(&bak, bytes).is_err() {
            let _ = fs::remove_file(&bak);
        }
    } else {
        // Not tracking a backup for this write — ensure no stale .bak survives to
        // resurrect data later.
        let _ = fs::remove_file(&bak);
    }
    Ok(())
}

/// Write a file by writing to a temp sibling then renaming, so a crash mid-write
/// never corrupts the real file.
///
/// On Windows the rename-over-existing transiently fails (ERROR_SHARING_VIOLATION /
/// ACCESS_DENIED) whenever antivirus, the Search Indexer, or OneDrive momentarily
/// holds the target open — unlike POSIX, the replace can't proceed over an open
/// handle. Left unhandled, that means a saved setting (verbose logging, diagnostics
/// URL, friends, chat, …) is silently lost. So: use a UNIQUE tmp sibling (so a stale
/// or locked tmp from a prior failed write can't block us), retry the rename a few
/// times with a short backoff to ride out the transient handle, and as a last resort
/// write in place so the value is never silently dropped. On macOS/Linux the very
/// first rename succeeds, so behavior is byte-identical to before (no sleeps, no
/// fallback).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    fs::write(&tmp, bytes)?;
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..5u32 {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(25 * (attempt as u64 + 1)));
            }
        }
    }
    // Atomic swap couldn't complete (Windows handle contention). Persist in place as a
    // best-effort floor so the value isn't silently lost, then drop the tmp.
    let direct = fs::write(path, bytes);
    let _ = fs::remove_file(&tmp);
    direct.map_err(|e| last_err.unwrap_or(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dropbeam-settings-test-{tag}-{}", std::process::id()))
    }

    #[test]
    fn resilient_read_returns_clean_and_legit_empty_without_backup() {
        let dir = tmp("clean");
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("list.json");
        write_atomic_with_backup(&p, b"[1,2,3]", true).unwrap();
        assert_eq!(read_json_array_resilient(&p).len(), 3);
        // A genuinely-empty, well-formed array returns [] (not the stale .bak).
        write_atomic_with_backup(&p, b"[]", true).unwrap();
        assert_eq!(read_json_array_resilient(&p).len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resilient_read_recovers_corrupt_primary_from_backup() {
        let dir = tmp("recover");
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("list.json");
        write_atomic_with_backup(&p, b"[10,20]", true).unwrap();
        fs::write(&p, b"[10,").unwrap(); // truncated/corrupt primary
        assert_eq!(read_json_array_resilient(&p).len(), 2, "recovered from .bak");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_backup_false_purges_stale_bak_so_no_resurrection() {
        // The resurrection guard: a write not tracking a backup must purge any existing
        // .bak, so a later corrupt primary read can't recover stale (removed) data.
        let dir = tmp("purge");
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("list.json");
        let bak = p.with_extension("bak");
        write_atomic_with_backup(&p, b"[1]", true).unwrap();
        assert!(bak.exists(), ".bak created");
        write_atomic_with_backup(&p, b"[2]", false).unwrap();
        assert!(!bak.exists(), "stale .bak purged when not tracking a backup");
        fs::write(&p, b"nonsense").unwrap(); // corrupt primary
        assert_eq!(read_json_array_resilient(&p).len(), 0, "no resurrection");
        let _ = fs::remove_dir_all(&dir);
    }
}
