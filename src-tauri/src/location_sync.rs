//! Synced folders: a folder on THIS device that keeps a friend's Location up to
//! date, one way.
//!
//! "Everything I put in this folder goes to the NAS." Nothing else: a file the
//! user deletes here stays on the NAS unless they explicitly ask otherwise, and
//! nothing ever comes back down (that's what Shared Drop Folders are for).
//!
//! The transport is the ordinary Location upload — batching, per-file resume,
//! hidden files, verification and the host-side skip of files that already
//! landed all come for free. This module is only the decision layer: WHEN to
//! upload, WHICH paths, and what the user is told while it happens.
//!
//! Per enabled folder one task runs:
//!   * a full reconcile on start/enable and every 30 minutes (the host's
//!     `locations.stat` makes unchanged files free, so this is cheap),
//!   * a filesystem watcher whose paths settle for 5 s AND stop growing before
//!     they're uploaded (a file still being copied in is not sent yet),
//!   * exactly ONE upload at a time — changes that arrive mid-upload are
//!     remembered and swept up in one more pass when it finishes,
//!   * backoff (1 / 5 / 15 / 30 min) while the host is unreachable, reported as
//!     "Waiting for <friend>" rather than as an error.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::Notify;

/// Live state of one synced folder, pushed to the UI.
pub const STATUS_EVENT: &str = "location-sync://status";
/// How long a touched path must stay quiet (and the same size) before it's sent.
pub const DEBOUNCE_MS: u64 = 5_000;
/// Belt-and-braces full re-check, so a missed filesystem event can't strand a file.
pub const RECONCILE_EVERY: Duration = Duration::from_secs(30 * 60);
/// Retry schedule while the friend hosting the location is unreachable.
pub const BACKOFF_SECS: [u64; 4] = [60, 300, 900, 1800];
/// Give up waiting on a transfer that has emitted nothing at all for this long.
const TRANSFER_SILENCE: Duration = Duration::from_secs(900);
const MAX_FOLDERS: usize = 50;

fn yes() -> bool { true }
fn now_ms() -> u64 { crate::chat::now_ms() }

// ── Model ────────────────────────────────────────────────────────────────────

/// What the last check did, in words the UI can show as-is.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LastResult {
    pub ok: bool,
    #[serde(default)]
    pub message: String,
}

/// One local folder kept in step with one folder inside one friend's Location.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncedFolder {
    pub id: String,
    pub friend_id: String,
    pub location_id: String,
    /// Destination folder INSIDE the location. "" puts the files at its root.
    #[serde(default)]
    pub rel_path: String,
    pub local_path: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Off by default: deleting a file here must never delete the NAS copy
    /// unless the user asked for that.
    #[serde(default)]
    pub delete_remote: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub last_check_at: u64,
    #[serde(default)]
    pub last_result: Option<LastResult>,
}

impl SyncedFolder {
    /// Everything a running task depends on. A change here restarts the task;
    /// a change to anything else (timestamps, last result) does not.
    fn signature(&self) -> String {
        format!("{}|{}|{}|{}|{}", self.friend_id, self.location_id, self.rel_path, self.local_path, self.delete_remote)
    }
    /// Where this folder's files land inside the location.
    fn dest(&self, rel: &str) -> String {
        format!("{}/{}", self.rel_path, rel).trim_matches('/').replace("//", "/")
    }
    fn name(&self) -> String {
        Path::new(&self.local_path).file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.local_path.clone())
    }
}

/// What the folder is doing right now. `state` is one of
/// idle | scanning | uploading | waiting | paused | error.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub id: String,
    pub state: String,
    pub pending_files: u64,
    pub last_check_at: u64,
    pub message: String,
    pub transfer_id: Option<String>,
}

impl SyncStatus {
    fn new(id: &str, state: &str, message: &str) -> Self {
        Self { id: id.into(), state: state.into(), pending_files: 0, last_check_at: 0, message: message.into(), transfer_id: None }
    }
}

// ── Persistence (atomic, like locations.json) ────────────────────────────────

pub fn config_path(config: &Path) -> PathBuf { config.join("location-sync.json") }

