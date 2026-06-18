//! Per-folder history for total-sync (mirror) folders. When a mirror folder
//! deletes or replaces a file, the old copy is moved here instead of being lost,
//! so it can be restored later. History lives in a hidden `.dropbeam-history`
//! dir INSIDE the folder — which the sync engine already skips (dotfiles), so it
//! never syncs and stays local to each device.
//!
//! Retention: saved copies are bounded by AGE (default 30 days), per-folder
//! SIZE (default 2 GiB), and a hard COUNT backstop (500). The oldest copies are
//! evicted first. This runs on every archive() and via sweep_all() at startup +
//! periodically, so the history can never grow unbounded (the old failure where
//! 500 multi-GB copies ate "tons of storage").

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::HistoryItem;
use crate::settings::write_atomic;

const HISTORY_DIR: &str = ".dropbeam-history";
/// Hard backstop on item count regardless of size/age policy.
const MAX_ITEMS: usize = 500;

/// How saved copies are bounded. `None` = that limit is off.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub max_age_ms: Option<u64>,
    pub max_bytes: Option<u64>,
    pub max_items: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy {
            max_age_ms: Some(30 * 24 * 60 * 60 * 1000), // 30 days
            max_bytes: Some(2 * 1024 * 1024 * 1024),    // 2 GiB
            max_items: MAX_ITEMS,
        }
    }
}

/// The live policy, set from Settings at startup and whenever the user changes
/// retention. archive() reads it so every new save is pruned to current rules.
static POLICY: Mutex<RetentionPolicy> = Mutex::new(RetentionPolicy {
    max_age_ms: Some(30 * 24 * 60 * 60 * 1000),
    max_bytes: Some(2 * 1024 * 1024 * 1024),
    max_items: MAX_ITEMS,
});

/// Serializes every index.json read-modify-write (and the data-dir moves that go
/// with it) so concurrent writers can't clobber each other. The mutators are:
/// archive() on the sync thread, sweep_all() on background threads (startup +
/// retention change), and restore()/forget()/clear_all() on command threads.
/// Without this, a just-archived recovery copy could be dropped from the index
/// by a concurrent sweep — the copy would survive on disk but be unrecoverable.
/// All fs here is fast & local, so a single coarse lock is fine. Recovered from
/// poison so one panicking op can't wedge all of history. Lock ORDER is always
/// IO_LOCK → POLICY (POLICY is only ever read into a local), so no deadlock.
static IO_LOCK: Mutex<()> = Mutex::new(());

fn io_guard() -> MutexGuard<'static, ()> {
    IO_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Update the live retention policy. `keep_days == 0` = keep forever;
/// `budget_bytes == 0` = no size limit.
pub fn set_policy(keep_days: u32, budget_bytes: u64) {
    let policy = RetentionPolicy {
        max_age_ms: if keep_days == 0 {
            None
        } else {
            Some(keep_days as u64 * 24 * 60 * 60 * 1000)
        },
        max_bytes: if budget_bytes == 0 {
            None
        } else {
            Some(budget_bytes)
        },
        max_items: MAX_ITEMS,
    };
    if let Ok(mut p) = POLICY.lock() {
        *p = policy;
    }
}

fn current_policy() -> RetentionPolicy {
    POLICY.lock().map(|p| *p).unwrap_or_default()
}

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
        // Atomic (write temp + rename) so a concurrent reader never sees a
        // half-written, unparseable index.json.
        let _ = write_atomic(&index_path(folder), txt.as_bytes());
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
    // Hold the lock across the rename + index update so a concurrent sweep can't
    // observe the data file on disk but miss it in the index (which would orphan
    // the just-archived copy). Read the policy into a local first (IO_LOCK→POLICY).
    let policy = current_policy();
    let _guard = io_guard();
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
    prune_with(folder, &mut items, policy);
    save(folder, &items);
    true
}

/// Restore a history entry back into the folder. Returns the restored absolute
/// path. The file re-appears in the folder, so a mirror re-syncs it to the peer.
pub fn restore(folder: &str, id: &str) -> Result<String, String> {
    let _guard = io_guard();
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
    // Sanitize before joining so a tampered index rel_path (../, leading /, etc.)
    // can never restore OUTSIDE the folder root — same invariant the receive side
    // enforces on incoming paths.
    let mut dest = Path::new(folder).join(crate::iroh_net::sanitize_rel(&item.rel_path));
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
    let _guard = io_guard();
    let mut items = load(folder);
    if let Some(pos) = items.iter().position(|i| i.id == id) {
        let _ = fs::remove_file(data_dir(folder).join(&items[pos].id));
        items.remove(pos);
        save(folder, &items);
    }
}

