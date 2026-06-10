//! Shared Drop Folder runtime.
//!
//! Per pair, depending on direction we run:
//!   * a SENDER side: a filesystem watcher (debounced + write-completion aware)
//!     feeds an ordered queue; a worker sends each file with `croc send` using
//!     the pair's derived outbound code, retrying with backoff while the peer is
//!     offline, and (if enabled) trashing the local copy only after croc exits 0.
//!   * a LISTENER side: a loop runs `croc <inbound-code>` receiving into a hidden
//!     staging dir, then moves arrivals into the folder collision-safely. Received
//!     paths are remembered so a two-way watcher never beams them back (loop guard).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::models::{
    DeleteMode, FolderState, FolderStatus, Friend, Locality, Pair, Settings,
};
use crate::{friends, pairing, AppState};

const MAX_BACKOFF_SECS: u64 = 30;

/// Manages all active Shared Drop Folders and friend inbox listeners.
pub struct SyncManager {
    app: AppHandle,
    config_dir: PathBuf,
    handles: Mutex<HashMap<String, PairHandle>>,
    friend_handles: Mutex<HashMap<String, FriendHandle>>,
}

struct FriendHandle {
    sig: String,
    stopped: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
}

struct PairHandle {
    /// Structural signature; a change forces a restart.
    sig: String,
    config: Arc<Mutex<Pair>>,
    stopped: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
    wake: Arc<Notify>,
    queue: Arc<Mutex<VecDeque<String>>>,
    inbound: Arc<Mutex<HashSet<String>>>,
    status: Arc<Mutex<StatusSnapshot>>,
    /// Loop-guard for files we just wrote/removed ourselves (shared with the
    /// listener + collector so an iroh receive lands without echoing back).
    self_deleted: Arc<Mutex<HashMap<String, Instant>>>,
    /// Mirror deletes queued for this link's peer; the control beacon flushes
    /// them. Stored here so a group delete can be FANNED to every other link.
    pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
    /// Wakes this link's control sender to flush a freshly-queued delete now.
    control_wake: Arc<Notify>,
    /// Tombstones (rel → deleted-at ms): every deletion we've observed locally or
    /// adopted from the peer. Rides the reconcile beacon so a delete propagates
    /// even if the live event was missed, and prevents a deleted file from being
    /// resurrected by the peer's add-reconcile. Persisted across restarts.
    tombstones: Arc<Mutex<HashMap<String, u64>>>,
    /// Set by the "Stop" button to immediately abort the in-flight folder transfer
    /// and move it aside, so a stuck send never traps the queue. Cleared by the
    /// sender once it's acted on.
    skip_current: Arc<AtomicBool>,
    _watcher: Option<notify::RecommendedWatcher>,
}

#[derive(Clone)]
struct StatusSnapshot {
    state: FolderState,
    sending_file: Option<String>,
    percent: f64,
    bytes_done: u64,
    bytes_total: u64,
    speed_bps: f64,
    eta_seconds: Option<f64>,
    detail: Option<String>,
    peer_online: bool,
    peer_name: Option<String>,
    locality: Locality,
    /// The peer told us they removed/stopped sharing this folder.
    peer_unshared: bool,
    /// How many files the peer reported in its last reconcile snapshot — so the UI
    /// can show "both have N files, in sync" and the user can SEE the folders match.
    peer_files: u32,
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        StatusSnapshot {
            state: FolderState::Idle,
            sending_file: None,
            percent: 0.0,
            bytes_done: 0,
            bytes_total: 0,
            speed_bps: 0.0,
            eta_seconds: None,
            detail: None,
            peer_online: false,
            peer_name: None,
            locality: Locality::Unknown,
            peer_unshared: false,
            peer_files: 0,
        }
    }
}

/// A removal to propagate to the mirror peer.
#[derive(Clone)]
struct DeleteEvent {
    rel: String,
    ts: u64,
}

/// A peer's full folder snapshot, carried on the control beacon for the periodic
/// self-heal reconcile. `files`: rel → (size, mtime). `tombstones`: rel → ms.
#[derive(Default, Clone)]
pub struct Reconcile {
    pub files: HashMap<String, (u64, u64)>,
    pub tombstones: HashMap<String, u64>,
    /// Relative paths of directories with NO files anywhere beneath them, so a
    /// folder someone makes to organize (even an empty one) still appears on the
    /// peer. Purely additive on apply — only ever creates directories.
    pub empty_dirs: Vec<String>,
}

impl SyncManager {
    pub fn new(app: AppHandle, config_dir: PathBuf) -> Arc<Self> {
        Arc::new(SyncManager {
            app,
            config_dir,
            handles: Mutex::new(HashMap::new()),
            friend_handles: Mutex::new(HashMap::new()),
        })
    }

    /// Bring running folders in line with what's persisted on disk.
    pub fn reconcile(self: &Arc<Self>) {
        let desired = pairing::load(&self.config_dir);
        let desired_ids: HashSet<String> = desired.iter().map(|p| p.id.clone()).collect();

        // Stop handles for removed pairs.
        let removed: Vec<String> = {
            let handles = self.handles.lock().unwrap();
            handles
                .keys()
                .filter(|id| !desired_ids.contains(*id))
                .cloned()
                .collect()
        };
        for id in removed {
            self.stop_pair(&id);
        }

        for pair in desired {
            let sig = structural_sig(&pair);
            let existing_sig = self
                .handles
                .lock()
                .unwrap()
                .get(&pair.id)
                .map(|h| h.sig.clone());
            match existing_sig {
                Some(s) if s == sig => {
                    // Only non-structural fields changed — update in place.
                    if let Some(h) = self.handles.lock().unwrap().get(&pair.id) {
                        *h.config.lock().unwrap() = pair.clone();
                    }
                }
                Some(_) => {
                    self.stop_pair(&pair.id);
                    self.start_pair(pair);
                }
                None => self.start_pair(pair),
            }
        }

        self.write_finder_folders();
        self.reconcile_friends();
    }

    /// Publish the list of shared-folder paths to a file the macOS Finder Sync
    /// extension reads (`finder-folders.json` in our app-support dir), so it knows
    /// which directories to watch + badge with sender provenance. No-op-safe.
    fn write_finder_folders(&self) {
        let folders: Vec<String> = pairing::load(&self.config_dir)
            .into_iter()
            .map(|p| p.folder)
            .filter(|f| !f.trim().is_empty())
            .collect();
        if let Ok(txt) = serde_json::to_string(&folders) {
            let _ = std::fs::write(self.config_dir.join("finder-folders.json"), txt);
        }
    }

