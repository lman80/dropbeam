//! Transfer history persistence (newest-first JSON list).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::models::HistoryEntry;
use crate::settings::write_atomic;

/// Serializes concurrent appends from multiple finishing transfers.
static LOCK: Mutex<()> = Mutex::new(());
const MAX_ENTRIES: usize = 300;

pub fn history_path(config_dir: &Path) -> PathBuf {
    config_dir.join("history.json")
}

pub fn load(config_dir: &Path) -> Vec<HistoryEntry> {
    match fs::read_to_string(history_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn append(config_dir: &Path, entry: HistoryEntry) {
    let _guard = LOCK.lock().unwrap();
    let mut items = load(config_dir);
    items.insert(0, entry);
    if items.len() > MAX_ENTRIES {
        items.truncate(MAX_ENTRIES);
    }
    let _ = fs::create_dir_all(config_dir);
    if let Ok(txt) = serde_json::to_string_pretty(&items) {
        let _ = write_atomic(&history_path(config_dir), txt.as_bytes());
    }
}

pub fn clear(config_dir: &Path) {
    let _guard = LOCK.lock().unwrap();
    let _ = fs::remove_file(history_path(config_dir));
}