/// Parsed element-wise so one forward-incompatible record drops only itself.
pub fn load(config: &Path) -> Vec<SyncedFolder> {
    crate::settings::read_json_array_resilient(&config_path(config))
        .into_iter()
        .filter_map(|v| serde_json::from_value::<SyncedFolder>(v).ok())
        .collect()
}

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub fn save_all(config: &Path, list: &[SyncedFolder]) -> Result<()> {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::fs::create_dir_all(config)?;
    let tmp = config.join(format!(".location-sync-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(list)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, config_path(config))?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

/// Read-modify-write one record under the same lock the writer uses.
fn update_record(config: &Path, id: &str, edit: impl FnOnce(&mut SyncedFolder)) -> Result<Vec<SyncedFolder>> {
    let mut list = load(config);
    let folder = list.iter_mut().find(|f| f.id == id).context("That synced folder is no longer set up")?;
    edit(folder);
    save_all(config, &list)?;
    Ok(list)
}

// ── Pure decision helpers (unit-tested) ──────────────────────────────────────

/// A path the watcher touched, with the size we last saw it at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPath {
    pub size: u64,
    /// When we last saw it change (event, or a different size).
    pub since_ms: u64,
}

/// A touched path is ready to send once it has been quiet for the debounce AND
/// has stopped growing — a 4 GB video still being copied in is not uploaded
/// half-written.
pub fn settled(entry: &PendingPath, size_now: u64, now_ms: u64) -> bool {
    entry.size == size_now && now_ms.saturating_sub(entry.since_ms) >= DEBOUNCE_MS
}

/// How long to wait before the Nth consecutive failed attempt is retried.
/// 1 min, 5 min, 15 min, then every 30 min.
pub fn backoff_delay(attempt: u32) -> Duration {
    let index = (attempt.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[index])
}

/// The top-level children of `root` that cover every changed path. Uploading
/// those (rather than each file) keeps every file's name relative to the folder,
/// so `sub/b.jpg` lands in `sub/`, and the host's stat-skip makes the untouched
/// files inside them free.
pub fn plan_upload_paths(root: &Path, changed: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in changed {
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let Some(first) = rel.components().next() else { continue };
        let child = root.join(first.as_os_str());
        if !out.contains(&child) { out.push(child); }
    }
    out.sort();
    out
}

/// Whether the folder's worker is between uploads, in one, or in one that newer
/// changes have already outdated. The engine NEVER starts a second concurrent
/// upload; `finished` says whether one more sweep is owed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunState {
    #[default]
    Idle,
    Running,
    RunningDirty,
}

impl RunState {
    /// A filesystem change arrived. While idle the change simply queues; while
    /// an upload is running it marks the run dirty.
    pub fn changed(self) -> Self {
        match self { Self::Idle => Self::Idle, _ => Self::RunningDirty }
    }
    pub fn started(self) -> Self { Self::Running }
    /// An upload finished. `true` = run once more right away.
    pub fn finished(self) -> (Self, bool) { (Self::Idle, self == Self::RunningDirty) }
    pub fn busy(self) -> bool { self != Self::Idle }
}

/// Everything waiting to be looked at for one folder.
#[derive(Debug, Default)]
struct Pending {
    /// A full reconcile is owed (start, "Sync now", the 30-minute timer).
    full: bool,
    paths: HashMap<PathBuf, PendingPath>,
    /// Paths the watcher saw disappear, with when we first noticed.
    deletes: HashMap<PathBuf, u64>,
    run: RunState,
}

// ── Engine ───────────────────────────────────────────────────────────────────

struct Handle {
    signature: String,
    stopped: Arc<AtomicBool>,
    wake: Arc<Notify>,
    pending: Arc<Mutex<Pending>>,
    status: Arc<Mutex<SyncStatus>>,
    _watcher: Option<notify::RecommendedWatcher>,
}

pub struct Engine {
    app: AppHandle,
    config_dir: PathBuf,
    iroh: Arc<crate::iroh_net::IrohState>,
    folders: Mutex<HashMap<String, Handle>>,
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// Bring the synced-folder engine up. Safe to call once, at iroh startup.
pub fn start(app: AppHandle, config_dir: PathBuf, iroh: Arc<crate::iroh_net::IrohState>) {
    let engine = Arc::new(Engine { app, config_dir, iroh, folders: Mutex::new(HashMap::new()) });
    if ENGINE.set(engine.clone()).is_err() { return; }
    // The first reconcile waits a beat: iroh needs to bind before a friend is
    // reachable, and a failed first dial would only start the backoff clock.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        engine.reload();
    });
}

pub fn engine() -> Option<Arc<Engine>> { ENGINE.get().cloned() }

/// Apply the on-disk config: start tasks for enabled folders, stop the rest.
pub fn reload() {
    if let Some(engine) = engine() { engine.reload(); }
}