    fn stop_pair(&self, id: &str) {
        if let Some(handle) = self.handles.lock().unwrap().remove(id) {
            handle.stopped.store(true, Ordering::SeqCst);
            handle.stop_notify.notify_waiters();
            handle.wake.notify_waiters();
            // Dropping `handle` drops the watcher, ending FS notifications.
        }
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.handles.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_pair(&id);
        }
        let fids: Vec<String> = self.friend_handles.lock().unwrap().keys().cloned().collect();
        for id in fids {
            self.stop_friend(&id);
        }
    }

    /// Bring friend inbox listeners in line with what's persisted on disk.
    pub fn reconcile_friends(self: &Arc<Self>) {
        // First collapse any duplicate records for the same person (added via more
        // than one path) and migrate their chat history — so the list below, and
        // every window we notify, only ever sees one canonical friend per person.
        friends::reconcile(&self.config_dir);
        let desired = friends::load(&self.config_dir);
        let desired_ids: HashSet<String> = desired.iter().map(|f| f.id.clone()).collect();

        let removed: Vec<String> = {
            let h = self.friend_handles.lock().unwrap();
            h.keys()
                .filter(|id| !desired_ids.contains(*id))
                .cloned()
                .collect()
        };
        for id in removed {
            self.stop_friend(&id);
        }

        for friend in desired {
            let sig = friend_sig(&friend);
            let existing = self
                .friend_handles
                .lock()
                .unwrap()
                .get(&friend.id)
                .map(|h| h.sig.clone());
            // iroh-only: friend pushes now arrive via the iroh accept loop's
            // "files" handler (with manual-accept handled there), so there's no
            // per-friend croc inbox listener to start.
            let _ = (existing, sig, friend);
        }
        // Tell every window (main + menu-bar popover) to reload its friend list,
        // so a newly added friend appears immediately without an app restart.
        let _ = self.app.emit("friends://changed", ());
    }

    fn stop_friend(&self, id: &str) {
        if let Some(h) = self.friend_handles.lock().unwrap().remove(id) {
            h.stopped.store(true, Ordering::SeqCst);
            h.stop_notify.notify_waiters();
        }
    }

    /// Abort the in-flight transfer for one folder NOW (the "Stop" button). The
    /// sender's watchdog catches the flag within ~1s, abandons the send, and moves
    /// the file aside so the rest of the folder keeps flowing. Nothing is dropped.
    pub fn stop_folder_transfer(&self, pair_id: &str) {
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            h.skip_current.store(true, Ordering::SeqCst);
            h.wake.notify_one();
        }
    }

    /// Force a self-heal reconcile NOW: wake every link's control sender so it
    /// re-beacons its manifest immediately (instead of waiting up to 5 min). Both
    /// sides then exchange snapshots and converge. Drives the manual "Verify"
    /// button.
    pub fn verify_now(&self) {
        for h in self.handles.lock().unwrap().values() {
            h.control_wake.notify_one();
        }
    }

    /// Current status snapshots for all active folders (for initial UI load).
    pub fn statuses(&self) -> Vec<FolderStatus> {
        let handles = self.handles.lock().unwrap();
        handles
            .iter()
            .map(|(id, h)| {
                let q = h.queue.lock().unwrap();
                let queued = q.len();
                let queued_files: Vec<String> = q.iter().take(60).map(|p| file_name_of(p)).collect();
                drop(q);
                let s = h.status.lock().unwrap().clone();
                let peer_name = h
                    .config
                    .lock()
                    .unwrap()
                    .endpoint_id
                    .as_deref()
                    .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
                    .or(s.peer_name);
                FolderStatus {
                    pair_id: id.clone(),
                    state: s.state,
                    queued,
                    sending_file: s.sending_file,
                    percent: s.percent,
                    bytes_done: s.bytes_done,
                    bytes_total: s.bytes_total,
                    speed_bps: s.speed_bps,
                    eta_seconds: s.eta_seconds,
                    detail: s.detail,
                    peer_online: s.peer_online,
                    peer_name,
                    locality: s.locality,
                    peer_unshared: s.peer_unshared,
                    queued_files,
                    peer_files: s.peer_files,
                }
            })
            .collect()
    }

    fn start_pair(self: &Arc<Self>, pair: Pair) {
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(Notify::new());
        let wake = Arc::new(Notify::new());
        let queue = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let inbound = Arc::new(Mutex::new(load_manifest(&self.config_dir, &pair.id)));
        let status = Arc::new(Mutex::new(StatusSnapshot::default()));
        let config = Arc::new(Mutex::new(pair.clone()));
        let sig = structural_sig(&pair);

        let mut watcher = None;

        // Total-sync (mirror) state: delete events queued for the peer, a loop
        // guard for files WE removed (so applying a remote delete or an overwrite
        // never echoes back), and a wake to flush deletes to the peer promptly.
        let pending_deletes: Arc<Mutex<Vec<DeleteEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let self_deleted: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let control_wake = Arc::new(Notify::new());
        let tombstones: Arc<Mutex<HashMap<String, u64>>> =
            Arc::new(Mutex::new(load_tombstones(&self.config_dir, &pair.id)));
        let skip_current = Arc::new(AtomicBool::new(false));

        if pairing::runs_sender(&pair) {
            // Filesystem watcher → candidate channel. We deliberately do NOT trust
            // the event *kind* to tell adds from deletes: macOS reports a
            // "Move to Trash" as a rename, not a Remove, so kind-based routing
            // silently dropped trashed-file deletes. Instead we forward every
            // touched path and let the collector classify by whether the file
            // still exists after a short settle.
            let (evt_tx, evt_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
            let folder = pair.folder.clone();
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    use notify::EventKind;
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        for path in event.paths {
                            let _ = evt_tx.send(path);
                        }
                    }
                }
            }) {
                Ok(mut w) => {
                    use notify::Watcher;
                    if let Err(e) =
                        w.watch(Path::new(&folder), notify::RecursiveMode::Recursive)
                    {
                        log::warn!("watch failed for {folder}: {e}");
                    } else {
                        watcher = Some(w);
                    }
                }
                Err(e) => log::warn!("watcher init failed: {e}"),
            }

            // Collector: debounce + size-stability → enqueue (and, in mirror mode,
            // turn removals into delete events for the peer).
            self.clone().spawn_collector(
                evt_rx,
                config.clone(),
                stopped.clone(),
                wake.clone(),
                queue.clone(),
                inbound.clone(),
                pending_deletes.clone(),
                self_deleted.clone(),
                control_wake.clone(),
                tombstones.clone(),
            );

            // Sender worker.
            self.clone().spawn_sender(
                config.clone(),
                stopped.clone(),
                stop_notify.clone(),
                wake.clone(),
                queue.clone(),
                inbound.clone(),
                status.clone(),
                skip_current.clone(),
            );

            // Seed the queue with any files already sitting in the folder.
            seed_existing(&pair.folder, &inbound, &queue, &wake);
        }

        // iroh-only: folder pushes arrive via the iroh accept loop's "folder-files"
        // handler (→ ingest_iroh_folder_files), so there's no croc receive listener.

        // Control channel (presence + identity + mirror delete events) runs for
        // BOTH peers on every pair, independent of file-sync direction — that's
        // how the creator learns the accepter exists + their name (fixing the
        // stuck "waiting" state) and how deletes reach the other side.
        self.clone().spawn_control_sender(
            config.clone(),
            stopped.clone(),
            stop_notify.clone(),
            status.clone(),
            pending_deletes.clone(),
            control_wake.clone(),
            tombstones.clone(),
        );
        // iroh-only: the peer's control payload (presence + name + mirror deletes)
        // arrives via the iroh accept loop's "folder-ctrl" handler.

        let handle = PairHandle {
            sig,
            config,
            stopped,
            stop_notify,
            wake,
            queue,
            inbound,
            status,
            self_deleted,
            pending_deletes,
            control_wake,
            tombstones,
            skip_current,
            _watcher: watcher,
        };
        self.handles.lock().unwrap().insert(pair.id.clone(), handle);
        self.emit_status(&pair.id);
    }

    fn spawn_collector(
        self: Arc<Self>,
        mut evt_rx: tokio::sync::mpsc::UnboundedReceiver<PathBuf>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        wake: Arc<Notify>,
        queue: Arc<Mutex<VecDeque<String>>>,
        inbound: Arc<Mutex<HashSet<String>>>,
        pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
        self_deleted: Arc<Mutex<HashMap<String, Instant>>>,
        control_wake: Arc<Notify>,
        tombstones: Arc<Mutex<HashMap<String, u64>>>,
    ) {
        let config_dir = self.config_dir.clone();
        let pair_id_c = config.lock().unwrap().id.clone();
        let debounce: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        tauri::async_runtime::spawn(async move {
            while let Some(path) = evt_rx.recv().await {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let (folder, mirror) = {
                    let c = config.lock().unwrap();
                    (c.folder.clone(), c.mirror)
                };
                let p = path.to_string_lossy().to_string();
                // One generation counter per path collapses bursts (and lets a
                // newer event supersede an in-flight handler for the same path).
                let gen = {
                    let mut d = debounce.lock().unwrap();
                    let g = d.entry(p.clone()).or_insert(0);
                    *g += 1;
                    *g
                };
                let debounce2 = debounce.clone();
                let queue2 = queue.clone();
                let wake2 = wake.clone();
                let inbound2 = inbound.clone();
                let stopped2 = stopped.clone();
                let folder2 = folder.clone();
                let pd = pending_deletes.clone();
                let cw = control_wake.clone();
                let self_deleted2 = self_deleted.clone();
                let tomb2 = tombstones.clone();
                let cfgdir2 = config_dir.clone();
                let pidc2 = pair_id_c.clone();
                tauri::async_runtime::spawn(async move {
                    // Classify by EXISTENCE, not by the OS event kind. A file that
                    // is present is an add/change; one that's gone is a delete —
                    // works no matter how macOS labels a trash/move/rename.
                    if Path::new(&p).is_file() {
                        // ── ADD / CHANGE ──────────────────────────────────────
                        if !is_sendable_candidate(&p, &folder2, &inbound2) {
                            return;
                        }
                        // Wait for quiet + confirm write-completion via size stability.
                        if !wait_until_stable(&p, &debounce2, gen, &stopped2).await {
                            return;
                        }
                        if stopped2.load(Ordering::SeqCst) {
                            return;
                        }
                        if !is_sendable_candidate(&p, &folder2, &inbound2) {
                            return;
                        }
                        {
                            let mut q = queue2.lock().unwrap();
                            if !q.iter().any(|x| x == &p) {
                                q.push_back(p.clone());
                            }
                        }
                        wake2.notify_one();
                    } else if Path::new(&p).is_dir() {
                        // ── DIRECTORY appeared / changed ──────────────────────
                        // A new subfolder, a folder dragged or moved IN, or the new
                        // side of a folder rename. macOS often fires one event for
                        // the directory rather than per child, so we recursively
                        // enqueue every sendable file inside — that syncs the whole
                        // structure + contents. CRITICAL: this branch also stops a
                        // directory from being misread as a delete (it isn't a file,
                        // so it used to fall through to the delete path and could
                        // wipe the folder's contents on the peer). The reconcile is
                        // the backstop for anything the live events still miss.
                        //
                        // Let an in-progress copy settle before scanning, so a folder
                        // dragged in mid-copy doesn't enqueue a half-written file. The
                        // reconcile re-sends on any later size/mtime change, so this
                        // just shrinks the partial-file window.
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        if stopped2.load(Ordering::SeqCst) {
                            return;
                        }
                        let files = list_files_rec(Path::new(&p));
                        let had_files = !files.is_empty();
                        let mut any = false;
                        {
                            let mut q = queue2.lock().unwrap();
                            for f in files {
                                let fp = f.to_string_lossy().to_string();
                                if is_sendable_candidate(&fp, &folder2, &inbound2)
                                    && !q.iter().any(|x| x == &fp)
                                {
                                    q.push_back(fp);
                                    any = true;
                                }
                            }
                        }
                        if any {
                            wake2.notify_one();
                        }
                        // An EMPTY new folder has no files to send — nudge the
                        // control beacon so its `emptyDirs` reconcile reaches the
                        // peer promptly instead of waiting for the idle cadence.
                        if mirror && !had_files {
                            cw.notify_one();
                        }
                    } else if mirror {
                        // ── DELETE (total-sync only; path is truly GONE) ──────
                        let Some(rel) = rel_path_of(&p, &folder2) else {
                            return;
                        };
                        if rel.is_empty()
                            || rel.ends_with(".dropbeam-incoming")
                            || rel.split('/').any(|c| c.starts_with('.'))
                        {
                            return;
                        }
                        // Loop guard: skip files WE removed (applying a remote
                        // delete, or replacing one we just received/overwrote).
                        {
                            let mut sd = self_deleted2.lock().unwrap();
                            prune_self_deleted(&mut sd);
                            if sd.remove(&rel).is_some() {
                                return;
                            }
                        }
                        // An editor's atomic save is unlink+rename; wait briefly and
                        // re-check. If the path came back as ANYTHING — a file (a
                        // save) or a directory (recreated/renamed) — it isn't a
                        // delete; bail so we don't propagate a phantom removal.
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                        if stopped2.load(Ordering::SeqCst) || Path::new(&p).exists() {
                            return;
                        }
                        // A newer event for this path superseded us (e.g. recreated
                        // then handled elsewhere).
                        if debounce2.lock().unwrap().get(&p).copied() != Some(gen) {
                            return;
                        }
                        {
                            let mut sd = self_deleted2.lock().unwrap();
                            prune_self_deleted(&mut sd);
                            if sd.remove(&rel).is_some() {
                                return;
                            }
                        }
                        // Expand a DIRECTORY deletion into its children. macOS fires
                        // one Remove event for the folder, not one per file inside —
                        // so propagating only the folder rel left every file behind
                        // on the peer. We recover the children from the manifest
                        // (the files we know lived under `rel/`), tombstone + queue a
                        // delete for each, AND for the folder rel itself (a real file
                        // delete has no children, so it just tombstones itself).
                        let mut targets: Vec<String> = {
                            let inb = inbound2.lock().unwrap();
                            let prefix = format!("{rel}/");
                            inb.iter()
                                .filter_map(|sig| sig_rel(sig))
                                .filter(|r| r == &rel || r.starts_with(&prefix))
                                .collect()
                        };
                        if !targets.iter().any(|r| r == &rel) {
                            targets.push(rel.clone());
                        }
                        let ts = now_ms();
                        {
                            let mut pend = pd.lock().unwrap();
                            for t in &targets {
                                note_tombstone(&cfgdir2, &pidc2, &tomb2, t, ts);
                                if !pend.iter().any(|d| &d.rel == t) {
                                    pend.push(DeleteEvent { rel: t.clone(), ts });
                                }
                            }
                        }
                        // Forget the deleted files in the loop-guard manifest so a
                        // later same-name file is treated as new.
                        {
                            let prefix = format!("{rel}/");
                            inbound2.lock().unwrap().retain(|sig| {
                                sig_rel(sig).map_or(true, |r| r != rel && !r.starts_with(&prefix))
                            });
                        }
                        cw.notify_one();
                    }
                });
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_sender(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        wake: Arc<Notify>,
        queue: Arc<Mutex<VecDeque<String>>>,
        inbound: Arc<Mutex<HashSet<String>>>,
        status: Arc<Mutex<StatusSnapshot>>,
        skip_current: Arc<AtomicBool>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let mut current = String::new();
            let mut offline_attempts: u32 = 0;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let next = queue.lock().unwrap().front().cloned();
                let Some(file) = next else {
                    set_status(&status, FolderState::Idle, None, 0.0, None);
                    manager.emit_status(&pair_id);
                    // While idle, periodically re-scan for any local file that never
                    // reached the peer — an initial send that got interrupted, or
                    // files that already existed when the folder was paired. Re-queuing
                    // them is how a total-sync folder reliably converges to the SAME
                    // set of files on both computers (seed_existing skips anything
                    // already delivered or queued, so this is cheap + idempotent).
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(45)) => {
                            let folder = config.lock().unwrap().folder.clone();
                            seed_existing(&folder, &inbound, &queue, &wake);
                        }
                    }
                    continue;
                };

                // A file removed from disk (e.g. user deleted it) is the ONLY reason
                // we ever drop a queued item without delivering.
                if !Path::new(&file).exists() {
                    queue.lock().unwrap().pop_front();
                    continue;
                }
                if file != current {
                    current = file.clone();
                    offline_attempts = 0;
                }

                let (pair, settings) = {
                    let p = config.lock().unwrap().clone();
                    let s = manager
                        .app
                        .try_state::<Arc<AppState>>()
                        .map(|st| st.settings.lock().unwrap().clone())
                        .unwrap_or_default();
                    (p, s)
                };
                let name = file_name_of(&file);

                set_status(&status, FolderState::Sending, Some(name.clone()), 0.0, None);
                manager.emit_status(&pair_id);

                // Always ATTEMPT the direct push (try_iroh_folder_send has its own
                // 12s dial timeout and returns None cheaply if the peer is
                // unreachable or we don't know their key yet). Driving the result
                // off the actual dial — not the cached presence flag — means a
                // queued file lands the instant the peer is reachable, instead of
                // waiting up to the 300s beacon cadence to flip peer_online.
                let iroh_loc = manager
                    .try_iroh_folder_send(&pair, &settings, &file, &status, &stopped, &skip_current)
                    .await;

                // The user hit "Stop" on this transfer: it was aborted above. Move
                // it to the back of the queue (never dropped — reconcile + the queue
                // will bring it back) so the rest of the folder keeps flowing NOW.
                if skip_current.swap(false, Ordering::SeqCst) {
                    let mut q = queue.lock().unwrap();
                    if let Some(pos) = q.iter().position(|x| x == &file) {
                        if let Some(f) = q.remove(pos) {
                            q.push_back(f);
                        }
                    }
                    drop(q);
                    current = String::new();
                    offline_attempts = 0;
                    set_status(&status, FolderState::Idle, None, 0.0, None);
                    manager.emit_status(&pair_id);
                    continue;
                }

                // iroh-only: deliver over iroh, else keep the file queued and back
                // off (the peer is offline / not yet reachable). The file is only
                // ever popped from the queue on confirmed delivery.
                let result = if iroh_loc.is_some() {
                    set_peer_online(&status, true);
                    SendOutcome::Delivered
                } else {
                    SendOutcome::Offline
                };

                match result {
                    SendOutcome::Delivered => {
                        queue.lock().unwrap().pop_front();
                        offline_attempts = 0;
                        // Remember it so a restart won't re-send it.
                        if let Some(sig) = file_sig(&file, &pair.folder) {
                            inbound.lock().unwrap().insert(sig);
                            let snapshot = inbound.lock().unwrap().clone();
                            manager.persist_manifest(&pair_id, &snapshot);
                        }
                        // Auto-delete only AFTER confirmed delivery.
                        if pair.auto_delete {
                            delete_local(&file, pair.delete_mode);
                        }
                        set_status(&status, FolderState::Idle, None, 0.0, None);
                        manager.emit_status(&pair_id);
                        // Cue the UI to optionally play a sound + flash the HUD.
                        let _ = manager.app.emit(
                            "folder-synced",
                            serde_json::json!({ "pairId": pair_id, "direction": "send" }),
                        );
                    }
                    SendOutcome::Offline => {
                        offline_attempts = offline_attempts.saturating_add(1);
                        set_status(
                            &status,
                            FolderState::Waiting,
                            Some(name.clone()),
                            0.0,
                            Some(format!("Waiting for {} to come online", peer_label(&pair))),
                        );
                        manager.emit_status(&pair_id);
                        // Don't let ONE file that keeps failing (e.g. a stalled send
                        // to a peer whose path just died) block every other file
                        // forever: after a few tries, rotate it to the BACK so the
                        // rest of the queue gets a turn. The file is never dropped —
                        // it comes back around and keeps retrying with backoff.
                        if offline_attempts >= 3 {
                            let mut q = queue.lock().unwrap();
                            if q.len() > 1 {
                                if let Some(pos) = q.iter().position(|x| x == &file) {
                                    if let Some(f) = q.remove(pos) {
                                        q.push_back(f);
                                    }
                                }
                                drop(q);
                                current = String::new();
                                offline_attempts = 0;
                                continue;
                            }
                        }
                        // Keep the file queued; just back off until the peer returns.
                        if wait_backoff(&stop_notify, &stopped, offline_attempts).await {
                            break;
                        }
                    }
                    SendOutcome::Failed(_msg) => {
                        // Peer is reachable but the rendezvous handshake raced —
                        // retry quickly. Never drop the file.
                        if wait_fixed(&stop_notify, &stopped, 1500).await {
                            break;
                        }
                    }
                    SendOutcome::Stopped => break,
                }
            }
        });
    }

    /// Beam our hello {name, deletes[]} to the peer on the control channel.
    /// Delivery means their control listener is up → they're online, and any
    /// deletes in the payload have been received (so we can clear them).
    fn spawn_control_sender(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        status: Arc<Mutex<StatusSnapshot>>,
        pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
        control_wake: Arc<Notify>,
        tombstones: Arc<Mutex<HashMap<String, u64>>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let ctrl_file = manager.config_dir.join(format!(".ctrl-out-{pair_id}.json"));
            let ctrl_path = ctrl_file.to_string_lossy().to_string();
            let mut offline_streak: u32 = 0;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let (pair, settings) = {
                    let p = config.lock().unwrap().clone();
                    let s = manager
                        .app
                        .try_state::<Arc<AppState>>()
                        .map(|st| st.settings.lock().unwrap().clone())
                        .unwrap_or_default();
                    (p, s)
                };
                let my_name = if settings.display_name.trim().is_empty() {
                    "DropBeam user".to_string()
                } else {
                    settings.display_name.clone()
                };
                let dels: Vec<DeleteEvent> = pending_deletes.lock().unwrap().clone();
                let dels_json: Vec<serde_json::Value> = dels
                    .iter()
                    .map(|d| serde_json::json!({ "rel": d.rel, "ts": d.ts }))
                    .collect();
                let payload = serde_json::json!({
                    "v": 1, "name": my_name, "ts": now_ms(), "deletes": dels_json,
                });
                let _ = std::fs::write(&ctrl_file, payload.to_string());

                // The self-heal reconcile snapshot: our full current file set +
                // tombstones, so the peer can converge to identical (mirror only).
                let reconcile_json = if pair.mirror {
                    let files: serde_json::Map<String, serde_json::Value> = live_manifest(&pair.folder)
                        .into_iter()
                        .map(|(rel, e)| (rel, serde_json::json!([e.size, e.mtime])))
                        .collect();
                    let tombs: serde_json::Map<String, serde_json::Value> = tombstones
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(rel, ts)| (rel.clone(), serde_json::json!(ts)))
                        .collect();
                    let empty_dirs = live_empty_dirs(&pair.folder);
                    Some(serde_json::json!({ "files": files, "tombstones": tombs, "emptyDirs": empty_dirs }))
                } else {
                    None
                };

                // Dial the peer directly and hand them the control payload.
                // Success means they're online AND received it (deletes included).
                let iroh_ok = match (pair.endpoint_id.clone(), manager.iroh_endpoint()) {
                    (Some(eid), Some(ep)) => {
                        let del_pairs: Vec<(String, u64)> =
                            dels.iter().map(|d| (d.rel.clone(), d.ts)).collect();
                        let (group_id, roster) =
                            build_group_roster(&manager.config_dir, &pair, &ep, &my_name);
                        let ok = crate::iroh_net::send_folder_ctrl(
                            &ep, &eid, &pair_id, &my_name, &del_pairs, &group_id, &roster, false,
                        )
                        .await
                        .is_ok();
                        // Self-heal manifest goes on its OWN stream (large cap), so a
                        // huge folder never bloats the presence beacon above. Only
                        // when the beacon landed (peer is reachable).
                        if ok {
                            if let Some(rec) = &reconcile_json {
                                let _ = crate::iroh_net::send_folder_reconcile(
                                    &ep, &eid, &pair_id, rec,
                                )
                                .await;
                            }
                        }
                        ok
                    }
                    _ => false,
                };
                if iroh_ok {
                    offline_streak = 0;
                    set_peer_online(&status, true);
                    manager.emit_status(&pair_id);
                    if !dels.is_empty() {
                        let sent: HashSet<String> =
                            dels.iter().map(|d| format!("{}|{}", d.rel, d.ts)).collect();
                        pending_deletes
                            .lock()
                            .unwrap()
                            .retain(|d| !sent.contains(&format!("{}|{}", d.rel, d.ts)));
                    }
                    tokio::select! {
                        _ = control_wake.notified() => {}
                        _ = tokio::time::sleep(Duration::from_secs(300)) => {}
                        _ = stop_notify.notified() => break,
                    }
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }

                // iroh couldn't reach the peer this round → treat them as offline
                // and back off. A pending delete keeps us trying briskly so mirrors
                // still converge; control_wake (a new delete) or the peer beaconing
                // us flips presence back online sooner.
                set_peer_online(&status, false);
                manager.emit_status(&pair_id);
                offline_streak = offline_streak.saturating_add(1);
                let have_deletes = !pending_deletes.lock().unwrap().is_empty();
                let wait = if have_deletes {
                    25
                } else {
                    match offline_streak {
                        1 => 60,
                        2 => 120,
                        _ => 300,
                    }
                };
                tokio::select! {
                    _ = control_wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                    _ = stop_notify.notified() => break,
                }
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
            }
            let _ = std::fs::remove_file(&ctrl_file);
        });
    }

    /// Tell the peer(s) of `pair_id` that we're no longer sharing this folder, so
    /// their side can show "no longer shared by ___" instead of a dead link.
    /// Best-effort + fire-and-forget — called just BEFORE the pair record is
    /// removed, so the endpoint id is still available.
    pub fn announce_unshare(self: &Arc<Self>, pair_id: &str) {
        let pairs = pairing::load(&self.config_dir);
        let Some(pair) = pairs.into_iter().find(|p| p.id == pair_id) else {
            return;
        };
        let Some(eid) = pair.endpoint_id.clone() else {
            return;
        };
        let Some(ep) = self.iroh_endpoint() else {
            return;
        };
        let my_name = self
            .app
            .try_state::<Arc<AppState>>()
            .map(|st| st.settings.lock().unwrap().display_name.clone())
            .unwrap_or_default();
        let pid = pair_id.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = crate::iroh_net::send_folder_ctrl(
                &ep, &eid, &pid, &my_name, &[], "", &[], true,
            )
            .await;
        });
    }

    fn on_peer_hello(
        self: &Arc<Self>,
        pair_id: &str,
        config: &Arc<Mutex<Pair>>,
        name: &str,
        status: &Arc<Mutex<StatusSnapshot>>,
    ) {
        // Surface the user's own LABEL for this peer if they set one (resolved by
        // the peer's stable endpoint id), otherwise the name the peer broadcast.
        let (eid, secret, role) = {
            let p = config.lock().unwrap();
            (p.endpoint_id.clone(), p.secret.clone(), p.role)
        };
        let shown = eid
            .as_deref()
            .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
            .unwrap_or_else(|| name.to_string());
        if let Ok(mut s) = status.lock() {
            s.peer_online = true;
            s.peer_name = Some(shown);
            // A normal beacon means they're sharing again — clear a stale "unshared".
            s.peer_unshared = false;
        }
        let changed = pairing::set_peer_name(&self.config_dir, pair_id, name);
        if changed {
            config.lock().unwrap().peer_name = name.to_string();
            // Link the friend by their STABLE endpoint id (dedups, never clobbers a
            // user-set label). Fall back to the legacy name-keyed path only when we
            // somehow don't have their endpoint id yet.
            match &eid {
                Some(e) => {
                    let _ = friends::upsert_by_endpoint(&self.config_dir, e, name);
                }
                None => friends::upsert_from_pairing(&self.config_dir, name, &secret, role),
            }
            self.reconcile_friends();
            let _ = self.app.emit("pairs://changed", ());
        }
        self.emit_status(pair_id);
    }

    /// Apply a control payload received over iroh — the iroh equivalent of the
    /// croc control listener. Marks the peer online, learns their name (which
    /// also links the friend record), and in mirror mode propagates their deletes
    /// into our folder. Called from the iroh accept loop's "folder-ctrl" handler.
    pub fn apply_remote_control(
        self: &Arc<Self>,
        pair_id: &str,
        name: &str,
        deletes: &[(String, u64)],
        group_id: &str,
        members: &[(String, String)],
        reconcile: Option<&Reconcile>,
        unshared: bool,
    ) {
        let (config, status, self_deleted, tombstones, queue, wake, inbound) = {
            let handles = self.handles.lock().unwrap();
            let Some(h) = handles.get(pair_id) else {
                return; // not a folder we're actively managing
            };
            (
                h.config.clone(),
                h.status.clone(),
                h.self_deleted.clone(),
                h.tombstones.clone(),
                h.queue.clone(),
                h.wake.clone(),
                h.inbound.clone(),
            )
        };
        // The peer stopped sharing this folder. Mark it so the UI can say so and
        // stop pestering them — we KEEP our local copy of the files (the user can
        // remove the now-defunct link themselves). One signal is enough; ignore
        // everything else in this beacon.
        if unshared {
            if let Ok(mut s) = status.lock() {
                s.peer_online = false;
                s.peer_unshared = true;
            }
            self.emit_status(pair_id);
            return;
        }
        // Presence + name (also links the friend), same as the croc path. If no
        // name rode along, still mark them online — we just heard from them.
        let name = name.trim();
        if !name.is_empty() {
            self.on_peer_hello(pair_id, &config, name, &status);
        } else {
            set_peer_online(&status, true);
            self.emit_status(pair_id);
        }
        // Mirror-mode delete propagation (apply_remote_delete is idempotent, so a
        // re-delivered delete is a harmless no-op).
        let (folder, mirror) = {
            let p = config.lock().unwrap();
            (p.folder.clone(), p.mirror)
        };
        // Only trust the roster/group on a beacon whose group_id MATCHES this
        // link's own group_id — a peer can't make us create links under some other
        // group id (defense in depth; the eids still only ever point at this folder).
        let my_group = config.lock().unwrap().group_id.clone();
        let group_ok = !group_id.is_empty() && my_group.as_deref() == Some(group_id);

        if mirror && !deletes.is_empty() {
            let mut applied: Vec<(String, u64)> = Vec::new();
            for (rel, ts) in deletes {
                let mut removed_rels: Vec<String> = Vec::new();
                let did = apply_remote_delete(&folder, rel, &self_deleted, &mut removed_rels);
                // Tombstone the rel(s) so reconcile won't resurrect them and so the
                // delete keeps propagating across a group. Always tombstone the
                // named rel (even a no-op re-delivery) at the peer's timestamp.
                note_tombstone(&self.config_dir, pair_id, &tombstones, rel, *ts);
                for r in &removed_rels {
                    note_tombstone(&self.config_dir, pair_id, &tombstones, r, *ts);
                }
                if did {
                    applied.push((rel.clone(), *ts));
                }
            }
            if !applied.is_empty() {
                let _ = self.app.emit("folder-history://changed", pair_id);
                // In a group, forward each delete WE just applied to our OTHER
                // links, so it reaches members not directly connected to the
                // origin (we only forward freshly-applied deletes, so a
                // re-delivery — file already gone — doesn't ping-pong forever).
                if group_ok {
                    self.fan_group_deletes(pair_id, group_id, &applied);
                }
            }
        }

        // Self-heal reconcile: the peer told us its full file set + tombstones.
        // Apply any deletes we missed, and queue any files the peer is missing —
        // the bulletproof double-check that both folders converge to identical.
        if mirror {
            if let Some(rec) = reconcile {
                // Record the peer's file count for the "both have N files, in sync"
                // visibility indicator.
                if let Ok(mut s) = status.lock() {
                    s.peer_files = rec.files.len() as u32;
                }
                self.reconcile_apply(
                    pair_id, &folder, rec, &self_deleted, &tombstones, &queue, &wake, &inbound,
                );
                self.emit_status(pair_id);
            }
        }

        // Multi-person folders: the beacon carries the group roster, so mesh with
        // any member we don't have a link to yet (gossip — this converges the whole
        // group and self-heals if someone was offline when a person joined). A
        // classic 1:1 folder has an empty group/roster, so this is a no-op there.
        if group_ok && !members.is_empty() {
            let my_eid = self
                .iroh_endpoint()
                .map(|ep| ep.id().to_string())
                .unwrap_or_default();
            // Without our own key we can't safely tell ourselves apart from a
            // roster entry → skip rather than risk a self-referential link.
            if !my_eid.is_empty() {
                let template = config.lock().unwrap().clone();
                let mut added = false;
                for (eid, mname) in members {
                    if pairing::ensure_member(
                        &self.config_dir,
                        group_id,
                        &template,
                        eid,
                        mname,
                        &my_eid,
                    )
                    .is_some()
                    {
                        added = true;
                    }
                }
                if added {
                    self.clone().reconcile();
                    let _ = self.app.emit("pairs://changed", ());
                }
            }
        }
    }

    /// Self-heal reconcile. Given the PEER's full folder snapshot (`rec.files`)
    /// and its tombstones, bring our copy into agreement WITHOUT ever guessing:
    ///   • Apply each peer tombstone newer than our local file → delete it (so a
    ///     missed live delete still converges). Pure tombstone-driven — we never
    ///     delete just because the peer "doesn't have" a file.
    ///   • Queue every file WE have that the peer lacks (and hasn't tombstoned
    ///     newer than ours) → re-send it. Catches any add the live path missed.
    /// The push is symmetric (the peer runs the same against our snapshot), so the
    /// two folders converge to the union of non-deleted files. Safe failure mode:
    /// a lost tombstone resurrects a file (extra copy), never silent data loss.
    #[allow(clippy::too_many_arguments)]
    fn reconcile_apply(
        self: &Arc<Self>,
        pair_id: &str,
        folder: &str,
        rec: &Reconcile,
        self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
        tombstones: &Arc<Mutex<HashMap<String, u64>>>,
        queue: &Arc<Mutex<VecDeque<String>>>,
        wake: &Arc<Notify>,
        inbound: &Arc<Mutex<HashSet<String>>>,
    ) {
        let mine = live_manifest(folder);
        let my_tomb = tombstones.lock().unwrap().clone();
        let plan = reconcile_plan(&mine, &rec.files, &rec.tombstones, &my_tomb);

        // 1) Apply peer tombstones we missed: delete a local file the peer deleted
        //    AFTER our copy. Archive first (recoverable from history).
        let mut deleted_any = false;
        for rel in &plan.delete {
            let tomb_ts = rec.tombstones.get(rel).copied().unwrap_or_else(now_ms);
            let mut removed: Vec<String> = Vec::new();
            if apply_remote_delete(folder, rel, self_deleted, &mut removed) {
                deleted_any = true;
                for r in &removed {
                    note_tombstone(&self.config_dir, pair_id, tombstones, r, tomb_ts);
                    inbound
                        .lock()
                        .unwrap()
                        .retain(|sig| sig_rel(sig).as_deref() != Some(r.as_str()));
                }
            }
        }
        // Adopt EVERY peer tombstone so we forward it and never resurrect, even the
        // ones for files we never had.
        for (rel, &ts) in &rec.tombstones {
            note_tombstone(&self.config_dir, pair_id, tombstones, rel, ts);
        }
        if deleted_any {
            let _ = self.app.emit("folder-history://changed", pair_id);
        }

        // 2) Push files the peer is missing or has an older copy of.
        let mut queued = 0usize;
        for rel in &plan.push {
            let abs = Path::new(folder).join(rel).to_string_lossy().to_string();
            if Path::new(&abs).is_file() {
                let mut q = queue.lock().unwrap();
                if !q.iter().any(|x| x == &abs) {
                    q.push_back(abs);
                    queued += 1;
                }
            }
        }
        if queued > 0 {
            log::info!("reconcile[{pair_id}]: re-queued {queued} file(s) the peer was missing");
            wake.notify_one();
        }

        // 3) Empty directories. Purely additive: create any empty folder the peer
        //    has that we don't — UNLESS we hold a tombstone for that path (we
        //    deleted it; don't resurrect). And honor a peer's delete: remove an
        //    empty dir we have if the peer tombstoned it (remove_dir only removes
        //    it when it's actually empty, so a dir that's since gained files is
        //    never touched). Never deletes data.
        for rel in &rec.empty_dirs {
            let norm = rel.replace('\\', "/");
            if norm.is_empty()
                || norm.starts_with('/')
                || norm.split('/').any(|c| c == ".." || c.starts_with('.'))
            {
                continue;
            }
            if my_tomb.contains_key(&norm) || rec.tombstones.contains_key(&norm) {
                continue; // deleted somewhere — don't recreate
            }
            let abs = Path::new(folder).join(&norm);
            if !abs.exists() {
                let _ = std::fs::create_dir_all(&abs);
            }
        }
        for (rel, _) in &rec.tombstones {
            let abs = Path::new(folder).join(rel);
            if abs.is_dir() {
                self_deleted
                    .lock()
                    .unwrap()
                    .insert(rel.clone(), Instant::now());
                let _ = std::fs::remove_dir(&abs); // only succeeds if empty
            }
        }
    }

    /// Forward a set of just-applied mirror deletes to every OTHER link in the
    /// same folder group, so a delete propagates across the whole mesh (not just
    /// the one hop from the origin). Dedups so a delete isn't queued twice.
    fn fan_group_deletes(&self, from_pair: &str, group_id: &str, deletes: &[(String, u64)]) {
        let other_ids: Vec<String> = pairing::members_of_group(&self.config_dir, group_id)
            .into_iter()
            .map(|p| p.id)
            .filter(|id| id != from_pair)
            .collect();
        let handles = self.handles.lock().unwrap();
        for id in other_ids {
            if let Some(h) = handles.get(&id) {
                {
                    let mut pd = h.pending_deletes.lock().unwrap();
                    for (rel, ts) in deletes {
                        if !pd.iter().any(|d| d.rel == *rel && d.ts == *ts) {
                            pd.push(DeleteEvent {
                                rel: rel.clone(),
                                ts: *ts,
                            });
                        }
                    }
                }
                h.control_wake.notify_one();
            }
        }
    }

    fn persist_manifest(&self, pair_id: &str, set: &HashSet<String>) {
        save_manifest(&self.config_dir, pair_id, set);
    }

    fn note_received(&self, pair: &Pair, files: &[String]) {
        use crate::history;
        use crate::models::{Direction, HistoryEntry, Locality, TransferState};
        // Stamp each received file with WHO it's from (your saved label, else their
        // broadcast name) as a macOS extended attribute, so the Finder Sync
        // extension — and Get Info — can show its provenance. Best-effort.
        let from = pair
            .endpoint_id
            .as_deref()
            .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
            .unwrap_or_else(|| pair.peer_name.clone());
        for f in files {
            crate::provenance::set_sender(Path::new(f), &from);
        }
        let names: Vec<String> = files.iter().map(|f| file_name_of(f)).collect();
        let total: u64 = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
            .sum();
        history::append(
            &self.config_dir,
            HistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                direction: Direction::Receive,
                file_names: names.clone(),
                bytes_total: total,
                peer: Some(peer_label(pair)),
                locality: Locality::Unknown,
                code: None,
                state: TransferState::Completed,
                timestamp_ms: now_ms(),
                error: None,
                out_dir: Some(pair.folder.clone()),
            },
        );
        let _ = self.app.emit("history://changed", ());
        let _ = self.app.emit(
            "folder-synced",
            serde_json::json!({ "pairId": pair.id, "direction": "receive" }),
        );
        let settings_notify = self
            .app
            .try_state::<Arc<AppState>>()
            .map(|s| s.settings.lock().unwrap().notify_on_complete)
            .unwrap_or(true);
        if settings_notify {
            use tauri_plugin_notification::NotificationExt;
            // Who it's from — your saved label for them by stable endpoint id, else
            // the name they broadcast. Surfaces provenance the way the user asked
            // for (issue #12), via a notification rather than a Finder badge.
            let from = pair
                .endpoint_id
                .as_deref()
                .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
                .filter(|n| !n.trim().is_empty())
                .or_else(|| {
                    let n = pair.peer_name.trim();
                    (!n.is_empty()).then(|| n.to_string())
                });
            let what = if names.len() == 1 {
                names[0].clone()
            } else {
                format!("{} files", names.len())
            };
            let body = match &from {
                Some(name) => format!("{what} from {name} → {}", folder_name(&pair.folder)),
                None => format!("{what} arrived in {}", folder_name(&pair.folder)),
            };
            let _ = self.app.notification().builder().title("DropBeam").body(body).show();
        }
    }

    // ── iroh direct folder sync (Phase 4) ────────────────────────────────────

    /// The live iroh endpoint, if Direct mode is up. Used to dial folder peers.
    fn iroh_endpoint(&self) -> Option<iroh::Endpoint> {
        self.app
            .try_state::<Arc<crate::iroh_net::IrohState>>()
            .and_then(|st| st.get().cloned())
    }

    /// Whether we're actively managing this folder pair (a running handle exists).
    /// The iroh accept loop checks this before landing a pushed folder file.
    pub fn has_pair(&self, pair_id: &str) -> bool {
        self.handles.lock().unwrap().contains_key(pair_id)
    }

    /// Live progress for an in-flight iroh folder RECEIVE — drives the same status
    /// bar (and Local/Internet badge) the croc receive path uses.
    /// On-disk path of a managed folder pair — used to drop a visible
    /// "<name>.dropbeam-incoming" placeholder while a file is arriving.
    pub fn folder_path(&self, pair_id: &str) -> Option<String> {
        self.handles
            .lock()
            .unwrap()
            .get(pair_id)
            .map(|h| h.config.lock().unwrap().folder.clone())
    }

    pub fn note_folder_receiving(&self, pair_id: &str, done: u64, total: u64, locality: Locality) {
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            if let Ok(mut s) = h.status.lock() {
                s.state = FolderState::Receiving;
                s.sending_file = None;
                s.bytes_done = done;
                if total > 0 {
                    s.bytes_total = total;
                    s.percent = done as f64 / total as f64 * 100.0;
                }
                if !matches!(locality, Locality::Unknown) {
                    s.locality = locality;
                }
            }
        }
        self.emit_status(pair_id);
    }

    /// Land iroh-received folder files: move them from the private staging dir into
    /// the shared folder using the EXACT same loop-protection / mirror / history
    /// rules as the croc path (shared `inbound` + `self_deleted` guards), then
    /// update the manifest, record history, and reset status to Idle.
    pub fn ingest_iroh_folder_files(&self, pair_id: &str, staging: &Path) -> Vec<String> {
        let (config, inbound, self_deleted) = {
            let handles = self.handles.lock().unwrap();
            let Some(h) = handles.get(pair_id) else {
                return Vec::new();
            };
            (h.config.clone(), h.inbound.clone(), h.self_deleted.clone())
        };
        let (folder, mirror, group) = {
            let p = config.lock().unwrap();
            (p.folder.clone(), p.mirror, p.group_id.is_some())
        };
        let moved = move_staged_into_folder(staging, &folder, &inbound, mirror, group, &self_deleted);
        if !moved.is_empty() {
            let snapshot = inbound.lock().unwrap().clone();
            self.persist_manifest(pair_id, &snapshot);
            let pair = config.lock().unwrap().clone();
            self.note_received(&pair, &moved);
        }
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            if let Ok(mut s) = h.status.lock() {
                s.state = FolderState::Idle;
                s.percent = 0.0;
                s.sending_file = None;
            }
        }
        self.emit_status(pair_id);
        moved
    }

    /// Try to push one folder file directly over iroh. Returns `Some(locality)` on
    /// confirmed delivery, or `None` when the direct path is unavailable or fails
    /// — in which case the caller falls back to the croc relay, so folders keep
    /// working even if the peer is offline or on an older build.
    async fn try_iroh_folder_send(
        self: &Arc<Self>,
        pair: &Pair,
        _settings: &Settings,
        file: &str,
        status: &Arc<Mutex<StatusSnapshot>>,
        stopped: &Arc<AtomicBool>,
        skip_current: &Arc<AtomicBool>,
    ) -> Option<Locality> {
        let eid = pair.endpoint_id.clone()?;
        let ep = self.iroh_endpoint()?;
        let paths = vec![PathBuf::from(file)];
        // Tracks the last time bytes actually moved, so a STALL watchdog can abandon
        // a frozen transfer instead of letting it wedge the whole folder queue.
        let last_progress = Arc::new(AtomicU64::new(now_ms()));
        let cb = {
            let mgr = self.clone();
            let status = status.clone();
            let pair_id = pair.id.clone();
            let last = Arc::new(AtomicU64::new(0));
            let lp = last_progress.clone();
            let start = Instant::now();
            move |done: u64, total: u64| {
                lp.store(now_ms(), Ordering::Relaxed);
                // Throttle to ~1% steps so we don't flood the UI per chunk.
                let permille = if total > 0 { done * 1000 / total } else { 0 };
                if done < total && permille <= last.load(Ordering::Relaxed) {
                    return;
                }
                last.store(permille, Ordering::Relaxed);
                // Live speed + ETA, same as the Send/Receive tab — folder transfers
                // showed neither before (issues #11/#13).
                let secs = start.elapsed().as_secs_f64().max(0.001);
                let speed = done as f64 / secs;
                if let Ok(mut s) = status.lock() {
                    s.state = FolderState::Sending;
                    s.bytes_done = done;
                    s.speed_bps = speed;
                    if total > 0 {
                        s.bytes_total = total;
                        s.percent = done as f64 / total as f64 * 100.0;
                        let remaining = total.saturating_sub(done) as f64;
                        s.eta_seconds = if speed > 1.0 { Some(remaining / speed) } else { None };
                    }
                }
                mgr.emit_status(&pair_id);
            }
        };
        // Race the send against a no-progress watchdog. A flaky/dead path (e.g. an
        // international link that drops, leaving the QUIC connection alive via
        // keep-alives but the stream flow-control-stuck) would otherwise hang
        // `send_folder_file` FOREVER — freezing this folder's single-file queue and
        // blocking every other file behind it. If no byte moves for STALL_SECS we
        // bail; dropping the future closes the wedged connection, and the sender
        // loop retries (or rotates the file) so the queue keeps flowing.
        const STALL_SECS: u64 = 45;
        let send_fut = crate::iroh_net::send_folder_file(
            &ep,
            &eid,
            &pair.id,
            &pair.folder,
            &paths,
            &**stopped,
            cb,
        );
        let watchdog = {
            let lp = last_progress.clone();
            let stopped = stopped.clone();
            let skip = skip_current.clone();
            async move {
                loop {
                    // Poll at 1s so a manual "Stop" aborts within a second.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if stopped.load(Ordering::SeqCst) || skip.load(Ordering::SeqCst) {
                        return false; // user/teardown abort — not a stall
                    }
                    if now_ms().saturating_sub(lp.load(Ordering::Relaxed)) > STALL_SECS * 1000 {
                        return true; // stalled
                    }
                }
            }
        };
        let outcome = tokio::select! {
            r = send_fut => r,
            stalled = watchdog => {
                if stalled {
                    log::warn!(
                        "folder send '{}' stalled ({}s no progress) — abandoning so the queue keeps moving",
                        file_name_of(file), STALL_SECS
                    );
                }
                Err(anyhow::anyhow!("folder send aborted"))
            }
        };
        match outcome {
            Ok(loc) => {
                if let Ok(mut s) = status.lock() {
                    if !matches!(loc, Locality::Unknown) {
                        s.locality = loc;
                    }
                }
                Some(loc)
            }
            Err(e) => {
                log::debug!("iroh folder send failed (falling back to croc): {e}");
                None
            }
        }
    }

    fn emit_status(&self, pair_id: &str) {
        let snapshot = {
            let handles = self.handles.lock().unwrap();
            let Some(h) = handles.get(pair_id) else {
                return;
            };
            let q = h.queue.lock().unwrap();
            let queued = q.len();
            let queued_files: Vec<String> =
                q.iter().take(60).map(|p| file_name_of(p)).collect();
            drop(q);
            let s = h.status.lock().unwrap().clone();
            let eid = h.config.lock().unwrap().endpoint_id.clone();
            (queued, queued_files, s, eid)
        };
        let (queued, queued_files, mut s, eid) = snapshot;
        // Always prefer the user's own label for this peer (by stable endpoint id).
        if let Some(label) = eid
            .as_deref()
            .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
        {
            s.peer_name = Some(label);
        }
        let status = FolderStatus {
            pair_id: pair_id.to_string(),
            state: s.state,
            queued,
            sending_file: s.sending_file,
            percent: s.percent,
            bytes_done: s.bytes_done,
            bytes_total: s.bytes_total,
            speed_bps: s.speed_bps,
            eta_seconds: s.eta_seconds,
            detail: s.detail,
            peer_online: s.peer_online,
            peer_name: s.peer_name,
            locality: s.locality,
            peer_unshared: s.peer_unshared,
            queued_files,
            peer_files: s.peer_files,
        };
        let _ = self.app.emit("folder://status", status);
    }
}