/// Total bytes the saved copies occupy (summed from the index — cheap). Orphan
/// entries (data file gone) are excluded so the figure can't over-report.
pub fn folder_size(folder: &str) -> u64 {
    let dir = data_dir(folder);
    load(folder)
        .iter()
        .filter(|i| dir.join(&i.id).is_file())
        .map(|i| i.size)
        .sum()
}

/// Wipe a folder's entire recovery history (every saved copy + the index).
/// Returns the bytes freed. Only ever touches files under this folder's
/// `.dropbeam-history` dir.
pub fn clear_all(folder: &str) -> u64 {
    let _guard = io_guard();
    let items = load(folder);
    let freed: u64 = items
        .iter()
        .filter(|i| data_dir(folder).join(&i.id).is_file())
        .map(|i| i.size)
        .sum();
    // remove_dir_all is bounded to data_dir(folder) = <folder>/.dropbeam-history/data.
    let _ = fs::remove_dir_all(data_dir(folder));
    let _ = fs::remove_file(index_path(folder));
    freed
}

/// Apply the current retention policy to every folder. Run at startup and
/// periodically so age-based expiry happens even for idle folders. Returns the
/// total bytes freed across all folders.
pub fn sweep_all(folders: &[String]) -> u64 {
    let policy = current_policy();
    let mut freed = 0u64;
    // Dedup folder paths — a group folder is reached by several pair links but is
    // one archive on disk.
    let mut seen: Vec<String> = Vec::new();
    for folder in folders {
        if seen.iter().any(|f| f == folder) {
            continue;
        }
        seen.push(folder.clone());
        if !root(folder).is_dir() {
            continue;
        }
        // Per-folder lock (not held across all folders) so a long folder list
        // can't stall an archive()/restore() on an unrelated folder for long.
        let _guard = io_guard();
        let mut items = load(folder);
        let before: u64 = items.iter().map(|i| i.size).sum();
        prune_with(folder, &mut items, policy);
        let after: u64 = items.iter().map(|i| i.size).sum();
        freed += before.saturating_sub(after);
        save(folder, &items);
    }
    freed
}

