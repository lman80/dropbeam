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
    // Parse element-wise so one corrupt entry drops only itself instead of
    // wiping the whole transfer history (plain from_str → unwrap_or_default
    // would Err on a single bad element and lose them all). A locked or torn
    // file is retried / kept aside rather than read as empty and overwritten.
    match crate::settings::read_json_store::<Vec<serde_json::Value>>(&history_path(config_dir)) {
        crate::settings::StoreRead::Loaded(vals) => {
            vals.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
        }
        _ => Vec::new(),
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

/// Native iOS timeline removal uses the same lock and atomic persistence as append.
#[cfg(target_os = "ios")]
#[tauri::command]
pub fn remove_history_entry(state: tauri::State<'_, std::sync::Arc<crate::AppState>>, id: String) -> Result<(), String> {
    let _guard = LOCK.lock().map_err(|e| e.to_string())?;
    let mut items = load(&state.config_dir);
    items.retain(|entry| entry.id != id);
    let text = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
    write_atomic(&history_path(&state.config_dir), text.as_bytes()).map_err(|e| e.to_string())
}