// ---------------------------------------------------------------------------
// croc send / receive runners
// ---------------------------------------------------------------------------

enum SendOutcome {
    Delivered,
    Offline,
    Failed(String),
    Stopped,
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Wait for the file to stop changing (size stable for two consecutive polls),
/// returning false if a newer event superseded this one or we stopped.
async fn wait_until_stable(
    path: &str,
    debounce: &Arc<Mutex<HashMap<String, u64>>>,
    gen: u64,
    stopped: &Arc<AtomicBool>,
) -> bool {
    // Quiet period: bail if a newer event arrived.
    tokio::time::sleep(Duration::from_millis(900)).await;
    if debounce.lock().unwrap().get(path) != Some(&gen) {
        return false;
    }
    let mut last = file_len(path);
    for _ in 0..40 {
        if stopped.load(Ordering::SeqCst) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        let now = file_len(path);
        if now.is_some() && now == last {
            debounce.lock().unwrap().remove(path);
            return now.unwrap_or(0) > 0 || Path::new(path).is_file();
        }
        last = now;
    }
    debounce.lock().unwrap().remove(path);
    true
}

fn file_len(path: &str) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn is_sendable_candidate(path: &str, folder: &str, inbound: &Arc<Mutex<HashSet<String>>>) -> bool {
    let p = Path::new(path);
    if !p.is_file() {
        return false;
    }
    // Skip the staging dir and dotfiles / temp files anywhere in the relative path.
    if let Ok(rel) = p.strip_prefix(folder) {
        for comp in rel.components() {
            let name = comp.as_os_str().to_string_lossy();
            if name.starts_with('.') {
                return false;
            }
        }
    }
    let fname = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if fname.ends_with(".crdownload") || fname.ends_with(".download") || fname.ends_with(".part") || fname.ends_with(".tmp") || fname.ends_with(".dropbeam-incoming") {
        return false;
    }
    // Already sent or received (and unchanged since)? Don't (re)send it.
    if let Some(sig) = file_sig(path, folder) {
        if inbound.lock().unwrap().contains(&sig) {
            return false;
        }
    }
    true
}

fn seed_existing(
    folder: &str,
    inbound: &Arc<Mutex<HashSet<String>>>,
    queue: &Arc<Mutex<VecDeque<String>>>,
    wake: &Arc<Notify>,
) {
    let files = list_files_rec(Path::new(folder));
    let mut any = false;
    {
        let mut q = queue.lock().unwrap();
        for f in files {
            let p = f.to_string_lossy().to_string();
            // Sweep a stale "<name>.dropbeam-incoming" placeholder left by a receive
            // that was interrupted (e.g. crash mid-transfer). It's never synced —
            // just visible litter — so delete it on scan instead of leaving it.
            if p.ends_with(".dropbeam-incoming") {
                let _ = std::fs::remove_file(&f);
                continue;
            }
            if is_sendable_candidate(&p, folder, inbound) && !q.iter().any(|x| x == &p) {
                q.push_back(p);
                any = true;
            }
        }
    }
    if any {
        wake.notify_one();
    }
}

fn list_files_rec(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            // file_type() does NOT follow symlinks (unlike is_dir/is_file). Skip a
            // symlink entirely so a link inside the folder can never lead a
            // recursive delete — or a send — out to external files.
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// Move everything received in `staging` into `folder`, collision-safe.
/// Returns the final absolute paths and records them as inbound (loop guard).
fn move_staged_into_folder(
    staging: &Path,
    folder: &str,
    inbound: &Arc<Mutex<HashSet<String>>>,
    mirror: bool,
    group: bool,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
) -> Vec<String> {
    let folder_str = folder;
    let folder_path = Path::new(folder);
    let mut moved = Vec::new();
    for src in list_files_rec(staging) {
        let Ok(rel) = src.strip_prefix(staging) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let dest_path = folder_path.join(rel);
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // The incoming (staged) version's identity. mtime was already stamped to
        // the origin's value by the receive path, so it's comparable across members.
        let in_size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        let in_mtime = std::fs::metadata(&src).ok().map(|m| meta_mtime(&m)).unwrap_or(0);

        // Mirror = shared source of truth. Resolve every incoming file
        // DETERMINISTICALLY so all members converge to the same bytes:
        //   • identical version already here → no-op (kills the mesh echo/storm),
        //   • a genuinely different version → newest modified-time wins (an
        //     equal-second race is broken by content hash); the loser is archived
        //     to History so nothing is ever lost.
        if mirror && dest_path.is_file() && group {
            // GROUP folder: deterministic resolution so all members converge.
            let loc_size = std::fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0);
            let loc_mtime = std::fs::metadata(&dest_path)
                .ok()
                .map(|m| meta_mtime(&m))
                .unwrap_or(0);
            // Truly identical (same size + mtime AND content) → no-op. The content
            // check guards a same-second, same-size, DIFFERENT-content collision
            // from being silently dropped.
            if in_size == loc_size
                && in_mtime == loc_mtime
                && content_hash(&src) == content_hash(&dest_path)
            {
                let _ = std::fs::remove_file(&src);
                if let Some(sig) = file_sig(&dest_path.to_string_lossy(), folder_str) {
                    inbound.lock().unwrap().insert(sig);
                }
                continue;
            }
            let incoming_wins = if in_mtime != loc_mtime {
                in_mtime > loc_mtime
            } else {
                content_hash(&src) > content_hash(&dest_path)
            };
            if !incoming_wins {
                // Local copy is the winner — keep it, archive the (older) incoming
                // version to History so it isn't lost, and DON'T rewrite.
                crate::folder_history::archive(
                    folder_str,
                    &src.to_string_lossy(),
                    &rel_str,
                    "replaced",
                );
                let _ = std::fs::remove_file(&src);
                continue;
            }
            // Incoming wins → archive the local copy, then replace it below.
            self_deleted
                .lock()
                .unwrap()
                .insert(rel_str.clone(), Instant::now());
            crate::folder_history::archive(
                folder_str,
                &dest_path.to_string_lossy(),
                &rel_str,
                "replaced",
            );
            if dest_path.exists() {
                let _ = std::fs::remove_file(&dest_path);
            }
        } else if mirror && dest_path.is_file() {
            // Classic 1:1 mirror — UNCHANGED: the incoming file always replaces the
            // local one (arrival-order wins), old copy archived to History. No
            // mtime/clock dependence, so no skew regression for shipped folders.
            self_deleted
                .lock()
                .unwrap()
                .insert(rel_str.clone(), Instant::now());
            crate::folder_history::archive(
                folder_str,
                &dest_path.to_string_lossy(),
                &rel_str,
                "replaced",
            );
            if dest_path.exists() {
                let _ = std::fs::remove_file(&dest_path);
            }
        }

        let dest = if mirror {
            dest_path
        } else {
            unique_dest(dest_path)
        };
        let dest_str = dest.to_string_lossy().to_string();
        let ok = if std::fs::rename(&src, &dest).is_ok() {
            true
        } else if std::fs::copy(&src, &dest).is_ok() {
            let _ = std::fs::remove_file(&src);
            true
        } else {
            false
        };
        if ok {
            // Bulletproof: stamp the origin mtime onto the landed file even if a
            // cross-device copy reset it, so its signature matches every member.
            stamp_mtime(&dest, in_mtime);
            // Remember it (by signature) so a watcher won't beam it back, now or
            // after a restart.
            if let Some(sig) = file_sig(&dest_str, folder_str) {
                inbound.lock().unwrap().insert(sig);
            }
            moved.push(dest_str);
        }
    }
    moved
}

/// A file's modified-time as whole seconds since the epoch (0 if unavailable).
fn meta_mtime(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stamp a file's modified-time to a specific epoch-seconds value (no-op on 0).
fn stamp_mtime(path: &Path, secs: u64) {
    if secs == 0 {
        return;
    }
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(when);
    }
}

/// A deterministic content hash (sha256 hex) for breaking same-second conflict
/// ties identically on every member. Empty string on read error (sorts lowest).
fn content_hash(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let mut h = Sha256::new();
    h.update(&bytes);
    hex::encode(h.finalize())
}

/// Compute a file's path relative to the folder (forward-slashed, so the same
/// key is used on both peers). Works even when the file is already gone.
fn rel_path_of(abs: &str, folder: &str) -> Option<String> {
    let p = Path::new(abs);
    let norm = |r: &Path| r.to_string_lossy().replace('\\', "/");
    if let Ok(rel) = p.strip_prefix(folder) {
        return Some(norm(rel));
    }
    if let Ok(canon_folder) = std::fs::canonicalize(folder) {
        if let Ok(rel) = p.strip_prefix(&canon_folder) {
            return Some(norm(rel));
        }
    }
    None
}

/// Drop loop-guard entries older than the debounce window.
fn prune_self_deleted(map: &mut HashMap<String, Instant>) {
    let now = Instant::now();
    map.retain(|_, t| now.duration_since(*t) < Duration::from_secs(30));
}

/// Apply a delete the peer made (mirror mode): move the file to history (so it's
/// recoverable) and mark it self-deleted so our watcher doesn't echo it back.
/// Returns true if a file was archived to history.
/// Apply a delete the peer propagated. `rel` may name a FILE or a DIRECTORY (when
/// the peer removed a whole folder). Every file removed is archived to history
/// first (so nothing is unrecoverable) and recorded in `self_deleted` (loop
/// guard) and `applied` (the rels we actually removed, for tombstoning). Returns
/// true if anything was removed.
fn apply_remote_delete(
    folder: &str,
    rel: &str,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
    applied: &mut Vec<String>,
) -> bool {
    let rel_norm = rel.replace('\\', "/");
    // Never let a peer reach outside the folder.
    if rel_norm.is_empty() || rel_norm.starts_with('/') || rel_norm.split('/').any(|c| c == "..") {
        return false;
    }
    let dest = Path::new(folder).join(&rel_norm);
    let mut removed = false;
    if dest.is_file() {
        self_deleted
            .lock()
            .unwrap()
            .insert(rel_norm.clone(), Instant::now());
        // archive MOVES the file out; if it fails, the file stays (no data loss).
        if crate::folder_history::archive(folder, &dest.to_string_lossy(), &rel_norm, "deleted") {
            applied.push(rel_norm.clone());
            removed = true;
        }
    } else if dest.is_dir() {
        // A whole folder was removed on the peer. Archive + remove every file
        // inside (recording each child rel), then prune the now-empty tree.
        for child_abs in list_files_rec(&dest) {
            let Some(child_rel) = rel_path_of(&child_abs.to_string_lossy(), folder) else {
                continue;
            };
            self_deleted
                .lock()
                .unwrap()
                .insert(child_rel.clone(), Instant::now());
            if crate::folder_history::archive(
                folder,
                &child_abs.to_string_lossy(),
                &child_rel,
                "deleted",
            ) {
                applied.push(child_rel);
                removed = true;
            }
        }
        self_deleted
            .lock()
            .unwrap()
            .insert(rel_norm.clone(), Instant::now());
        let _ = std::fs::remove_dir_all(&dest);
        removed = true;
    }
    // Tidy up: drop any now-empty parent directories up to (but not including)
    // the shared-folder root, so a removed subtree doesn't leave hollow folders.
    prune_empty_dirs(folder, &rel_norm);
    removed
}

/// Remove empty ancestor directories of `rel` within `folder` (never the root).
fn prune_empty_dirs(folder: &str, rel: &str) {
    let mut cur = Path::new(rel).parent().map(|p| p.to_path_buf());
    while let Some(dir) = cur {
        if dir.as_os_str().is_empty() {
            break;
        }
        let abs = Path::new(folder).join(&dir);
        if abs.is_dir() && std::fs::read_dir(&abs).map(|mut e| e.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&abs);
            cur = dir.parent().map(|p| p.to_path_buf());
        } else {
            break;
        }
    }
}