/// Evict saved copies down to the retention policy, oldest first, deleting each
/// evicted data file. Mutates `items` in place to the surviving set. Order:
/// drop orphans → expire by age → cap by count → trim to size budget.
fn prune_with(folder: &str, items: &mut Vec<HistoryItem>, policy: RetentionPolicy) {
    let dir = data_dir(folder);
    // 1. Drop index entries whose data file is gone (no file to delete).
    items.retain(|i| dir.join(&i.id).is_file());
    // Oldest first, so front-of-vec eviction removes the oldest.
    items.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms));

    let remove_front = |items: &mut Vec<HistoryItem>| {
        let old = items.remove(0);
        let _ = fs::remove_file(dir.join(&old.id));
    };

    // 2. Age: expire everything older than the cutoff (can empty the folder).
    if let Some(max_age) = policy.max_age_ms {
        let now = now_ms();
        let cutoff = now.saturating_sub(max_age);
        while items.first().map(|i| i.timestamp_ms < cutoff).unwrap_or(false) {
            remove_front(items);
        }
    }

    // 3. Count backstop.
    while items.len() > policy.max_items {
        remove_front(items);
    }

    // 4. Size budget: evict oldest until under budget, but always keep at least
    // the newest one (a single copy larger than the budget is still recoverable
    // until something newer replaces it).
    if let Some(max_bytes) = policy.max_bytes {
        let mut total: u64 = items.iter().map(|i| i.size).sum();
        while total > max_bytes && items.len() > 1 {
            let old = items.remove(0);
            total = total.saturating_sub(old.size);
            let _ = fs::remove_file(dir.join(&old.id));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("db-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().to_string()
    }

    fn make_file(folder: &str, name: &str, bytes: usize) -> String {
        let p = Path::new(folder).join(name);
        std::fs::write(&p, vec![0u8; bytes]).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn evicts_oldest_by_size_budget_but_keeps_newest() {
        let folder = tmp();
        set_policy(0, 0); // no limits for archiving
        // Archive 3 files of 100 bytes each.
        for i in 0..3 {
            let abs = make_file(&folder, &format!("f{i}.bin"), 100);
            archive(&folder, &abs, &format!("f{i}.bin"), "deleted");
        }
        let mut items = load(&folder);
        // Budget of 150 bytes → keep only the newest (1 item), since each is 100.
        let policy = RetentionPolicy {
            max_age_ms: None,
            max_bytes: Some(150),
            max_items: MAX_ITEMS,
        };
        prune_with(&folder, &mut items, policy);
        assert_eq!(items.len(), 1, "size budget keeps at least the newest");
        // The kept item's data file still exists; the evicted ones are gone.
        let dir = data_dir(&folder);
        let alive = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(alive, 1);
        set_policy(30, 2 * 1024 * 1024 * 1024); // restore default for other tests
    }

    #[test]
    fn expires_by_age_can_empty() {
        let folder = tmp();
        set_policy(0, 0);
        let abs = make_file(&folder, "old.bin", 10);
        archive(&folder, &abs, "old.bin", "deleted");
        let mut items = load(&folder);
        // Backdate the single item far into the past.
        items[0].timestamp_ms = 1;
        save(&folder, &items);
        let mut items = load(&folder);
        let policy = RetentionPolicy {
            max_age_ms: Some(1000), // 1s
            max_bytes: None,
            max_items: MAX_ITEMS,
        };
        prune_with(&folder, &mut items, policy);
        assert_eq!(items.len(), 0, "age expiry can remove the last item");
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn prune_drops_orphan_index_entries() {
        let folder = tmp();
        set_policy(0, 0);
        let abs = make_file(&folder, "a.bin", 10);
        archive(&folder, &abs, "a.bin", "deleted");
        // Delete the data file out from under the index.
        let items = load(&folder);
        std::fs::remove_file(data_dir(&folder).join(&items[0].id)).unwrap();
        let mut items = load(&folder);
        prune_with(&folder, &mut items, RetentionPolicy::default());
        assert_eq!(items.len(), 0, "orphan entry dropped");
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn clear_all_empties_only_the_archive() {
        let folder = tmp();
        set_policy(0, 0);
        // A real file in the folder must survive clear_all.
        make_file(&folder, "live.txt", 5);
        let abs = make_file(&folder, "gone.bin", 100);
        archive(&folder, &abs, "gone.bin", "deleted");
        assert_eq!(load(&folder).len(), 1);
        let freed = clear_all(&folder);
        assert_eq!(freed, 100);
        assert_eq!(load(&folder).len(), 0);
        assert!(Path::new(&folder).join("live.txt").is_file(), "live file untouched");
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn folder_size_sums_live_copies() {
        let folder = tmp();
        set_policy(0, 0);
        for i in 0..2 {
            let abs = make_file(&folder, &format!("s{i}.bin"), 250);
            archive(&folder, &abs, &format!("s{i}.bin"), "replaced");
        }
        assert_eq!(folder_size(&folder), 500);
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn concurrent_archives_and_sweep_lose_no_items() {
        // The IO_LOCK must serialize index.json writes: archiving N files
        // concurrently (the rapid-multi-delete pattern) while a sweep runs in
        // parallel must end with all N recovery copies recorded, not clobbered.
        let folder = tmp();
        set_policy(0, 0); // no eviction so the count is a clean invariant
        const N: usize = 24;
        let mut handles = Vec::new();
        for i in 0..N {
            let f = folder.clone();
            handles.push(std::thread::spawn(move || {
                let p = Path::new(&f).join(format!("c{i}.bin"));
                std::fs::write(&p, vec![1u8; 64]).unwrap();
                archive(&f, &p.to_string_lossy(), &format!("c{i}.bin"), "deleted");
            }));
        }
        // A concurrent sweeper hammering the same index.
        for _ in 0..4 {
            let f = folder.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..20 {
                    sweep_all(&[f.clone()]);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let items = load(&folder);
        assert_eq!(items.len(), N, "no archived recovery copy was lost to a race");
        // Every indexed item still has its data file (no dangling index rows).
        let dir = data_dir(&folder);
        for it in &items {
            assert!(dir.join(&it.id).is_file(), "indexed item missing its data file");
        }
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }
}
