//! Per-folder history for total-sync (mirror) folders. When a mirror folder
//! deletes or replaces a file, the old copy is moved here instead of being lost,
//! so it can be restored later. History lives in a hidden `.dropbeam-history`
//! dir INSIDE the folder — which the sync engine already skips (dotfiles), so it
//! never syncs and stays local to each device.
//!
//! Retention: saved copies are bounded by AGE (default 30 days), per-folder
//! SIZE (default 2 GiB), and a COUNT backstop (500) — oldest first. BUT the size
//! and count caps never evict a copy saved in the last 30 days: a bulk delete of
//! 3,000 photos must not silently make its own recovery copies vanish (D3). The
//! one exception is DISK SAFETY: when history alone grows past a hard limit, or
//! the disk is nearly full, the oldest copies move to the OS Trash (still
//! recoverable there) and the History view says so. This runs on every archive()
//! and via sweep_all() at startup + periodically.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::HistoryItem;
use crate::settings::write_atomic;

const HISTORY_DIR: &str = ".dropbeam-history";
/// Count backstop (never applied to copies younger than `PROTECT_MS`).
const MAX_ITEMS: usize = 500;
/// Copies saved this recently are never evicted by the size/count caps.
const PROTECT_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Disk-safety: history over this many bytes (or this many × the budget, if
/// larger) starts moving its OLDEST copies to the OS Trash. (Deliberately a size
/// limit, not a free-space one: the Trash lives on the same volume, so trashing
/// can't free space by itself — it hands the decision to the user.)
const HARD_LIMIT_FLOOR: u64 = 20 * 1024 * 1024 * 1024;
const HARD_LIMIT_BUDGET_MULT: u64 = 4;

fn hard_limit(policy: RetentionPolicy) -> u64 {
    policy
        .max_bytes
        .map(|b| b.saturating_mul(HARD_LIMIT_BUDGET_MULT).max(HARD_LIMIT_FLOOR))
        .unwrap_or(u64::MAX)
}

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
    load_checked(folder).unwrap_or_default()
}

/// The index for a READ-MODIFY-WRITE (D14). `None` = it can't be read right now
/// (an IO error — a NAS hiccup): the caller must NOT write, or it would orphan every
/// saved copy. A file that reads fine but isn't valid JSON (a torn write) won't
/// heal by waiting: `read_json_store` already set a `.corrupt-…` copy aside, and we
/// rebuild an index from the saved copies themselves so none is ever orphaned.
fn load_for_write(folder: &str) -> Option<Vec<HistoryItem>> {
    if let Some(items) = load_checked(folder) {
        return Some(items);
    }
    fs::read_to_string(index_path(folder)).ok()?;
    log::warn!("folder history index for {folder} is corrupt — rebuilding it from the saved copies");
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(data_dir(folder)) {
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let id = e.file_name().to_string_lossy().to_string();
            let ts = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or_else(now_ms);
            items.push(HistoryItem {
                rel_path: format!("Recovered/{id}"),
                id,
                size: meta.len(),
                reason: "recovered".into(),
                timestamp_ms: ts,
            });
        }
    }
    Some(items)
}

/// Hide the archive dir in Explorer (Windows has no dot-file convention).
fn hide_root(folder: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("attrib")
            .arg("+h")
            .arg(root(folder))
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(not(windows))]
    let _ = folder;
}

fn ensure_root(folder: &str) {
    let r = root(folder);
    if !r.is_dir() {
        let _ = fs::create_dir_all(&r);
        hide_root(folder);
    }
}

/// `None` when index.json exists but can't be read/parsed (a NAS hiccup, a torn
/// write). Callers that would REWRITE the index must skip then: saving the empty
/// fallback would orphan every saved copy (data files kept, index entries gone).
fn load_checked(folder: &str) -> Option<Vec<HistoryItem>> {
    match crate::settings::read_json_store(&index_path(folder)) {
        crate::settings::StoreRead::Loaded(items) => Some(items),
        crate::settings::StoreRead::Missing => Some(Vec::new()),
        crate::settings::StoreRead::Unreadable => None,
    }
}