fn unique_dest(dest: PathBuf) -> PathBuf {
    if !dest.exists() {
        return dest;
    }
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = dest.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest
}

fn delete_local(file: &str, mode: DeleteMode) {
    match mode {
        DeleteMode::Trash => {
            let _ = trash::delete(file);
            // If trash silently no-op'd (some system locations), still remove the
            // local copy — the peer already has it, so the data is safe.
            if Path::new(file).exists() {
                let _ = std::fs::remove_file(file);
            }
        }
        DeleteMode::Permanent => {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn set_status(
    status: &Arc<Mutex<StatusSnapshot>>,
    state: FolderState,
    sending_file: Option<String>,
    percent: f64,
    detail: Option<String>,
) {
    if let Ok(mut s) = status.lock() {
        s.state = state;
        s.sending_file = sending_file;
        s.percent = percent;
        s.detail = detail;
        // A status transition (start/idle/waiting) clears stale progress metrics;
        // the live on_progress callback refills them during a transfer.
        s.bytes_done = 0;
        s.bytes_total = 0;
        s.speed_bps = 0.0;
        s.eta_seconds = None;
        s.locality = Locality::Unknown;
    }
}

fn set_peer_online(status: &Arc<Mutex<StatusSnapshot>>, online: bool) {
    if let Ok(mut s) = status.lock() {
        s.peer_online = online;
    }
}

async fn wait_backoff(stop_notify: &Arc<Notify>, stopped: &Arc<AtomicBool>, attempt: u32) -> bool {
    let secs = (2u64.saturating_pow(attempt.min(6))).min(MAX_BACKOFF_SECS).max(2);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => stopped.load(Ordering::SeqCst),
        _ = stop_notify.notified() => true,
    }
}

async fn wait_fixed(stop_notify: &Arc<Notify>, stopped: &Arc<AtomicBool>, ms: u64) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => stopped.load(Ordering::SeqCst),
        _ = stop_notify.notified() => true,
    }
}

