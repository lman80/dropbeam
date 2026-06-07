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
        }
    }
}

/// A removal to propagate to the mirror peer.
#[derive(Clone)]
struct DeleteEvent {
    rel: String,
    ts: u64,
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

        self.reconcile_friends();
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
    }

    fn stop_friend(&self, id: &str) {
        if let Some(h) = self.friend_handles.lock().unwrap().remove(id) {
            h.stopped.store(true, Ordering::SeqCst);
            h.stop_notify.notify_waiters();
        }
    }

    /// Current status snapshots for all active folders (for initial UI load).
    pub fn statuses(&self) -> Vec<FolderStatus> {
        let handles = self.handles.lock().unwrap();
        handles
            .iter()
            .map(|(id, h)| {
                let queued = h.queue.lock().unwrap().len();
                let s = h.status.lock().unwrap().clone();
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
                    peer_name: s.peer_name,
                    locality: s.locality,
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
    ) {
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
                    } else if mirror {
                        // ── DELETE (total-sync only) ──────────────────────────
                        let Some(rel) = rel_path_of(&p, &folder2) else {
                            return;
                        };
                        if rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
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
                        // re-check. If the file came back, it was a save (its own
                        // create event re-sends it) — not a real delete.
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                        if stopped2.load(Ordering::SeqCst) || Path::new(&p).is_file() {
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
                        {
                            let mut pend = pd.lock().unwrap();
                            if !pend.iter().any(|d| d.rel == rel) {
                                pend.push(DeleteEvent { rel: rel.clone(), ts: now_ms() });
                            }
                        }
                        cw.notify_one();
                    }
                });
            }
        });
    }

    fn spawn_sender(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        wake: Arc<Notify>,
        queue: Arc<Mutex<VecDeque<String>>>,
        inbound: Arc<Mutex<HashSet<String>>>,
        status: Arc<Mutex<StatusSnapshot>>,
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

                let peer_online = status.lock().map(|s| s.peer_online).unwrap_or(true);

                // Direct engine: when the peer is reachable and we know their iroh
                // key, push straight over iroh (fast, often LAN-direct).
                let iroh_loc = if peer_online {
                    manager
                        .try_iroh_folder_send(&pair, &settings, &file, &status, &stopped)
                        .await
                } else {
                    None
                };

                // iroh-only: deliver over iroh, else keep the file queued. Offline
                // → wait for the peer (presence flips via the iroh control beacon);
                // online but the push failed → quick retry. The file is only ever
                // popped from the queue on confirmed delivery.
                let result = if iroh_loc.is_some() {
                    SendOutcome::Delivered
                } else if peer_online {
                    SendOutcome::Failed("direct folder push failed".into())
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

                // Prefer iroh: dial the peer directly and hand them the control
                // payload. Success means they're online AND received it (deletes
                // included), so we skip croc entirely this round.
                let iroh_ok = if settings.direct_mode {
                    match (pair.endpoint_id.clone(), manager.iroh_endpoint()) {
                        (Some(eid), Some(ep)) => {
                            let del_pairs: Vec<(String, u64)> =
                                dels.iter().map(|d| (d.rel.clone(), d.ts)).collect();
                            crate::iroh_net::send_folder_ctrl(
                                &ep, &eid, &pair_id, &my_name, &del_pairs,
                            )
                            .await
                            .is_ok()
                        }
                        _ => false,
                    }
                } else {
                    false
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

    fn on_peer_hello(
        self: &Arc<Self>,
        pair_id: &str,
        config: &Arc<Mutex<Pair>>,
        name: &str,
        status: &Arc<Mutex<StatusSnapshot>>,
    ) {
        if let Ok(mut s) = status.lock() {
            s.peer_online = true;
            s.peer_name = Some(name.to_string());
        }
        let changed = pairing::set_peer_name(&self.config_dir, pair_id, name);
        if changed {
            let (secret, role) = {
                let mut p = config.lock().unwrap();
                p.peer_name = name.to_string();
                (p.secret.clone(), p.role)
            };
            friends::upsert_from_pairing(&self.config_dir, name, &secret, role);
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
    ) {
        let (config, status, self_deleted) = {
            let handles = self.handles.lock().unwrap();
            let Some(h) = handles.get(pair_id) else {
                return; // not a folder we're actively managing
            };
            (h.config.clone(), h.status.clone(), h.self_deleted.clone())
        };
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
        if mirror && !deletes.is_empty() {
            let mut applied_any = false;
            for (rel, _ts) in deletes {
                if apply_remote_delete(&folder, rel, &self_deleted) {
                    applied_any = true;
                }
            }
            if applied_any {
                let _ = self.app.emit("folder-history://changed", pair_id);
            }
        }
    }

    fn persist_manifest(&self, pair_id: &str, set: &HashSet<String>) {
        save_manifest(&self.config_dir, pair_id, set);
    }

    fn note_received(&self, pair: &Pair, files: &[String]) {
        use crate::history;
        use crate::models::{Direction, HistoryEntry, Locality, TransferState};
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
        let settings_notify = self
            .app
            .try_state::<Arc<AppState>>()
            .map(|s| s.settings.lock().unwrap().notify_on_complete)
            .unwrap_or(true);
        if settings_notify {
            use tauri_plugin_notification::NotificationExt;
            let body = if names.len() == 1 {
                format!("{} arrived in {}", names[0], folder_name(&pair.folder))
            } else {
                format!("{} files arrived in {}", names.len(), folder_name(&pair.folder))
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
        let (folder, mirror) = {
            let p = config.lock().unwrap();
            (p.folder.clone(), p.mirror)
        };
        let moved = move_staged_into_folder(staging, &folder, &inbound, mirror, &self_deleted);
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
        settings: &Settings,
        file: &str,
        status: &Arc<Mutex<StatusSnapshot>>,
        stopped: &Arc<AtomicBool>,
    ) -> Option<Locality> {
        if !settings.direct_mode {
            return None;
        }
        let eid = pair.endpoint_id.clone()?;
        let ep = self.iroh_endpoint()?;
        let paths = vec![PathBuf::from(file)];
        let cb = {
            let mgr = self.clone();
            let status = status.clone();
            let pair_id = pair.id.clone();
            let last = Arc::new(AtomicU64::new(0));
            move |done: u64, total: u64| {
                // Throttle to ~1% steps so we don't flood the UI per chunk.
                let permille = if total > 0 { done * 1000 / total } else { 0 };
                if done < total && permille <= last.load(Ordering::Relaxed) {
                    return;
                }
                last.store(permille, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    s.state = FolderState::Sending;
                    s.bytes_done = done;
                    if total > 0 {
                        s.bytes_total = total;
                        s.percent = done as f64 / total as f64 * 100.0;
                    }
                }
                mgr.emit_status(&pair_id);
            }
        };
        match crate::iroh_net::send_folder_file(
            &ep,
            &eid,
            &pair.id,
            &pair.folder,
            &paths,
            &**stopped,
            cb,
        )
        .await
        {
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
            let queued = h.queue.lock().unwrap().len();
            let s = h.status.lock().unwrap().clone();
            (queued, s)
        };
        let (queued, s) = snapshot;
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
    if fname.ends_with(".crdownload") || fname.ends_with(".download") || fname.ends_with(".part") || fname.ends_with(".tmp") {
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
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
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
        // Mirror = source of truth: REPLACE an edited file (archiving the old
        // copy to history) instead of keeping a "file (1)" duplicate. Guard the
        // resulting Remove event so it isn't echoed back as a delete.
        let dest = if mirror {
            if dest_path.is_file() {
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
            // Remember it (by signature) so a two-way watcher won't beam it back,
            // now or after a restart.
            if let Some(sig) = file_sig(&dest_str, folder_str) {
                inbound.lock().unwrap().insert(sig);
            }
            moved.push(dest_str);
        }
    }
    moved
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
fn apply_remote_delete(
    folder: &str,
    rel: &str,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
) -> bool {
    let rel_norm = rel.replace('\\', "/");
    // Never let a peer reach outside the folder.
    if rel_norm.is_empty() || rel_norm.starts_with('/') || rel_norm.split('/').any(|c| c == "..") {
        return false;
    }
    let dest = Path::new(folder).join(&rel_norm);
    if dest.is_file() {
        self_deleted
            .lock()
            .unwrap()
            .insert(rel_norm.clone(), Instant::now());
        // archive MOVES the file out; if it fails, the file stays (no data loss).
        crate::folder_history::archive(folder, &dest.to_string_lossy(), &rel_norm, "deleted")
    } else if dest.is_dir() {
        self_deleted
            .lock()
            .unwrap()
            .insert(rel_norm.clone(), Instant::now());
        let _ = std::fs::remove_dir(&dest); // only succeeds if already empty
        false
    } else {
        false
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
        let archived = apply_remote_delete(&folder_s, "sub/x.txt", &sd);
        assert!(archived, "a file should have been archived");
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
        assert!(!apply_remote_delete(&folder_s, "../escape.txt", &sd));
        assert!(!apply_remote_delete(&folder_s, "/etc/passwd", &sd));
        assert!(!apply_remote_delete(&folder_s, "a/../../b.txt", &sd));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn history_is_a_dotdir_so_it_never_syncs() {
        // The history dir must start with '.' so is_sendable_candidate skips it.
        let folder = temp_dir("hist");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("a.txt"), b"x").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        apply_remote_delete(&folder_s, "a.txt", &sd);
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