/// Current live status for every configured folder (unknown ones read "Paused"
/// or "Waiting", never a blank card).
pub fn statuses() -> HashMap<String, SyncStatus> {
    engine().map(|e| e.folders.lock().unwrap().iter()
        .map(|(id, h)| (id.clone(), h.status.lock().unwrap().clone())).collect())
        .unwrap_or_default()
}

/// Ask one folder to do a full check right now (also clears any backoff wait).
pub fn sync_now(id: &str) -> Result<()> {
    let engine = engine().context("DropBeam is still starting up — try again in a moment")?;
    let folders = engine.folders.lock().unwrap();
    let handle = folders.get(id).context("That folder isn't syncing right now — turn it back on first")?;
    handle.pending.lock().unwrap().full = true;
    handle.wake.notify_waiters();
    Ok(())
}

impl Engine {
    fn emit(&self, status: &SyncStatus) { let _ = self.app.emit(STATUS_EVENT, status); }

    /// Record and publish what a folder is doing. A live upload ticks about once
    /// a second; nothing is emitted unless something the user can SEE changed.
    fn set_status(&self, status: &Arc<Mutex<SyncStatus>>, state: &str, message: &str, pending: u64, transfer: Option<String>) {
        let snapshot = {
            let mut s = status.lock().unwrap();
            let before = s.clone();
            s.state = state.into();
            s.message = message.into();
            s.pending_files = pending;
            s.transfer_id = transfer;
            if matches!(state, "idle" | "error") { s.last_check_at = now_ms(); }
            if *s == before { return; }
            s.clone()
        };
        self.emit(&snapshot);
    }

    pub fn reload(self: &Arc<Self>) {
        let wanted = load(&self.config_dir);
        let mut folders = self.folders.lock().unwrap();
        // Stop tasks for folders that are gone, paused, or reconfigured.
        folders.retain(|id, handle| {
            let live = wanted.iter().find(|f| &f.id == id)
                .is_some_and(|f| f.enabled && f.signature() == handle.signature);
            if !live {
                if handle.pending.lock().unwrap().run.busy() {
                    log::info!("synced folder {id} changed mid-upload — that upload finishes on its own");
                }
                handle.stopped.store(true, Ordering::SeqCst);
                handle.wake.notify_waiters();
            }
            live
        });
        // Anything configured but not running is either paused (report it) or
        // needs a task.
        for folder in wanted.iter() {
            if !folder.enabled {
                let mut status = SyncStatus::new(&folder.id, "paused", "Paused — nothing is being copied");
                status.last_check_at = folder.last_check_at;
                self.emit(&status);
                continue;
            }
            if folders.contains_key(&folder.id) { continue; }
            if let Some(handle) = self.clone().spawn_folder(folder.clone()) {
                folders.insert(folder.id.clone(), handle);
            }
        }
    }