fn save(folder: &str, items: &[HistoryItem]) {
    ensure_root(folder);
    // Compact JSON: this index is machine-read only and rewritten once per archived
    // file during a bulk delete — pretty-printing roughly doubled that IO.
    if let Ok(txt) = serde_json::to_string(items) {
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
    // Read the index BEFORE moving anything: if it can't be read right now, the
    // file stays where it is (the delete/replace simply doesn't happen yet)
    // rather than landing in an archive whose index we'd then clobber.
    let Some(mut items) = load_for_write(folder) else {
        log::warn!("folder history index for {folder} unreadable — not archiving {rel_path:?} (file kept in place)");
        return false;
    };
    let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let id = uuid::Uuid::new_v4().to_string();
    ensure_root(folder);
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
    let mut items = load_for_write(folder).ok_or("Couldn't read this folder's history right now — try again.")?;
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
    // Map the saved rel exactly like the folder-sync receive side does (D14): it
    // keeps the real name — a "Report 7:3.pdf" comes back as itself, not mangled
    // to "file" by the quick-send sanitizer — while still refusing traversal
    // (../, leading /) and DropBeam's own control paths.
    let rel = crate::iroh_net::folder_receive_rel(&item.rel_path)
        .ok_or("That file's saved name can't be restored here.")?;
    let mut dest = Path::new(folder).join(rel);
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
    let Some(mut items) = load_for_write(folder) else { return };
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
        let Some(mut items) = load_checked(folder) else {
            continue; // unreadable index — never overwrite it with []
        };
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
/// drop orphans → expire by age → cap by count → trim to size budget → disk
/// safety. The count/size caps NEVER evict a copy younger than `PROTECT_MS`.
fn prune_with(folder: &str, items: &mut Vec<HistoryItem>, policy: RetentionPolicy) {
    prune_with_disk(folder, items, policy, hard_limit(policy), &move_to_os_trash);
}

/// Disk-safety overflow notices: folder → (copies moved to the Trash, bytes, when).
fn overflow_notices() -> &'static Mutex<std::collections::HashMap<String, (u64, u64, u64)>> {
    static N: std::sync::OnceLock<Mutex<std::collections::HashMap<String, (u64, u64, u64)>>> =
        std::sync::OnceLock::new();
    N.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// How many recovery copies of this folder had to move to the OS Trash (disk
/// safety) since launch, their bytes, and when it last happened.
pub fn overflow_notice(folder: &str) -> Option<(u64, u64, u64)> {
    overflow_notices().lock().unwrap_or_else(|e| e.into_inner()).get(folder).copied()
}

/// Move one saved copy to the OS Trash under its ORIGINAL file name (not its
/// opaque id), so the user can find it there. Returns false if that isn't
/// possible (no Trash on this platform, or it failed) — the copy then stays.
fn move_to_os_trash(data: &Path, item: &HistoryItem) -> bool {
    #[cfg(desktop)]
    {
        let name = Path::new(&item.rel_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| item.id.clone());
        let Some(dir) = data.parent().map(|d| d.join(format!("trash-{}", item.id))) else {
            return false;
        };
        if fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let named = dir.join(&name);
        if fs::rename(data, &named).is_err() {
            let _ = fs::remove_dir(&dir);
            return false;
        }
        if trash::delete(&named).is_ok() && !named.exists() {
            let _ = fs::remove_dir(&dir);
            return true;
        }
        // Couldn't trash it: put it back, keep it.
        let _ = fs::rename(&named, data);
        let _ = fs::remove_dir(&dir);
        false
    }
    #[cfg(not(desktop))]
    {
        let _ = (data, item);
        false
    }
}

#[allow(clippy::type_complexity)]
fn prune_with_disk(
    folder: &str,
    items: &mut Vec<HistoryItem>,
    policy: RetentionPolicy,
    hard: u64,
    to_trash: &dyn Fn(&Path, &HistoryItem) -> bool,
) {
    let dir = data_dir(folder);
    // 1. Drop index entries whose data file is gone (no file to delete).
    items.retain(|i| dir.join(&i.id).is_file());
    // Oldest first, so front-of-vec eviction removes the oldest.
    items.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms));
    let now = now_ms();
    let protect_from = now.saturating_sub(PROTECT_MS);

    let remove_front = |items: &mut Vec<HistoryItem>| {
        let old = items.remove(0);
        let _ = fs::remove_file(dir.join(&old.id));
    };

    // 2. Age: expire everything older than the cutoff the user chose (can empty
    //    the folder). This is an explicit setting, so it applies to any copy.
    if let Some(max_age) = policy.max_age_ms {
        let cutoff = now.saturating_sub(max_age);
        while items.first().map(|i| i.timestamp_ms < cutoff).unwrap_or(false) {
            remove_front(items);
        }
    }

    // 3. Count backstop — only ever evicts copies OLDER than the protected window.
    while items.len() > policy.max_items
        && items.first().map(|i| i.timestamp_ms < protect_from).unwrap_or(false)
    {
        remove_front(items);
    }

    // 4. Size budget: evict oldest until under budget, but always keep at least
    // the newest one, and never a copy from the protected window (a bulk delete
    // must not evict its own recovery copies).
    let mut total: u64 = items.iter().map(|i| i.size).sum();
    if let Some(max_bytes) = policy.max_bytes {
        while total > max_bytes
            && items.len() > 1
            && items.first().map(|i| i.timestamp_ms < protect_from).unwrap_or(false)
        {
            let old = items.remove(0);
            total = total.saturating_sub(old.size);
            let _ = fs::remove_file(dir.join(&old.id));
        }
    }

    // 5. DISK SAFETY: history over its hard limit (recent copies piling up past
    // the budget) → move the oldest copies (any age) to the OS Trash — never
    // plain-delete them — and record a notice for the UI. Without a Trash
    // (iOS) nothing is removed.
    let mut moved = 0u64;
    let mut moved_bytes = 0u64;
    while items.len() > 1 && total > hard {
        let old = items[0].clone();
        if !to_trash(&dir.join(&old.id), &old) {
            break;
        }
        items.remove(0);
        total = total.saturating_sub(old.size);
        moved += 1;
        moved_bytes += old.size;
    }
    if moved > 0 {
        log::warn!(
            "folder history for {folder}: disk safety moved {moved} recovery cop{} ({moved_bytes} bytes) to the Trash",
            if moved == 1 { "y" } else { "ies" }
        );
        let mut n = overflow_notices().lock().unwrap_or_else(|e| e.into_inner());
        let e = n.entry(folder.to_string()).or_insert((0, 0, 0));
        e.0 += moved;
        e.1 += moved_bytes;
        e.2 = now;
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
        // Copies from the last 30 days are protected from the caps (D3) — age
        // these past the window so the budget applies.
        let old = now_ms() - PROTECT_MS - 60_000;
        for (i, it) in items.iter_mut().enumerate() {
            it.timestamp_ms = old + i as u64;
        }
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

    #[test]
    fn recent_copies_are_never_evicted_by_size_or_count_caps() {
        // D3: a bulk delete of many files must not evict its own recovery copies
        // because of the size budget or the 500-item backstop.
        let folder = tmp();
        set_policy(0, 0);
        for i in 0..6 {
            let abs = make_file(&folder, &format!("r{i}.bin"), 100);
            archive(&folder, &abs, &format!("r{i}.bin"), "deleted");
        }
        let mut items = load(&folder);
        let policy = RetentionPolicy { max_age_ms: None, max_bytes: Some(150), max_items: 2 };
        prune_with_disk(&folder, &mut items, policy, u64::MAX, &|_, _| false);
        assert_eq!(items.len(), 6, "every copy from the last 30 days survives the caps");
        assert_eq!(std::fs::read_dir(data_dir(&folder)).unwrap().count(), 6);
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn disk_safety_moves_oldest_copies_to_trash_never_deletes() {
        let folder = tmp();
        set_policy(0, 0);
        for i in 0..3 {
            let abs = make_file(&folder, &format!("d{i}.bin"), 100);
            archive(&folder, &abs, &format!("sub/d{i}.bin"), "deleted");
        }
        let mut items = load(&folder);
        let outside = tmp();
        let to_trash = |data: &Path, item: &HistoryItem| {
            // Stand-in for the OS Trash: the copy must MOVE (stay recoverable).
            std::fs::rename(data, Path::new(&outside).join(&item.id)).is_ok()
        };
        let policy = RetentionPolicy { max_age_ms: None, max_bytes: None, max_items: MAX_ITEMS };
        // History over its hard limit (here 150 bytes): oldest copies move out.
        prune_with_disk(&folder, &mut items, policy, 150, &to_trash);
        assert_eq!(items.len(), 1, "stops once back under the hard limit");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 2, "moved copies still exist (in the Trash)");
        assert_eq!(overflow_notice(&folder).map(|n| n.0), Some(2), "the UI is told");
        // Without a Trash, NOTHING is removed.
        let abs = make_file(&folder, "d9.bin", 100);
        archive(&folder, &abs, "d9.bin", "deleted");
        let mut items = load(&folder);
        let n = items.len();
        prune_with_disk(&folder, &mut items, policy, 1, &|_, _| false);
        assert_eq!(items.len(), n, "no Trash available = everything is kept");
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_index_is_never_clobbered_and_the_file_stays() {
        // D14: an index.json that can't be READ right now (IO error) must not be
        // replaced by a fresh one-item index — that orphaned every saved copy.
        let folder = tmp();
        set_policy(0, 0);
        let abs = make_file(&folder, "keep.bin", 10);
        archive(&folder, &abs, "keep.bin", "deleted");
        let before = std::fs::read(index_path(&folder)).unwrap();
        // Make the index unreadable: replace it with a directory (EISDIR on read).
        std::fs::remove_file(index_path(&folder)).unwrap();
        std::fs::create_dir(index_path(&folder)).unwrap();
        let abs2 = make_file(&folder, "second.bin", 10);
        assert!(!archive(&folder, &abs2, "second.bin", "deleted"), "refuses to archive blind");
        assert!(Path::new(&abs2).is_file(), "the file stays in place — nothing lost");
        forget(&folder, "anything");
        assert!(index_path(&folder).is_dir(), "forget didn't overwrite it either");
        // Put the real index back: still intact.
        std::fs::remove_dir(index_path(&folder)).unwrap();
        std::fs::write(index_path(&folder), &before).unwrap();
        assert_eq!(load(&folder).len(), 1);
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn corrupt_index_is_rebuilt_from_the_saved_copies() {
        let folder = tmp();
        set_policy(0, 0);
        for i in 0..2 {
            let abs = make_file(&folder, &format!("c{i}.bin"), 10);
            archive(&folder, &abs, &format!("c{i}.bin"), "deleted");
        }
        std::fs::write(index_path(&folder), b"{ torn wri").unwrap();
        let abs = make_file(&folder, "c2.bin", 10);
        assert!(archive(&folder, &abs, "c2.bin", "deleted"));
        let items = load(&folder);
        assert_eq!(items.len(), 3, "the two older copies are still listed (as recovered)");
        assert_eq!(items.iter().filter(|i| i.reason == "recovered").count(), 2);
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }

    #[cfg(not(windows))]
    #[test]
    fn restore_keeps_colons_and_hidden_names_but_never_escapes() {
        // D14: restore used the quick-send sanitizer, which drops ':' components —
        // "Report 7:3.pdf" came back as "file".
        let folder = tmp();
        set_policy(0, 0);
        let abs = make_file(&folder, "Report 7:3.pdf", 7);
        archive(&folder, &abs, "sub/Report 7:3.pdf", "deleted");
        let id = load(&folder)[0].id.clone();
        let restored = restore(&folder, &id).unwrap();
        assert!(restored.ends_with("sub/Report 7:3.pdf"), "{restored}");
        // A tampered index can't restore outside the folder or into the archive.
        let abs = make_file(&folder, "x.bin", 3);
        archive(&folder, &abs, "x.bin", "deleted");
        let mut items = load(&folder);
        items[0].rel_path = ".dropbeam-history/evil".into();
        save(&folder, &items);
        assert!(restore(&folder, &items[0].id).is_err());
        set_policy(30, 2 * 1024 * 1024 * 1024);
    }
}
