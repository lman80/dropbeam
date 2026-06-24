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