    fn spawn_folder(self: Arc<Self>, folder: SyncedFolder) -> Option<Handle> {
        let root = PathBuf::from(&folder.local_path);
        let stopped = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let pending = Arc::new(Mutex::new(Pending { full: true, ..Default::default() }));
        let mut status = SyncStatus::new(&folder.id, "scanning", "Checking this folder…");
        status.last_check_at = folder.last_check_at;
        let status = Arc::new(Mutex::new(status));

        // Watcher → pending. Like the Shared Drop Folder watcher, we do NOT trust
        // the event KIND: macOS reports "move to Trash" as a rename, so a path is
        // classified by whether it still exists once it has settled.
        let watcher = {
            let pending = pending.clone();
            let wake = wake.clone();
            let root_c = root.clone();
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                use notify::EventKind;
                if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) { return; }
                let now = now_ms();
                let mut p = pending.lock().unwrap();
                for path in event.paths {
                    if path == root_c || !path.starts_with(&root_c) { continue; }
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let entry = p.paths.entry(path).or_insert(PendingPath { size, since_ms: now });
                    if entry.size != size { entry.size = size; }
                    entry.since_ms = now;
                }
                p.run = p.run.changed();
                drop(p);
                wake.notify_waiters();
            }) {
                Ok(mut w) => {
                    use notify::Watcher;
                    match w.watch(&root, notify::RecursiveMode::Recursive) {
                        Ok(()) => Some(w),
                        Err(e) => { log::warn!("synced folder watch failed for {}: {e}", folder.id); None }
                    }
                }
                Err(e) => { log::warn!("synced folder watcher init failed: {e}"); None }
            }
        };

        let handle = Handle {
            signature: folder.signature(),
            stopped: stopped.clone(),
            wake: wake.clone(),
            pending: pending.clone(),
            status: status.clone(),
            _watcher: watcher,
        };
        let engine = self.clone();
        tauri::async_runtime::spawn(async move {
            engine.run_folder(folder.id.clone(), root, stopped, wake, pending, status).await;
        });
        Some(handle)
    }

    /// The worker: one upload at a time, forever, until the folder is removed.
    async fn run_folder(
        self: Arc<Self>,
        id: String,
        root: PathBuf,
        stopped: Arc<AtomicBool>,
        wake: Arc<Notify>,
        pending: Arc<Mutex<Pending>>,
        status: Arc<Mutex<SyncStatus>>,
    ) {
        let mut last_full = 0u64;
        let mut failures = 0u32;
        let mut retry_at = 0u64;
        while !stopped.load(Ordering::SeqCst) {
            let now = now_ms();
            let due_full = pending.lock().unwrap().full
                || now.saturating_sub(last_full) >= RECONCILE_EVERY.as_millis() as u64;
            let ready = take_ready(&pending, now);
            let deletes = take_ready_deletes(&pending, &root, now);
            let waiting_for_retry = now < retry_at;
            if waiting_for_retry || (!due_full && ready.is_empty() && deletes.is_empty()) {
                // Put back what we're not acting on yet.
                if waiting_for_retry { requeue(&pending, ready, deletes); }
                let nap = nap_for(&pending, now, last_full, retry_at);
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(nap) => {}
                }
                continue;
            }
            { let mut p = pending.lock().unwrap(); p.run = p.run.started(); }
            if due_full { pending.lock().unwrap().full = false; last_full = now; }

            let Some(folder) = load(&self.config_dir).into_iter().find(|f| f.id == id) else { break };
            let outcome = self.run_once(&folder, &status, due_full, ready, deletes).await;
            match &outcome {
                Ok(message) => {
                    failures = 0;
                    retry_at = 0;
                    let _ = update_record(&self.config_dir, &id, |f| {
                        f.last_check_at = now_ms();
                        f.last_result = Some(LastResult { ok: true, message: message.clone() });
                    });
                }
                Err(problem) => {
                    failures += 1;
                    let delay = backoff_delay(failures);
                    retry_at = now_ms() + delay.as_millis() as u64;
                    // An unreachable host is NOT an error the user has to act on;
                    // it's a wait. Everything else reads as a problem.
                    let waiting = problem.waiting;
                    self.set_status(&status, if waiting { "waiting" } else { "error" }, &problem.message, 0, None);
                    let _ = update_record(&self.config_dir, &id, |f| {
                        f.last_check_at = now_ms();
                        f.last_result = Some(LastResult { ok: false, message: problem.message.clone() });
                    });
                    // A failed pass is owed a full re-check, not just its changes.
                    pending.lock().unwrap().full = true;
                }
            }
            let again = { let mut p = pending.lock().unwrap(); let (next, again) = p.run.finished(); p.run = next; again };
            if again { wake.notify_waiters(); }
        }
        log::info!("synced folder {id} stopped");
    }

    /// One pass: check the host is there, push deletes (if opted in), then
    /// upload. Returns the plain-language summary for the folder's last result.
    async fn run_once(
        &self,
        folder: &SyncedFolder,
        status: &Arc<Mutex<SyncStatus>>,
        full: bool,
        ready: Vec<PathBuf>,
        deletes: Vec<PathBuf>,
    ) -> std::result::Result<String, Problem> {
        let root = PathBuf::from(&folder.local_path);
        let friend = crate::friends::get(&self.config_dir, &folder.friend_id)
            .ok_or_else(|| Problem::error("That device is no longer one of your friends"))?;
        let who = friend.name.clone();
        if !root.is_dir() {
            return Err(Problem::error(&format!("Can't find the folder “{}” on this Mac", folder.name())));
        }
        self.set_status(status, "scanning", &format!("Checking what's new in “{}”", folder.name()), 0, None);
        let endpoint = friend.endpoint_id.clone()
            .ok_or_else(|| Problem::waiting(&format!("Waiting for {who}")))?;

        // Reachability + access, before anything moves. Both failures read as a
        // wait: a NAS host that's asleep is normal, not broken.
        let shared = crate::iroh_net::location_request(&self.iroh, &endpoint, json!({"kind": "locations.list"}))
            .await
            .map_err(|_| Problem::waiting(&format!("Waiting for {who}")))?;
        let entry = shared.as_array().into_iter().flatten()
            .find(|l| l["id"] == folder.location_id.as_str());
        let Some(entry) = entry else {
            return Err(Problem::error(&format!("{who} isn't sharing that folder any more")));
        };
        if entry["rights"]["upload"] != true {
            return Err(Problem::error(&format!("{who} turned off uploads for that folder")));
        }
        let place = entry["name"].as_str().unwrap_or("that folder").to_string();

        // Deletes are opt-in and never touch bytes we didn't put there: the host
        // moves its copy to the location's own trash, where it can be recovered.
        let mut trashed = 0usize;
        if folder.delete_remote {
            for path in &deletes {
                let Ok(rel) = path.strip_prefix(&root) else { continue };
                let dest = folder.dest(&rel.to_string_lossy().replace('\\', "/"));
                if dest.is_empty() { continue; }
                match crate::iroh_net::location_request(&self.iroh, &endpoint,
                    json!({"kind": "locations.trash", "id": folder.location_id, "rel_path": dest})).await {
                    Ok(_) => trashed += 1,
                    // A file that was never there, or a location without manage
                    // rights, must not stall the folder — say so and carry on.
                    Err(e) => log::info!("synced folder {} could not remove {dest} on the host: {e:#}", folder.id),
                }
            }
        }

        let paths = if full {
            children(&root).map_err(|e| Problem::error(&format!("Can't read “{}” — {e}", folder.name())))?
        } else {
            plan_upload_paths(&root, &ready)
        };
        let paths: Vec<String> = paths.iter().filter(|p| p.exists()).map(|p| p.to_string_lossy().into_owned()).collect();
        if paths.is_empty() {
            let message = if trashed > 0 {
                format!("Removed {trashed} item{} from {place}", if trashed == 1 { "" } else { "s" })
            } else { "Up to date".into() };
            self.set_status(status, "idle", &message, 0, None);
            return Ok(message);
        }

        self.set_status(status, "uploading", &format!("Copying to {place}…"), 0, None);
        let target = crate::locations::Target { location_id: folder.location_id.clone(), rel_path: folder.rel_path.clone() };
        let app_state = self.app.try_state::<Arc<crate::AppState>>()
            .ok_or_else(|| Problem::error("DropBeam is still starting up"))?.inner().clone();
        let started = crate::commands::start_location_upload_replacing(
            self.app.clone(), app_state, self.iroh.clone(), folder.friend_id.clone(), target, paths, true,
        ).await.map_err(|e| classify(&e, &who))?;

        let transfer_id = started.id.clone();
        self.set_status(status, "uploading", &format!("Copying to {place}…"), 0, Some(transfer_id.clone()));
        let finish = self.await_transfer(&transfer_id, |left| {
            self.set_status(status, "uploading",
                &if left == 0 { format!("Copying to {place}…") } else { format!("Copying {left} file{} to {place}…", if left == 1 { "" } else { "s" }) },
                left, Some(transfer_id.clone()));
        }).await;

        match finish.as_deref() {
            Some("completed") => {
                let message = if trashed > 0 {
                    format!("Up to date · removed {trashed} item{} from {place}", if trashed == 1 { "" } else { "s" })
                } else { "Up to date".into() };
                self.set_status(status, "idle", &message, 0, None);
                Ok(message)
            }
            Some("paused") | Some("canceled") => {
                let message = "Stopped — it will pick up where it left off".to_string();
                self.set_status(status, "idle", &message, 0, None);
                Ok(message)
            }
            _ => Err(Problem::waiting(&format!("Waiting for {who}"))),
        }
    }

    /// Follow one upload's card to its end. `progress` reports how many files of
    /// the batch still have to move (the host skips the ones already there).
    async fn await_transfer(&self, transfer_id: &str, progress: impl Fn(u64)) -> Option<String> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, u64)>();
        let want = transfer_id.to_string();
        let listener = self.app.listen("transfer://update", move |event| {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) else { return };
            if value["id"].as_str() != Some(want.as_str()) { return }
            let state = value["state"].as_str().unwrap_or("").to_string();
            let count = value["fileCount"].as_u64().unwrap_or(0);
            let skipped = value["locationSkipped"].as_u64().unwrap_or(0);
            let _ = tx.send((state, count.saturating_sub(skipped)));
        });
        let mut outcome = None;
        loop {
            match tokio::time::timeout(TRANSFER_SILENCE, rx.recv()).await {
                Ok(Some((state, left))) => {
                    if matches!(state.as_str(), "completed" | "failed" | "canceled" | "paused") {
                        outcome = Some(state);
                        break;
                    }
                    progress(left);
                }
                // The card went quiet for a quarter of an hour: stop following it
                // (the transfer itself is untouched) and let backoff re-check.
                Ok(None) | Err(_) => break,
            }
        }
        self.app.unlisten(listener);
        outcome
    }
}

