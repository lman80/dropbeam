//! Which directories may hold resumable partials or receive stages, so the
//! startup sweep and "Clear transfer cache" can find abandoned ones (audit T14).
//!
//! It used to be one JSON list re-read and re-written for EVERY received file,
//! growing by every subfolder of every folder ever received — never pruned.
//! Now:
//! * it lives in memory (loaded once) and is saved debounced, off the async
//!   threads, only when it actually changes;
//! * resumable partials register their receive ROOT (bounded, aged out);
//! * a receive stage's directory is listed only WHILE a stage is live there
//!   (so a crash mid-receive still leaves it listed for the sweep), and is
//!   dropped again once its last stage is published or removed.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const MAX_ROOTS: usize = 64;
const MAX_AGE_MS: u64 = 30 * 24 * 3600 * 1000;
const FILE: &str = "partial-dirs.v2.json";
const LEGACY: &str = "partial-dirs.json";

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct Root { path: PathBuf, at: u64 }

#[derive(Default)]
struct Registry {
    config: Option<PathBuf>,
    loaded: bool,
    roots: Vec<Root>,
    live: HashMap<PathBuf, usize>,
    dirty: bool,
    saving: bool,
}

static REG: std::sync::LazyLock<Mutex<Registry>> = std::sync::LazyLock::new(Default::default);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn lock() -> std::sync::MutexGuard<'static, Registry> { REG.lock().unwrap_or_else(|p| p.into_inner()) }

/// Where the registry is persisted (the app config dir). Set once at startup;
/// the first caller wins (one engine per process).
pub(super) fn init(config: &Path) {
    let mut r = lock();
    if r.config.is_none() {
        r.config = Some(config.to_path_buf());
        r.loaded = false;
    }
}

fn load(r: &mut Registry) {
    if r.loaded { return; }
    r.loaded = true;
    let Some(config) = r.config.clone() else { return };
    let now = now_ms();
    let mut roots: Vec<Root> = std::fs::read(config.join(FILE)).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if roots.is_empty() {
        // One-time import of the old unbounded list (newest first).
        let legacy: Vec<PathBuf> = std::fs::read(config.join(LEGACY)).ok()
            .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        roots = legacy.into_iter().take(MAX_ROOTS).map(|path| Root { path, at: now }).collect();
        if !roots.is_empty() { r.dirty = true; }
    }
    for root in roots {
        if !r.roots.iter().any(|x| x.path == root.path) { r.roots.push(root); }
    }
    prune(&mut r.roots, now);
}

fn prune(roots: &mut Vec<Root>, now: u64) {
    roots.retain(|r| now.saturating_sub(r.at) < MAX_AGE_MS);
    roots.truncate(MAX_ROOTS);
}

fn schedule_save(r: &mut Registry) {
    r.dirty = true;
    if r.saving || r.config.is_none() { return; }
    r.saving = true;
    let _ = std::thread::Builder::new().name("partial-dirs-save".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        save_now();
    });
}

fn snapshot(r: &Registry) -> Vec<Root> {
    let mut out = r.roots.clone();
    for dir in r.live.keys() {
        if !out.iter().any(|x| &x.path == dir) { out.push(Root { path: dir.clone(), at: now_ms() }); }
    }
    out
}

fn save_now() {
    let (config, list) = {
        let mut r = lock();
        r.saving = false;
        if !r.dirty { return; }
        r.dirty = false;
        let Some(config) = r.config.clone() else { return };
        (config, snapshot(&r))
    };
    let Ok(json) = serde_json::to_vec(&list) else { return };
    let path = config.join(FILE);
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() { let _ = std::fs::rename(&tmp, &path); }
    // The legacy file is superseded; keep old builds from re-growing a stale one.
    let _ = std::fs::remove_file(config.join(LEGACY));
}

/// A resumable partial (or published file) lives under `dir`.
pub(super) fn note_root(dir: &Path) {
    let mut r = lock();
    load(&mut r);
    let now = now_ms();
    if let Some(first) = r.roots.first_mut() {
        if first.path == dir {
            // Refresh the age without a write unless it is getting old.
            if now.saturating_sub(first.at) > 24 * 3600 * 1000 { first.at = now; schedule_save(&mut r); }
            return;
        }
    }
    r.roots.retain(|x| x.path != dir);
    r.roots.insert(0, Root { path: dir.to_path_buf(), at: now });
    prune(&mut r.roots, now);
    schedule_save(&mut r);
}

/// A receive stage was created in `dir`.
pub(super) fn stage_enter(dir: &Path) {
    let mut r = lock();
    load(&mut r);
    let known = r.roots.iter().any(|x| x.path == dir);
    let count = r.live.entry(dir.to_path_buf()).or_insert(0);
    *count += 1;
    if *count == 1 && !known { schedule_save(&mut r); }
}

/// A receive stage in `dir` ended. `left_behind`: its file is still on disk
/// (cleanup deferred) — keep the dir listed as a root so the sweep finds it.
pub(super) fn stage_leave(dir: &Path, left_behind: bool) {
    let mut r = lock();
    let gone = match r.live.get_mut(dir) {
        Some(count) => { *count = count.saturating_sub(1); *count == 0 }
        None => false,
    };
    if gone { r.live.remove(dir); }
    drop(r);
    if left_behind { note_root(dir); } else if gone {
        let mut r = lock();
        if !r.roots.iter().any(|x| x.path == dir) { schedule_save(&mut r); }
    }
}

/// Every directory to sweep: registered roots plus dirs with live stages.
pub(super) fn all() -> Vec<PathBuf> {
    let mut r = lock();
    load(&mut r);
    snapshot(&r).into_iter().map(|x| x.path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t14_roots_are_bounded_and_aged() {
        let now = now_ms();
        let mut roots: Vec<Root> = (0..100).map(|i| Root { path: format!("/r{i}").into(), at: now }).collect();
        roots.push(Root { path: "/old".into(), at: now - MAX_AGE_MS - 1 });
        prune(&mut roots, now);
        assert_eq!(roots.len(), MAX_ROOTS);
        assert!(!roots.iter().any(|r| r.path == Path::new("/old")));
    }
}