/// A stable signature (relative path + size + mtime) used to remember files we've
/// already sent or received, so restarts don't re-send delivered files and a
/// two-way folder never beams a received file back.
fn file_sig(path: &str, folder: &str) -> Option<String> {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).ok()?;
    // Canonicalize both sides so symlinked roots (e.g. /tmp → /private/tmp on
    // macOS) and the configured folder path resolve to a matching relative path.
    let canon_p = std::fs::canonicalize(p).ok()?;
    let canon_folder = std::fs::canonicalize(folder).ok()?;
    let rel = canon_p
        .strip_prefix(&canon_folder)
        .ok()?
        .to_string_lossy()
        .to_string();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("{rel}|{}|{}", meta.len(), mtime))
}

/// Recover the relative path from a `rel|size|mtime` signature. Splits from the
/// RIGHT so a relative path that itself contains '|' is preserved intact.
fn sig_rel(sig: &str) -> Option<String> {
    let mut it = sig.rsplitn(3, '|');
    let _mtime = it.next()?;
    let _size = it.next()?;
    let rel = it.next()?;
    if rel.is_empty() {
        None
    } else {
        Some(rel.to_string())
    }
}

fn manifest_path(config_dir: &Path, pair_id: &str) -> PathBuf {
    config_dir.join(format!("synced-{pair_id}.json"))
}