/// A failed pass, and whether it's the host being away (a wait) or something the
/// user has to fix (an error).
struct Problem { message: String, waiting: bool }
impl Problem {
    fn error(message: &str) -> Self { Self { message: message.into(), waiting: false } }
    fn waiting(message: &str) -> Self { Self { message: message.into(), waiting: true } }
}

/// Upload refusals that mean "not now" rather than "not ever".
fn classify(error: &str, who: &str) -> Problem {
    let lower = error.to_ascii_lowercase();
    if lower.contains("still connecting") || lower.contains("no device address")
        || lower.contains("already running") || lower.contains("timed out")
        || lower.contains("unreachable") || lower.contains("connection") {
        Problem::waiting(&format!("Waiting for {who}"))
    } else {
        Problem::error(error)
    }
}

/// Immediate children of a folder — hidden files included, nothing skipped.
fn children(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(root)? {
        out.push(entry?.path());
    }
    out.sort();
    Ok(out)
}

/// Pull out the touched paths that still exist and have settled.
fn take_ready(pending: &Arc<Mutex<Pending>>, now: u64) -> Vec<PathBuf> {
    let mut p = pending.lock().unwrap();
    let mut ready = Vec::new();
    let mut refresh: Vec<(PathBuf, u64)> = Vec::new();
    for (path, entry) in p.paths.iter() {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => { if settled(entry, entry.size, now) { ready.push(path.clone()); } }
            Ok(meta) => {
                if settled(entry, meta.len(), now) { ready.push(path.clone()); }
                else if meta.len() != entry.size { refresh.push((path.clone(), meta.len())); }
            }
            // Gone. It becomes a delete candidate once it has been gone a while —
            // a rename in place shows up as "missing" for a moment.
            Err(_) => {
                if now.saturating_sub(entry.since_ms) >= DEBOUNCE_MS { ready.push(path.clone()); }
            }
        }
    }
    for (path, size) in refresh {
        if let Some(entry) = p.paths.get_mut(&path) { entry.size = size; entry.since_ms = now; }
    }
    for path in &ready { p.paths.remove(path); }
    // The vanished ones move to the delete list; the rest are real uploads.
    let (gone, live): (Vec<_>, Vec<_>) = ready.into_iter().partition(|p| !p.exists());
    for path in gone { p.deletes.entry(path).or_insert(now); }
    live
}

