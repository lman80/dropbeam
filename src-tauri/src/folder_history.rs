//! Per-folder history for total-sync (mirror) folders. When a mirror folder
//! deletes or replaces a file, the old copy is moved here instead of being lost,
//! so it can be restored later. History lives in a hidden `.dropbeam-history`
//! dir INSIDE the folder — which the sync engine already skips (dotfiles), so it
//! never syncs and stays local to each device.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::HistoryItem;

const HISTORY_DIR: &str = ".dropbeam-history";
const MAX_ITEMS: usize = 500;

fn root(folder: &str) -> PathBuf {
    Path::new(folder).join(HISTORY_DIR)
}

fn data_dir(folder: &str) -> PathBuf {
    root(folder).join("data")
}

fn index_path(folder: &str) -> PathBuf {
    root(folder).join("index.json")
}

pub fn load(folder: &str) -> Vec<HistoryItem> {
    fs::read_to_string(index_path(folder))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(folder: &str, items: &[HistoryItem]) {
    let _ = fs::create_dir_all(root(folder));
    if let Ok(txt) = serde_json::to_string_pretty(items) {
        let _ = fs::write(index_path(folder), txt);
    }
}

/// Move `abs_path` (a file about to be deleted or overwritten) into history.
/// `rel_path` is its path relative to the folder; `reason` is "deleted" or
/// "replaced". Returns true if it was archived.
pub fn archive(folder: &str, abs_path: &str, rel_path: &str, reason: &str) -> bool {
    let src = Path::new(abs_path);
    if !src.is_file() {
        return false;
    }
    let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let id = uuid::Uuid::new_v4().to_string();
    let _ = fs::create_dir_all(data_dir(folder));
    let dest = data_dir(folder).join(&id);
    let ok = fs::rename(src, &dest).is_ok()
        || (fs::copy(src, &dest).is_ok() && {
            let _ = fs::remove_file(src);
            true
        });
    if !ok {
        return false;
    }
    let mut items = load(folder);
    items.push(HistoryItem {
        id,
        rel_path: rel_path.to_string(),
        size,
        reason: reason.to_string(),
        timestamp_ms: now_ms(),
    });
    prune(folder, &mut items);
    save(folder, &items);
    true
}

/// Restore a history entry back into the folder. Returns the restored absolute
/// path. The file re-appears in the folder, so a mirror re-syncs it to the peer.
pub fn restore(folder: &str, id: &str) -> Result<String, String> {
    let mut items = load(folder);
    let pos = items
        .iter()
        .position(|i| i.id == id)
        .ok_or("That history item no longer exists.")?;
    let item = items[pos].clone();
    let data = data_dir(folder).join(&item.id);
    if !data.is_file() {
        // Index entry without its data — drop it.
        items.remove(pos);
        save(folder, &items);
        return Err("The saved copy of that file is missing.".into());
    }
    let mut dest = Path::new(folder).join(&item.rel_path);
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Don't clobber a file that's there now — restore alongside it.
    dest = unique_dest(dest);
    let dest_str = dest.to_string_lossy().to_string();
    let ok = fs::rename(&data, &dest).is_ok()
        || (fs::copy(&data, &dest).is_ok() && {
            let _ = fs::remove_file(&data);
            true
        });
    if !ok {
        return Err("Couldn't write the restored file.".into());
    }
    items.remove(pos);
    save(folder, &items);
    Ok(dest_str)
}

/// Permanently forget a history entry (and its stored bytes).
pub fn forget(folder: &str, id: &str) {
    let mut items = load(folder);
    if let Some(pos) = items.iter().position(|i| i.id == id) {
        let _ = fs::remove_file(data_dir(folder).join(&items[pos].id));
        items.remove(pos);
        save(folder, &items);
    }
}

fn prune(folder: &str, items: &mut Vec<HistoryItem>) {
    while items.len() > MAX_ITEMS {
        let old = items.remove(0);
        let _ = fs::remove_file(data_dir(folder).join(&old.id));
    }
}

fn unique_dest(dest: PathBuf) -> PathBuf {
    if !dest.exists() {
        return dest;
    }
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = dest.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} (restored {n}).{e}"),
            None => format!("{stem} (restored {n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