fn load_manifest(config_dir: &Path, pair_id: &str) -> HashSet<String> {
    std::fs::read_to_string(manifest_path(config_dir, pair_id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_manifest(config_dir: &Path, pair_id: &str, set: &HashSet<String>) {
    if let Ok(txt) = serde_json::to_string(set) {
        let _ = std::fs::write(manifest_path(config_dir, pair_id), txt);
    }
}

// ── Tombstones ──────────────────────────────────────────────────────────────
// A tombstone records that `rel` was deleted at `ts` (ms). They travel on the
// reconcile beacon so a deletion converges across the group even if the live
// event was dropped, and they stop the peer's add-reconcile from resurrecting a
// file we deliberately removed. Pruned after a long TTL so the file can't grow
// without bound (by then both sides have long since converged).
const TOMBSTONE_TTL_MS: u64 = 45 * 24 * 3600 * 1000;

fn tombstones_path(config_dir: &Path, pair_id: &str) -> PathBuf {
    config_dir.join(format!("tombstones-{pair_id}.json"))
}

fn load_tombstones(config_dir: &Path, pair_id: &str) -> HashMap<String, u64> {
    std::fs::read_to_string(tombstones_path(config_dir, pair_id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_tombstones(config_dir: &Path, pair_id: &str, map: &HashMap<String, u64>) {
    let cutoff = now_ms().saturating_sub(TOMBSTONE_TTL_MS);
    let pruned: HashMap<&String, &u64> = map.iter().filter(|(_, &ts)| ts >= cutoff).collect();
    if let Ok(txt) = serde_json::to_string(&pruned) {
        let _ = std::fs::write(tombstones_path(config_dir, pair_id), txt);
    }
}

/// Record `rel` as deleted at `ts` (keeping the NEWEST timestamp), in memory and
/// on disk. No-op if an equal-or-newer tombstone already exists.
fn note_tombstone(
    config_dir: &Path,
    pair_id: &str,
    tomb: &Arc<Mutex<HashMap<String, u64>>>,
    rel: &str,
    ts: u64,
) {
    // Clamp an absurd FUTURE timestamp down to ~now. A peer with a wildly-ahead
    // clock (or a malicious one) could otherwise stamp a tombstone "newer than
    // everything forever" and have reconcile delete files the user still wants.
    // Clamping DOWN is the safe direction — at worst a legit delete from a very
    // fast clock won't reconcile-propagate (the live delete path still does), it
    // never causes an extra deletion.
    let ts = ts.min(now_ms().saturating_add(24 * 3600 * 1000));
    let mut changed = false;
    {
        let mut t = tomb.lock().unwrap();
        let e = t.entry(rel.to_string()).or_insert(0);
        if ts > *e {
            *e = ts;
            changed = true;
        }
    }
    if changed {
        let snapshot = tomb.lock().unwrap().clone();
        save_tombstones(config_dir, pair_id, &snapshot);
    }
}

/// Every relative file path currently present under `folder`, mapped to its
/// signature (`size|mtime`). The authoritative "what I actually have on disk".
fn live_manifest(folder: &str) -> HashMap<String, FileEntry> {
    let mut out = HashMap::new();
    for abs in list_files_rec(Path::new(folder)) {
        let p = abs.to_string_lossy().to_string();
        if p.ends_with(".dropbeam-incoming") {
            continue;
        }
        let Some(rel) = rel_path_of(&p, folder) else {
            continue;
        };
        if rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&abs) {
            out.insert(
                rel,
                FileEntry {
                    size: meta.len(),
                    mtime: meta_mtime(&meta),
                },
            );
        }
    }
    out
}

#[derive(Clone, Copy)]
struct FileEntry {
    size: u64,
    mtime: u64,
}

/// Relative paths of directories under `folder` that contain NO files anywhere
/// beneath them (a dir whose whole subtree is file-less). Skips dot-dirs and
/// symlinks. Used so empty organizing-folders still sync.
fn live_empty_dirs(folder: &str) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    // For every file, every ANCESTOR directory rel is "has files".
    let mut has_files: HashSet<String> = HashSet::new();
    let mut stack = vec![Path::new(folder).to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let Some(rel) = rel_path_of(&path.to_string_lossy(), folder) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            if ft.is_dir() {
                dirs.push(rel);
                stack.push(path);
            } else if ft.is_file() {
                // Mark every ancestor dir rel as containing files.
                let mut anc = Path::new(&rel).parent();
                while let Some(a) = anc {
                    let s = a.to_string_lossy().to_string();
                    if s.is_empty() {
                        break;
                    }
                    has_files.insert(s);
                    anc = a.parent();
                }
            }
        }
    }
    dirs.into_iter().filter(|d| !has_files.contains(d)).collect()
}

/// What a reconcile pass decided to do to OUR folder, given the peer's snapshot.
#[derive(Default, Debug, PartialEq)]
struct ReconcilePlan {
    /// Local rels to delete (the peer tombstoned them newer than our copy).
    delete: Vec<String>,
    /// Local rels to (re)send to the peer (it lacks them or has an older copy).
    push: Vec<String>,
}

/// THE data-loss-critical decision, isolated as a pure function so it can be
/// exhaustively unit-tested. Inputs: our live files, the peer's files + the
/// peer's tombstones + our own tombstones (all rel→ts in ms). Rules:
///   • DELETE a local file only when the peer has a tombstone for it strictly
///     newer than the file's mtime — an explicit deletion, never inferred from
///     mere absence. (A concurrent local EDIT newer than the tombstone wins.)
///   • PUSH a local file when the peer lacks it, or has an older copy — UNLESS
///     a tombstone (peer's OR ours) is newer than it (that's a pending delete).
/// `mtime` is epoch SECONDS; tombstone `ts` is epoch MILLIS (compared as ms).
fn reconcile_plan(
    mine: &HashMap<String, FileEntry>,
    peer_files: &HashMap<String, (u64, u64)>,
    peer_tombs: &HashMap<String, u64>,
    my_tombs: &HashMap<String, u64>,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    for (rel, entry) in mine {
        let file_ms = entry.mtime.saturating_mul(1000);
        let peer_t = peer_tombs.get(rel).copied().unwrap_or(0);
        let mine_t = my_tombs.get(rel).copied().unwrap_or(0);
        // A tombstone newer than our file means it should be gone.
        if peer_t > file_ms {
            plan.delete.push(rel.clone());
            continue;
        }
        if mine_t > file_ms {
            // We already know it's deleted locally-pending; don't push it.
            continue;
        }
        let need = match peer_files.get(rel) {
            None => true,
            Some(&(psize, pmtime)) => {
                (psize != entry.size || pmtime != entry.mtime) && entry.mtime > pmtime
            }
        };
        if need {
            plan.push.push(rel.clone());
        }
    }
    plan.delete.sort();
    plan.push.sort();
    plan
}

fn friend_sig(f: &Friend) -> String {
    // auto_accept is included so flipping it restarts the listener in the new mode.
    format!("{}|{:?}|{}|{}", f.id, f.role, f.secret, f.auto_accept)
}

fn structural_sig(p: &Pair) -> String {
    format!(
        "{}|{:?}|{}|{}|{}",
        p.folder, p.role, p.secret, p.two_way, p.mirror
    )
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn peer_label(pair: &Pair) -> String {
    if pair.peer_name.trim().is_empty() {
        "the other device".to_string()
    } else {
        pair.peer_name.clone()
    }
}

/// The group roster to advertise on a pair's control beacon: ourselves plus every
/// member of the folder group we already have a link to. Empty for a 1:1 folder.
fn build_group_roster(
    config_dir: &Path,
    pair: &Pair,
    ep: &iroh::Endpoint,
    my_name: &str,
) -> (String, Vec<(String, String)>) {
    let Some(gid) = pair.group_id.clone() else {
        return (String::new(), Vec::new());
    };
    let mut roster: Vec<(String, String)> = vec![(ep.id().to_string(), my_name.to_string())];
    for p in pairing::members_of_group(config_dir, &gid) {
        if let Some(eid) = &p.endpoint_id {
            let n = if p.peer_name.trim().is_empty() {
                "Member".to_string()
            } else {
                p.peer_name.clone()
            };
            roster.push((eid.clone(), n));
        }
    }
    (gid, roster)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "dropbeam-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_with_mtime(path: &Path, content: &[u8], mtime: u64) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, content).unwrap();
        stamp_mtime(path, mtime);
    }

    fn apply(staging: &Path, folder: &Path) -> Vec<String> {
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let self_del = Arc::new(Mutex::new(HashMap::new()));
        move_staged_into_folder(
            staging,
            &folder.to_string_lossy(),
            &inbound,
            true, // mirror
            true, // group → deterministic resolution
            &self_del,
        )
    }

    #[test]
    fn group_apply_is_idempotent_for_identical_files() {
        // A re-received byte-identical file (same content + preserved mtime) must
        // be a NO-OP — this is what stops a mesh sync storm.
        let folder = temp_dir("idem-f");
        let staging = temp_dir("idem-s");
        write_with_mtime(&folder.join("a.txt"), b"hello", 1000);
        write_with_mtime(&staging.join("a.txt"), b"hello", 1000);
        let moved = apply(&staging, &folder);
        assert!(moved.is_empty(), "identical file should not be rewritten");
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"hello");
        assert!(!staging.join("a.txt").exists(), "staged dup consumed");
    }

    #[test]
    fn group_conflict_newest_mtime_wins() {
        let folder = temp_dir("win-f");
        let staging = temp_dir("win-s");
        write_with_mtime(&folder.join("a.txt"), b"old", 1000);
        write_with_mtime(&staging.join("a.txt"), b"new", 2000);
        apply(&staging, &folder);
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"new");
    }

    #[test]
    fn group_conflict_older_incoming_is_rejected() {
        // Every member must converge to the SAME winner, so an older incoming
        // version never clobbers a newer local one (it's archived instead).
        let folder = temp_dir("rej-f");
        let staging = temp_dir("rej-s");
        write_with_mtime(&folder.join("a.txt"), b"newer", 2000);
        write_with_mtime(&staging.join("a.txt"), b"older", 1000);
        apply(&staging, &folder);
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"newer");
        assert!(!staging.join("a.txt").exists());
    }

    #[test]
    fn one_to_one_mirror_keeps_arrival_wins() {
        // A classic 1:1 (non-group) mirror folder must be UNCHANGED: the incoming
        // file always replaces local (arrival-wins), regardless of mtime — so the
        // new group logic can't regress shipped 1:1 folders via clock skew.
        let folder = temp_dir("1to1-f");
        let staging = temp_dir("1to1-s");
        write_with_mtime(&folder.join("a.txt"), b"local-newer", 2000);
        write_with_mtime(&staging.join("a.txt"), b"incoming-older", 1000);
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let self_del = Arc::new(Mutex::new(HashMap::new()));
        move_staged_into_folder(
            &staging,
            &folder.to_string_lossy(),
            &inbound,
            true,  // mirror
            false, // NOT a group → old arrival-wins behavior
            &self_del,
        );
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"incoming-older");
    }

    #[test]
    fn group_same_second_different_content_not_dropped() {
        // Same size + same second but DIFFERENT bytes must not be silently dropped
        // as "identical" — the content check forces a real (deterministic) resolve.
        let folder = temp_dir("ss-f");
        let staging = temp_dir("ss-s");
        write_with_mtime(&folder.join("a.txt"), b"AAAAA", 1500);
        write_with_mtime(&staging.join("a.txt"), b"BBBBB", 1500);
        apply(&staging, &folder);
        let content = std::fs::read(folder.join("a.txt")).unwrap();
        assert!(content == b"AAAAA" || content == b"BBBBB");
        assert!(!staging.join("a.txt").exists());
    }

    #[test]
    fn rel_path_of_strips_folder() {
        assert_eq!(
            rel_path_of("/a/b/c/d.txt", "/a/b").as_deref(),
            Some("c/d.txt")
        );
        assert_eq!(rel_path_of("/a/b/x.txt", "/a/b").as_deref(), Some("x.txt"));
        assert_eq!(rel_path_of("/x/y.txt", "/a/b"), None);
    }

    #[test]
    fn mirror_delete_archives_to_history_and_restores() {
        let folder = temp_dir("del");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("sub/x.txt"), b"hello world").unwrap();

        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut applied = Vec::new();
        let archived = apply_remote_delete(&folder_s, "sub/x.txt", &sd, &mut applied);
        assert!(archived, "a file should have been archived");
        assert_eq!(applied, vec!["sub/x.txt".to_string()]);
        assert!(!folder.join("sub/x.txt").exists(), "original is gone");
        assert!(
            sd.lock().unwrap().contains_key("sub/x.txt"),
            "loop guard recorded"
        );

        let hist = crate::folder_history::load(&folder_s);
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].rel_path, "sub/x.txt");
        assert_eq!(hist[0].reason, "deleted");

        let restored = crate::folder_history::restore(&folder_s, &hist[0].id).unwrap();
        assert!(Path::new(&restored).exists(), "restored file exists");
        assert_eq!(std::fs::read(&restored).unwrap(), b"hello world");
        assert!(
            crate::folder_history::load(&folder_s).is_empty(),
            "history entry consumed on restore"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn mirror_delete_rejects_escape_paths() {
        let folder = temp_dir("trav");
        let folder_s = folder.to_string_lossy().to_string();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        assert!(!apply_remote_delete(&folder_s, "../escape.txt", &sd, &mut ap));
        assert!(!apply_remote_delete(&folder_s, "/etc/passwd", &sd, &mut ap));
        assert!(!apply_remote_delete(&folder_s, "a/../../b.txt", &sd, &mut ap));
        assert!(ap.is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn sig_rel_recovers_path_even_with_pipes() {
        assert_eq!(sig_rel("a/b.txt|123|456").as_deref(), Some("a/b.txt"));
        assert_eq!(sig_rel("weird|name.txt|0|99").as_deref(), Some("weird|name.txt"));
        assert_eq!(sig_rel("noseps"), None);
    }

    #[test]
    fn deleting_a_directory_removes_every_file_inside_on_the_peer() {
        // THE BUG: deleting a subfolder left all its files on the peer because
        // apply_remote_delete could only remove an EMPTY dir. Now it recursively
        // archives + removes the whole tree and reports each child rel.
        let folder = temp_dir("dirdel");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("clips/raw")).unwrap();
        std::fs::write(folder.join("clips/a.mov"), b"a").unwrap();
        std::fs::write(folder.join("clips/raw/b.mov"), b"b").unwrap();
        std::fs::write(folder.join("keep.txt"), b"k").unwrap();

        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut applied = Vec::new();
        let removed = apply_remote_delete(&folder_s, "clips", &sd, &mut applied);
        assert!(removed, "the directory delete should report removal");
        assert!(!folder.join("clips").exists(), "whole subtree gone");
        assert!(folder.join("keep.txt").exists(), "sibling untouched");
        applied.sort();
        assert_eq!(applied, vec!["clips/a.mov".to_string(), "clips/raw/b.mov".to_string()]);
        // Both removed files are recoverable from history.
        assert_eq!(crate::folder_history::load(&folder_s).len(), 2);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn prune_empty_dirs_collapses_emptied_tree_but_keeps_root() {
        let folder = temp_dir("prune");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("a/b/c")).unwrap();
        prune_empty_dirs(&folder_s, "a/b/c/gone.txt");
        assert!(!folder.join("a").exists(), "empty tree collapsed up");
        assert!(folder.exists(), "root folder preserved");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn reconcile_plan_pushes_missing_and_never_deletes_without_a_tombstone() {
        let mut mine = HashMap::new();
        mine.insert("have.txt".into(), FileEntry { size: 1, mtime: 100 });
        mine.insert("alsohave.txt".into(), FileEntry { size: 2, mtime: 100 });
        // Peer has neither → both must be pushed; NOTHING deleted (no tombstones).
        let plan = reconcile_plan(&mine, &HashMap::new(), &HashMap::new(), &HashMap::new());
        assert_eq!(plan.push, vec!["alsohave.txt".to_string(), "have.txt".to_string()]);
        assert!(plan.delete.is_empty(), "absence alone must NEVER cause a delete");
    }

    #[test]
    fn reconcile_plan_applies_a_newer_tombstone_as_a_delete() {
        let mut mine = HashMap::new();
        mine.insert("old.mov".into(), FileEntry { size: 9, mtime: 100 }); // mtime 100s
        let mut peer_tombs = HashMap::new();
        peer_tombs.insert("old.mov".to_string(), 200_000u64); // deleted at 200s (ms)
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new());
        assert_eq!(plan.delete, vec!["old.mov".to_string()]);
        assert!(plan.push.is_empty(), "a tombstoned file is never pushed");
    }

    #[test]
    fn reconcile_plan_local_edit_newer_than_tombstone_wins() {
        // We edited the file AFTER the peer's delete → keep + push (edit beats delete).
        let mut mine = HashMap::new();
        mine.insert("doc.txt".into(), FileEntry { size: 9, mtime: 300 }); // edited at 300s
        let mut peer_tombs = HashMap::new();
        peer_tombs.insert("doc.txt".to_string(), 200_000u64); // delete at 200s
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new());
        assert!(plan.delete.is_empty(), "newer local edit must survive the delete");
        assert_eq!(plan.push, vec!["doc.txt".to_string()]);
    }

    #[test]
    fn tombstone_future_timestamp_is_clamped() {
        let dir = temp_dir("tomb");
        let tomb = Arc::new(Mutex::new(HashMap::new()));
        // A peer claims a delete a century in the future → must be clamped to ~now,
        // so it can't sit "newer than everything forever" and delete wanted files.
        let absurd = now_ms() + 100 * 365 * 24 * 3600 * 1000;
        note_tombstone(&dir, "p", &tomb, "x.mov", absurd);
        let stored = *tomb.lock().unwrap().get("x.mov").unwrap();
        assert!(stored <= now_ms() + 24 * 3600 * 1000 + 1000, "clamped to ~now+1day");
        // A normal recent timestamp is kept as-is.
        let normal = now_ms().saturating_sub(5000);
        note_tombstone(&dir, "p", &tomb, "y.mov", normal);
        assert_eq!(*tomb.lock().unwrap().get("y.mov").unwrap(), normal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_files_rec_skips_symlinks() {
        let dir = temp_dir("syml");
        std::fs::write(dir.join("real.txt"), b"r").unwrap();
        let outside = temp_dir("syml-ext");
        std::fs::write(outside.join("secret.txt"), b"s").unwrap();
        // A symlink inside the folder pointing OUT must not be walked.
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&outside, dir.join("link"));
            let files = list_files_rec(&dir);
            let names: Vec<String> =
                files.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
            assert!(names.contains(&"real.txt".to_string()));
            assert!(
                !names.iter().any(|n| n == "secret.txt"),
                "must not follow the symlink to external files: {names:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn live_empty_dirs_finds_only_fileless_directories() {
        let folder = temp_dir("emptydirs");
        let fs = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("empty")).unwrap();
        std::fs::create_dir_all(folder.join("has/sub")).unwrap();
        std::fs::write(folder.join("has/sub/f.txt"), b"x").unwrap();
        std::fs::create_dir_all(folder.join("outer/inner")).unwrap(); // both empty of files
        let mut got = live_empty_dirs(&fs);
        got.sort();
        assert_eq!(
            got,
            vec!["empty".to_string(), "outer".to_string(), "outer/inner".to_string()],
            "only truly file-less dirs; 'has' and 'has/sub' contain a file"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn reconcile_plan_identical_files_do_nothing() {
        let mut mine = HashMap::new();
        mine.insert("same.txt".into(), FileEntry { size: 5, mtime: 100 });
        let mut peer = HashMap::new();
        peer.insert("same.txt".to_string(), (5u64, 100u64));
        let plan = reconcile_plan(&mine, &peer, &HashMap::new(), &HashMap::new());
        assert!(plan.push.is_empty() && plan.delete.is_empty(), "no-op when in sync");
    }

    #[test]
    fn history_is_a_dotdir_so_it_never_syncs() {
        // The history dir must start with '.' so is_sendable_candidate skips it.
        let folder = temp_dir("hist");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("a.txt"), b"x").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        apply_remote_delete(&folder_s, "a.txt", &sd, &mut ap);
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        for f in list_files_rec(&folder) {
            let p = f.to_string_lossy().to_string();
            assert!(
                !is_sendable_candidate(&p, &folder_s, &inbound),
                "history file must not be sendable: {p}"
            );
        }
        let _ = std::fs::remove_dir_all(&folder);
    }
}