/// Deletes only count once the path has been absent for a full debounce AND is
/// still absent now — a move within the folder must not trash the NAS copy.
fn take_ready_deletes(pending: &Arc<Mutex<Pending>>, root: &Path, now: u64) -> Vec<PathBuf> {
    let mut p = pending.lock().unwrap();
    let ripe: Vec<PathBuf> = p.deletes.iter()
        .filter(|(_, since)| now.saturating_sub(**since) >= DEBOUNCE_MS)
        .map(|(path, _)| path.clone()).collect();
    let mut out = Vec::new();
    for path in ripe {
        p.deletes.remove(&path);
        if !path.exists() && path.starts_with(root) { out.push(path); }
    }
    out
}

/// Put an untouched batch back (we're in a backoff wait, not a pass).
fn requeue(pending: &Arc<Mutex<Pending>>, paths: Vec<PathBuf>, deletes: Vec<PathBuf>) {
    let now = now_ms();
    let mut p = pending.lock().unwrap();
    for path in paths {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        p.paths.entry(path).or_insert(PendingPath { size, since_ms: now.saturating_sub(DEBOUNCE_MS) });
    }
    for path in deletes { p.deletes.entry(path).or_insert(now.saturating_sub(DEBOUNCE_MS)); }
}

/// How long the worker may sleep: a short poll while something is settling, a
/// backoff wait while the host is away, otherwise until the next full check.
fn nap_for(pending: &Arc<Mutex<Pending>>, now: u64, last_full: u64, retry_at: u64) -> Duration {
    let busy = { let p = pending.lock().unwrap(); !p.paths.is_empty() || !p.deletes.is_empty() };
    if retry_at > now { return Duration::from_millis((retry_at - now).min(60_000)); }
    if busy { return Duration::from_secs(1); }
    let since = now.saturating_sub(last_full);
    let left = RECONCILE_EVERY.as_millis() as u64;
    Duration::from_millis(left.saturating_sub(since).clamp(1_000, 60_000))
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_synced_folders(state: tauri::State<'_, Arc<crate::AppState>>) -> Vec<SyncedFolder> {
    load(&state.config_dir)
}

#[tauri::command]
pub fn synced_folder_statuses() -> HashMap<String, SyncStatus> { statuses() }

#[tauri::command]
pub fn add_synced_folder(
    state: tauri::State<'_, Arc<crate::AppState>>,
    friend_id: String,
    location_id: String,
    rel_path: String,
    local_path: String,
    delete_remote: bool,
) -> std::result::Result<Vec<SyncedFolder>, String> {
    (|| -> Result<Vec<SyncedFolder>> {
        let root = PathBuf::from(&local_path);
        ensure!(root.is_dir(), "Pick a folder on this computer");
        let root = std::fs::canonicalize(&root)?;
        let rel_path = crate::locations::relative(rel_path.trim())?.to_string_lossy().replace('\\', "/");
        ensure!(crate::friends::get(&state.config_dir, &friend_id).is_some(), "Friend not found");
        let mut list = load(&state.config_dir);
        ensure!(list.len() < MAX_FOLDERS, "That's as many synced folders as one device can keep up with");
        ensure!(!list.iter().any(|f| Path::new(&f.local_path) == root && f.location_id == location_id),
            "That folder is already being copied there");
        // Two synced folders that nest would each re-upload the other's files.
        ensure!(!list.iter().any(|f| root.starts_with(&f.local_path) || Path::new(&f.local_path).starts_with(&root)),
            "Another synced folder is already inside (or around) this one");
        list.push(SyncedFolder {
            id: uuid::Uuid::new_v4().to_string(),
            friend_id, location_id, rel_path,
            local_path: root.to_string_lossy().into_owned(),
            enabled: true, delete_remote,
            created_at: now_ms(), last_check_at: 0, last_result: None,
        });
        save_all(&state.config_dir, &list)?;
        reload();
        Ok(list)
    })().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn update_synced_folder(
    state: tauri::State<'_, Arc<crate::AppState>>,
    id: String,
    enabled: Option<bool>,
    delete_remote: Option<bool>,
) -> std::result::Result<Vec<SyncedFolder>, String> {
    let list = update_record(&state.config_dir, &id, |f| {
        if let Some(enabled) = enabled { f.enabled = enabled; }
        if let Some(delete_remote) = delete_remote { f.delete_remote = delete_remote; }
    }).map_err(|e| format!("{e:#}"))?;
    reload();
    Ok(list)
}

/// Stop copying. Files already on the NAS, and the local folder, are untouched.
#[tauri::command]
pub fn remove_synced_folder(
    state: tauri::State<'_, Arc<crate::AppState>>,
    id: String,
) -> std::result::Result<Vec<SyncedFolder>, String> {
    let mut list = load(&state.config_dir);
    list.retain(|f| f.id != id);
    save_all(&state.config_dir, &list).map_err(|e| format!("{e:#}"))?;
    reload();
    Ok(list)
}

#[tauri::command]
pub fn sync_folder_now(id: String) -> std::result::Result<(), String> {
    sync_now(&id).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dropbeam-locsync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backoff_walks_one_five_fifteen_thirty_then_holds() {
        assert_eq!(backoff_delay(1), Duration::from_secs(60));
        assert_eq!(backoff_delay(2), Duration::from_secs(300));
        assert_eq!(backoff_delay(3), Duration::from_secs(900));
        assert_eq!(backoff_delay(4), Duration::from_secs(1800));
        assert_eq!(backoff_delay(9), Duration::from_secs(1800));
        // Attempt 0 is meaningless; never wait less than the first step.
        assert_eq!(backoff_delay(0), Duration::from_secs(60));
    }

    #[test]
    fn a_growing_file_is_not_sent_yet() {
        let entry = PendingPath { size: 1_000, since_ms: 0 };
        // Quiet long enough, same size → go.
        assert!(settled(&entry, 1_000, DEBOUNCE_MS));
        // Quiet long enough but STILL growing → wait.
        assert!(!settled(&entry, 4_000, DEBOUNCE_MS * 10));
        // Same size but touched a moment ago → wait.
        assert!(!settled(&entry, 1_000, DEBOUNCE_MS - 1));
    }

    #[test]
    fn a_burst_of_changes_becomes_one_upload_per_branch() {
        let root = PathBuf::from("/Users/me/Travel");
        let changed = vec![
            root.join("sub/deep/a.jpg"),
            root.join("sub/b.jpg"),
            root.join("top.txt"),
            root.join("sub/deep/c.jpg"),
            PathBuf::from("/elsewhere/x.jpg"),
        ];
        let plan = plan_upload_paths(&root, &changed);
        assert_eq!(plan, vec![root.join("sub"), root.join("top.txt")]);
        // Nothing outside the folder ever gets planned.
        assert!(!plan.iter().any(|p| p.starts_with("/elsewhere")));
        assert!(plan_upload_paths(&root, &[root.clone()]).is_empty());
    }

    #[test]
    fn changes_during_an_upload_earn_exactly_one_more_pass() {
        let state = RunState::default();
        assert!(!state.busy());
        // A change while idle just queues — it does not mark a run dirty.
        assert_eq!(state.changed(), RunState::Idle);
        let running = state.started();
        assert!(running.busy());
        // Two changes mid-upload still owe ONE more pass, not two.
        let dirty = running.changed().changed();
        assert_eq!(dirty, RunState::RunningDirty);
        let (after, again) = dirty.finished();
        assert_eq!(after, RunState::Idle);
        assert!(again, "changes that arrived mid-upload must be swept up");
        // A clean run owes nothing.
        let (after, again) = RunState::Running.finished();
        assert_eq!(after, RunState::Idle);
        assert!(!again);
    }

    #[test]
    fn config_survives_a_save_load_round_trip() {
        let dir = tmp();
        assert!(load(&dir).is_empty());
        let folder = SyncedFolder {
            id: "abc".into(), friend_id: "f1".into(), location_id: "loc1".into(),
            rel_path: "Travel".into(), local_path: "/Users/me/Travel".into(),
            enabled: true, delete_remote: false, created_at: 42, last_check_at: 43,
            last_result: Some(LastResult { ok: true, message: "Up to date".into() }),
        };
        save_all(&dir, std::slice::from_ref(&folder)).unwrap();
        assert_eq!(load(&dir), vec![folder.clone()]);
        // Editing one record leaves the rest of the file intact.
        let list = update_record(&dir, "abc", |f| f.enabled = false).unwrap();
        assert!(!list[0].enabled);
        assert_eq!(load(&dir)[0].enabled, false);
        assert!(update_record(&dir, "nope", |_| {}).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn older_records_without_the_newer_fields_still_load() {
        let dir = tmp();
        std::fs::write(config_path(&dir),
            r#"[{"id":"a","friendId":"f","locationId":"l","localPath":"/tmp/x"}]"#).unwrap();
        let list = load(&dir);
        assert_eq!(list.len(), 1);
        assert!(list[0].enabled, "a folder with no saved switch defaults to ON");
        assert!(!list[0].delete_remote, "deleting on the NAS is never on by default");
        assert_eq!(list[0].rel_path, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn destination_paths_never_double_up_slashes() {
        let mut folder = SyncedFolder {
            id: "a".into(), friend_id: "f".into(), location_id: "l".into(), rel_path: "Travel".into(),
            local_path: "/Users/me/Travel".into(), enabled: true, delete_remote: false,
            created_at: 0, last_check_at: 0, last_result: None,
        };
        assert_eq!(folder.dest("sub/b.jpg"), "Travel/sub/b.jpg");
        assert_eq!(folder.name(), "Travel");
        folder.rel_path = String::new();
        assert_eq!(folder.dest("sub/b.jpg"), "sub/b.jpg");
        assert_eq!(folder.dest(""), "");
    }

    #[test]
    fn an_away_host_is_a_wait_and_a_lost_share_is_an_error() {
        assert!(classify("DropBeam is still connecting — try again in a moment.", "Linux Box").waiting);
        assert!(classify("This upload is already running — let it finish", "Linux Box").waiting);
        assert!(!classify("This location is not shared with upload permission", "Linux Box").waiting);
        assert_eq!(classify("Friend not found", "Linux Box").message, "Friend not found");
    }
}
