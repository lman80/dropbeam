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
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::croc::{croc_binary_path, ProgressMetrics};
use crate::models::{
    DeleteMode, Direction, FolderState, FolderStatus, Friend, Locality, Pair, Settings,
    TransferState, TransferUpdate,
};
use crate::{friends, pairing, AppState};

const STAGING_DIR: &str = ".dropbeam-incoming";
const CONNECT_TIMEOUT_SECS: u64 = 75;
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
        }
    }
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
            match existing {
                Some(s) if s == sig => {}
                Some(_) => {
                    self.stop_friend(&friend.id);
                    self.start_friend_listener(friend);
                }
                None => self.start_friend_listener(friend),
            }
        }
    }

    fn stop_friend(&self, id: &str) {
        if let Some(h) = self.friend_handles.lock().unwrap().remove(id) {
            h.stopped.store(true, Ordering::SeqCst);
            h.stop_notify.notify_waiters();
        }
    }

    /// A persistent listener on this friend's inbox channel.
    ///
    /// Auto-accept friends: files arrive and land in Downloads automatically.
    /// Manual friends: each incoming file first surfaces an Accept/Decline offer.
    /// Either way we emit `transfer://update` so the receive shows live on the
    /// Send & Receive page with the same progress the sender sees.
    fn start_friend_listener(self: &Arc<Self>, friend: Friend) {
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(Notify::new());
        let sig = friend_sig(&friend);
        let friend_id = friend.id.clone();

        let manager = self.clone();
        let stopped_t = stopped.clone();
        let stop_t = stop_notify.clone();
        tauri::async_runtime::spawn(async move {
            let staging = manager
                .config_dir
                .join(format!(".friend-inbox-{}", friend.id));
            let mut idle_streak: u32 = 0;
            loop {
                if stopped_t.load(Ordering::SeqCst) {
                    break;
                }
                let _ = std::fs::create_dir_all(&staging);
                clear_dir(&staging);

                let settings = manager
                    .app
                    .try_state::<Arc<AppState>>()
                    .map(|st| st.settings.lock().unwrap().clone())
                    .unwrap_or_default();
                let code = friends::my_inbox_code(&friend);

                // One id for this receive attempt, shared by offer/progress/complete
                // so the UI updates a single card rather than spawning new ones.
                let tid = uuid::Uuid::new_v4().to_string();
                let started = Arc::new(AtomicBool::new(false));
                let names_cell = Arc::new(Mutex::new(Vec::<String>::new()));

                let on_progress = {
                    let app = manager.app.clone();
                    let tid = tid.clone();
                    let fname = friend.name.clone();
                    let started = started.clone();
                    let names_cell = names_cell.clone();
                    move |m: ProgressMetrics| {
                        started.store(true, Ordering::SeqCst);
                        let names = names_cell.lock().unwrap().clone();
                        let mut u = recv_update(&tid, &fname, TransferState::Transferring, &names);
                        u.percent = m.percent;
                        u.bytes_done = m.done;
                        if m.total > 0 {
                            u.bytes_total = m.total;
                        }
                        if let Some(s) = m.speed_bps {
                            u.speed_bps = s;
                        }
                        u.eta_seconds = m.eta;
                        let _ = app.emit("transfer://update", &u);
                    }
                };

                let outcome = if friend.auto_accept {
                    run_croc_receive(&settings, &code, &staging, &stopped_t, &stop_t, on_progress)
                        .await
                } else {
                    let on_offer = {
                        let app = manager.app.clone();
                        let tid = tid.clone();
                        let fname = friend.name.clone();
                        let names_cell = names_cell.clone();
                        move |names: Vec<String>, total: u64| {
                            *names_cell.lock().unwrap() = names.clone();
                            let app = app.clone();
                            let tid = tid.clone();
                            let fname = fname.clone();
                            async move {
                                let mut u =
                                    recv_update(&tid, &fname, TransferState::WaitingForAccept, &names);
                                u.bytes_total = total;
                                let _ = app.emit("transfer://update", &u);

                                let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                                if let Some(st) = app.try_state::<Arc<AppState>>() {
                                    st.offers.lock().unwrap().insert(tid.clone(), tx);
                                }
                                let accept = rx.await.unwrap_or(false);
                                if let Some(st) = app.try_state::<Arc<AppState>>() {
                                    st.offers.lock().unwrap().remove(&tid);
                                }
                                if !accept {
                                    let u =
                                        recv_update(&tid, &fname, TransferState::Canceled, &names);
                                    let _ = app.emit("transfer://update", &u);
                                }
                                accept
                            }
                        }
                    };
                    run_croc_receive_interactive(
                        &settings, &code, &staging, &stopped_t, &stop_t, on_offer, on_progress,
                    )
                    .await
                };

                match outcome {
                    ReceiveOutcome::Received => {
                        idle_streak = 0;
                        let dest = friend_download_dir(&settings);
                        let _ = std::fs::create_dir_all(&dest);
                        let throwaway = Arc::new(Mutex::new(HashSet::new()));
                        let moved = move_staged_into_folder(&staging, &dest, &throwaway);
                        if !moved.is_empty() {
                            let names: Vec<String> =
                                moved.iter().map(|f| file_name_of(f)).collect();
                            let total: u64 = moved
                                .iter()
                                .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
                                .sum();
                            let mut u =
                                recv_update(&tid, &friend.name, TransferState::Completed, &names);
                            u.bytes_total = total;
                            u.bytes_done = total;
                            u.percent = 100.0;
                            u.out_dir = Some(dest.clone());
                            let _ = manager.app.emit("transfer://update", &u);
                            manager.note_friend_received(&friend, &dest, &moved);
                        }
                    }
                    ReceiveOutcome::Stopped => break,
                    ReceiveOutcome::Error => {
                        // Only surface a failure if a transfer had actually started;
                        // an idle poll with no sender is normal and silent.
                        if started.load(Ordering::SeqCst) {
                            let names = names_cell.lock().unwrap().clone();
                            let mut u =
                                recv_update(&tid, &friend.name, TransferState::Failed, &names);
                            u.error = Some("The transfer didn't finish.".into());
                            let _ = manager.app.emit("transfer://update", &u);
                        }
                        idle_streak = idle_streak.saturating_add(1);
                        let gap = match idle_streak {
                            1 => 1200,
                            2 => 3000,
                            _ => 8000,
                        };
                        if wait_fixed(&stop_t, &stopped_t, gap).await {
                            break;
                        }
                    }
                }
            }
            clear_dir(&staging);
        });

        self.friend_handles.lock().unwrap().insert(
            friend_id,
            FriendHandle {
                sig,
                stopped,
                stop_notify,
            },
        );
    }

    fn note_friend_received(&self, friend: &Friend, dest: &str, files: &[String]) {
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
                peer: Some(friend.name.clone()),
                locality: Locality::Unknown,
                code: None,
                state: TransferState::Completed,
                timestamp_ms: now_ms(),
                error: None,
                out_dir: Some(dest.to_string()),
            },
        );
        let _ = self.app.emit("history://changed", ());
        let notify_on = self
            .app
            .try_state::<Arc<AppState>>()
            .map(|s| s.settings.lock().unwrap().notify_on_complete)
            .unwrap_or(true);
        if notify_on {
            use tauri_plugin_notification::NotificationExt;
            let body = if names.len() == 1 {
                format!("{} sent you {}", friend.name, names[0])
            } else {
                format!("{} sent you {} files", friend.name, names.len())
            };
            let _ = self
                .app
                .notification()
                .builder()
                .title("DropBeam")
                .body(body)
                .show();
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

        if pairing::runs_sender(&pair) {
            // Filesystem watcher → candidate channel.
            let (evt_tx, evt_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
            let folder = pair.folder.clone();
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    if matches!(
                        event.kind,
                        notify::EventKind::Create(_) | notify::EventKind::Modify(_)
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

            // Collector: debounce + size-stability → enqueue.
            self.clone().spawn_collector(
                evt_rx,
                config.clone(),
                stopped.clone(),
                wake.clone(),
                queue.clone(),
                inbound.clone(),
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

        if pairing::runs_listener(&pair) {
            self.clone().spawn_listener(
                config.clone(),
                stopped.clone(),
                stop_notify.clone(),
                inbound.clone(),
                status.clone(),
            );
        }

        // Control channel (presence + identity) runs for BOTH peers on every pair,
        // independent of file-sync direction — that's how the creator learns the
        // accepter exists + their name (fixing the stuck "waiting" state).
        self.clone().spawn_control_sender(
            config.clone(),
            stopped.clone(),
            stop_notify.clone(),
            status.clone(),
        );
        self.clone().spawn_control_listener(
            config.clone(),
            stopped.clone(),
            stop_notify.clone(),
            status.clone(),
        );

        let handle = PairHandle {
            sig,
            config,
            stopped,
            stop_notify,
            wake,
            queue,
            inbound,
            status,
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
    ) {
        let debounce: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        tauri::async_runtime::spawn(async move {
            while let Some(path) = evt_rx.recv().await {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let folder = config.lock().unwrap().folder.clone();
                let p = path.to_string_lossy().to_string();
                if !is_sendable_candidate(&p, &folder, &inbound) {
                    continue;
                }
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
                tauri::async_runtime::spawn(async move {
                    // Wait for quiet, then confirm write-completion via size stability.
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
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
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
                let code = pairing::outbound_code(&pair);
                let name = file_name_of(&file);

                set_status(&status, FolderState::Sending, Some(name.clone()), 0.0, None);
                manager.emit_status(&pair_id);

                let status_cb = status.clone();
                let pair_id_cb = pair_id.clone();
                let manager_cb = manager.clone();
                let result = run_croc_send(
                    &settings,
                    &code,
                    &file,
                    &stopped,
                    &stop_notify,
                    move |m| {
                        if let Ok(mut s) = status_cb.lock() {
                            s.state = FolderState::Sending;
                            s.percent = m.percent;
                            s.bytes_done = m.done;
                            if m.total > 0 {
                                s.bytes_total = m.total;
                            }
                            if let Some(spd) = m.speed_bps {
                                s.speed_bps = spd;
                            }
                            s.eta_seconds = m.eta;
                        }
                        manager_cb.emit_status(&pair_id_cb);
                    },
                )
                .await;

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

    fn spawn_listener(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        inbound: Arc<Mutex<HashSet<String>>>,
        status: Arc<Mutex<StatusSnapshot>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let folder = config.lock().unwrap().folder.clone();
            let staging = Path::new(&folder).join(STAGING_DIR);
            let mut idle_streak: u32 = 0;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let _ = std::fs::create_dir_all(&staging);
                clear_dir(&staging);

                let (pair, settings) = {
                    let p = config.lock().unwrap().clone();
                    let s = manager
                        .app
                        .try_state::<Arc<AppState>>()
                        .map(|st| st.settings.lock().unwrap().clone())
                        .unwrap_or_default();
                    (p, s)
                };
                let code = pairing::inbound_code(&pair);

                let manager_cb = manager.clone();
                let status_cb = status.clone();
                let pair_id_cb = pair_id.clone();
                let outcome = run_croc_receive(
                    &settings,
                    &code,
                    &staging,
                    &stopped,
                    &stop_notify,
                    move |m| {
                        if let Ok(mut s) = status_cb.lock() {
                            s.state = FolderState::Receiving;
                            s.sending_file = None;
                            s.percent = m.percent;
                            s.bytes_done = m.done;
                            if m.total > 0 {
                                s.bytes_total = m.total;
                            }
                            if let Some(spd) = m.speed_bps {
                                s.speed_bps = spd;
                            }
                            s.eta_seconds = m.eta;
                        }
                        manager_cb.emit_status(&pair_id_cb);
                    },
                )
                .await;

                match outcome {
                    ReceiveOutcome::Received => {
                        idle_streak = 0;
                        let folder = config.lock().unwrap().folder.clone();
                        let moved = move_staged_into_folder(&staging, &folder, &inbound);
                        if !moved.is_empty() {
                            let snapshot = inbound.lock().unwrap().clone();
                            manager.persist_manifest(&pair_id, &snapshot);
                            manager.note_received(&pair, &moved);
                        }
                        set_status(&status, FolderState::Idle, None, 0.0, None);
                        manager.emit_status(&pair_id);
                        // Loop straight back to drain any further files immediately.
                    }
                    ReceiveOutcome::Stopped => break,
                    ReceiveOutcome::Error => {
                        // Usually just "no sender waiting right now". Re-poll with an
                        // adaptive gap — quick at first, easing off to limit chatter.
                        idle_streak = idle_streak.saturating_add(1);
                        let gap = match idle_streak {
                            1 => 1000,
                            2 => 2500,
                            _ => 5000,
                        };
                        if wait_fixed(&stop_notify, &stopped, gap).await {
                            break;
                        }
                    }
                }
            }
            clear_dir(&staging);
        });
    }

    /// Periodically beam a tiny hello {name} to the peer on the control channel.
    /// Delivery means their control listener is up → they're online.
    fn spawn_control_sender(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        status: Arc<Mutex<StatusSnapshot>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let ctrl_file = manager.config_dir.join(format!(".ctrl-out-{pair_id}.json"));
            let ctrl_path = ctrl_file.to_string_lossy().to_string();
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
                let payload = serde_json::json!({ "v": 1, "name": my_name, "ts": now_ms() });
                let _ = std::fs::write(&ctrl_file, payload.to_string());
                let code = pairing::control_outbound_code(&pair);
                let outcome = run_croc_send(
                    &settings,
                    &code,
                    &ctrl_path,
                    &stopped,
                    &stop_notify,
                    |_m| {},
                )
                .await;
                match outcome {
                    SendOutcome::Delivered => {
                        set_peer_online(&status, true);
                        manager.emit_status(&pair_id);
                        // Refresh presence periodically.
                        if wait_fixed(&stop_notify, &stopped, 30_000).await {
                            break;
                        }
                    }
                    SendOutcome::Offline => {
                        set_peer_online(&status, false);
                        manager.emit_status(&pair_id);
                        if wait_backoff(&stop_notify, &stopped, 3).await {
                            break;
                        }
                    }
                    SendOutcome::Failed(_) => {
                        if wait_fixed(&stop_notify, &stopped, 3000).await {
                            break;
                        }
                    }
                    SendOutcome::Stopped => break,
                }
            }
            let _ = std::fs::remove_file(&ctrl_file);
        });
    }

    /// Listen for the peer's hello → learn their name, mark them online, and link
    /// them as a friend (folder partners are always friends).
    fn spawn_control_listener(
        self: Arc<Self>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        stop_notify: Arc<Notify>,
        status: Arc<Mutex<StatusSnapshot>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let staging = manager.config_dir.join(format!(".ctrl-in-{pair_id}"));
            let mut idle_streak: u32 = 0;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let _ = std::fs::create_dir_all(&staging);
                clear_dir(&staging);

                let (pair, settings) = {
                    let p = config.lock().unwrap().clone();
                    let s = manager
                        .app
                        .try_state::<Arc<AppState>>()
                        .map(|st| st.settings.lock().unwrap().clone())
                        .unwrap_or_default();
                    (p, s)
                };
                let code = pairing::control_inbound_code(&pair);
                let outcome =
                    run_croc_receive(&settings, &code, &staging, &stopped, &stop_notify, |_m| {})
                        .await;
                match outcome {
                    ReceiveOutcome::Received => {
                        idle_streak = 0;
                        // Read the staging dir directly — the hello arrives as a
                        // dotfile, which list_files_rec would skip.
                        if let Ok(entries) = std::fs::read_dir(&staging) {
                            for e in entries.flatten() {
                                let p = e.path();
                                if !p.is_file() {
                                    continue;
                                }
                                if let Ok(txt) = std::fs::read_to_string(&p) {
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                                        if let Some(name) =
                                            v.get("name").and_then(|n| n.as_str()).map(|s| s.trim())
                                        {
                                            if !name.is_empty() {
                                                manager.on_peer_hello(
                                                    &pair_id, &config, name, &status,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        clear_dir(&staging);
                    }
                    ReceiveOutcome::Stopped => break,
                    ReceiveOutcome::Error => {
                        // Presence/identity isn't latency-critical — poll gently to
                        // keep background croc churn low.
                        idle_streak = idle_streak.saturating_add(1);
                        let gap = match idle_streak {
                            1 => 2000,
                            2 => 7000,
                            _ => 15000,
                        };
                        if wait_fixed(&stop_notify, &stopped, gap).await {
                            break;
                        }
                    }
                }
            }
            clear_dir(&staging);
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

enum ReceiveOutcome {
    Received,
    Stopped,
    Error,
}

fn apply_relay(cmd: &mut Command, settings: &Settings) {
    if !settings.custom_relay.trim().is_empty() {
        cmd.arg("--relay").arg(settings.custom_relay.trim());
        if !settings.custom_relay_pass.trim().is_empty() {
            cmd.arg("--pass").arg(settings.custom_relay_pass.trim());
        }
    }
}

async fn run_croc_send(
    settings: &Settings,
    code: &str,
    file: &str,
    stopped: &Arc<AtomicBool>,
    stop_notify: &Arc<Notify>,
    on_progress: impl Fn(crate::croc::ProgressMetrics) + Send + 'static,
) -> SendOutcome {
    let bin = croc_binary_path();
    let mut cmd = Command::new(&bin);
    cmd.env("CROC_SECRET", code).env("NO_COLOR", "1");
    cmd.arg("--ignore-stdin");
    apply_relay(&mut cmd, settings);
    cmd.arg("--disable-clipboard").arg("send").arg(file);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return SendOutcome::Failed(format!("couldn't start croc: {e}")),
    };
    let mut stderr = child.stderr.take().expect("stderr piped");

    let connect_deadline = Instant::now() + Duration::from_secs(CONNECT_TIMEOUT_SECS);
    let mut connected = false;
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            n = stderr.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if b == b'\r' || b == b'\n' {
                                if let Some(m) =
                                    crate::croc::parse_progress_metrics(&String::from_utf8_lossy(&line))
                                {
                                    connected = true;
                                    on_progress(m);
                                }
                                line.clear();
                            } else {
                                line.push(b);
                            }
                        }
                    }
                }
            }
            _ = sleep_until(connect_deadline), if !connected => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return SendOutcome::Offline;
            }
            _ = stop_notify.notified() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return SendOutcome::Stopped;
            }
        }
    }

    if stopped.load(Ordering::SeqCst) {
        let _ = child.wait().await;
        return SendOutcome::Stopped;
    }
    match child.wait().await {
        Ok(s) if s.success() => SendOutcome::Delivered,
        Ok(_) => SendOutcome::Failed("transfer did not complete".into()),
        Err(e) => SendOutcome::Failed(e.to_string()),
    }
}

async fn run_croc_receive(
    settings: &Settings,
    code: &str,
    out_dir: &Path,
    stopped: &Arc<AtomicBool>,
    stop_notify: &Arc<Notify>,
    on_progress: impl Fn(crate::croc::ProgressMetrics) + Send + 'static,
) -> ReceiveOutcome {
    let bin = croc_binary_path();
    let mut cmd = Command::new(&bin);
    cmd.env("CROC_SECRET", code).env("NO_COLOR", "1");
    cmd.arg("--ignore-stdin")
        .arg("--yes")
        .arg("--overwrite");
    apply_relay(&mut cmd, settings);
    cmd.arg("--out").arg(out_dir);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return ReceiveOutcome::Error,
    };
    let mut stderr = child.stderr.take().expect("stderr piped");
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            n = stderr.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if b == b'\r' || b == b'\n' {
                                if let Some(m) =
                                    crate::croc::parse_progress_metrics(&String::from_utf8_lossy(&line))
                                {
                                    on_progress(m);
                                }
                                line.clear();
                            } else {
                                line.push(b);
                            }
                        }
                    }
                }
            }
            _ = stop_notify.notified() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return ReceiveOutcome::Stopped;
            }
        }
    }

    if stopped.load(Ordering::SeqCst) {
        let _ = child.wait().await;
        return ReceiveOutcome::Stopped;
    }
    match child.wait().await {
        Ok(s) if s.success() => ReceiveOutcome::Received,
        _ => ReceiveOutcome::Error,
    }
}

/// Build a receive-side TransferUpdate tagged with the friend's name so the UI
/// shows "from {name}". Callers fill in progress/size fields as needed.
fn recv_update(id: &str, friend_name: &str, state: TransferState, names: &[String]) -> TransferUpdate {
    let mut u = TransferUpdate::new(id.to_string(), Direction::Receive, names.to_vec());
    u.state = state;
    u.friend_name = Some(friend_name.to_string());
    u.peer = Some(friend_name.to_string());
    u.locality = Locality::Unknown;
    u
}

// croc's accept prompt, e.g. "Accept 'photo.jpg' (1.2 MB)? (Y/n)" or
// "Accept 3 files (10.5 MB)? (Y/n)". Written WITHOUT a trailing newline.
static RE_ACCEPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Accept\s+(?:'(?P<name>[^']*)'|(?P<count>\d+)\s+files?)\s+\((?P<size>[^)]+)\)\s*\?")
        .unwrap()
});

fn parse_accept_prompt(s: &str) -> Option<(Vec<String>, u64)> {
    let c = RE_ACCEPT.captures(s)?;
    let names = if let Some(n) = c.name("name") {
        vec![n.as_str().to_string()]
    } else {
        let count = c.name("count").map(|m| m.as_str()).unwrap_or("0");
        vec![format!("{count} files")]
    };
    let size = c.name("size").map(|m| m.as_str().trim()).unwrap_or("");
    let mut it = size.split_whitespace();
    let val = it.next().unwrap_or("0");
    let unit = it.next().unwrap_or("");
    Some((names, crate::croc::parse_bytes(val, unit)))
}

/// Like `run_croc_receive`, but for a manual-accept friend: croc runs without
/// `--yes`, so it pauses at an Accept prompt (indefinitely, while stdin stays
/// open). We surface the offer via `on_offer` (which resolves to the user's
/// accept/decline), write the answer to croc's stdin, then stream progress.
async fn run_croc_receive_interactive<F, Fut>(
    settings: &Settings,
    code: &str,
    out_dir: &Path,
    stopped: &Arc<AtomicBool>,
    stop_notify: &Arc<Notify>,
    on_offer: F,
    on_progress: impl Fn(ProgressMetrics) + Send + 'static,
) -> ReceiveOutcome
where
    F: FnOnce(Vec<String>, u64) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let bin = croc_binary_path();
    let mut cmd = Command::new(&bin);
    cmd.env("CROC_SECRET", code).env("NO_COLOR", "1");
    cmd.arg("--overwrite"); // deliberately NO --yes / --ignore-stdin
    apply_relay(&mut cmd, settings);
    cmd.arg("--out").arg(out_dir);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return ReceiveOutcome::Error,
    };
    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::new();
    let mut on_offer = Some(on_offer);

    loop {
        tokio::select! {
            n = stderr.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if b == b'\r' || b == b'\n' {
                                if let Some(m) = crate::croc::parse_progress_metrics(
                                    &String::from_utf8_lossy(&line),
                                ) {
                                    on_progress(m);
                                }
                                line.clear();
                            } else {
                                line.push(b);
                            }
                        }
                        // The accept prompt carries no newline, so test the partial line.
                        if on_offer.is_some() {
                            let partial = String::from_utf8_lossy(&line);
                            if let Some((names, total)) = parse_accept_prompt(&partial) {
                                let cb = on_offer.take().unwrap();
                                let accept = tokio::select! {
                                    a = cb(names, total) => a,
                                    _ = stop_notify.notified() => {
                                        let _ = child.start_kill();
                                        let _ = child.wait().await;
                                        return ReceiveOutcome::Stopped;
                                    }
                                };
                                let answer: &[u8] = if accept { b"y\n" } else { b"n\n" };
                                let _ = stdin.write_all(answer).await;
                                let _ = stdin.flush().await;
                                line.clear();
                            }
                        }
                    }
                }
            }
            _ = stop_notify.notified() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return ReceiveOutcome::Stopped;
            }
        }
    }

    if stopped.load(Ordering::SeqCst) {
        let _ = child.wait().await;
        return ReceiveOutcome::Stopped;
    }
    match child.wait().await {
        Ok(s) if s.success() => ReceiveOutcome::Received,
        _ => ReceiveOutcome::Error,
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        tokio::time::sleep(deadline - now).await;
    }
}

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
) -> Vec<String> {
    let folder_str = folder;
    let folder = Path::new(folder);
    let mut moved = Vec::new();
    for src in list_files_rec(staging) {
        let Ok(rel) = src.strip_prefix(staging) else {
            continue;
        };
        let mut dest = folder.join(rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        dest = unique_dest(dest);
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

fn clear_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
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

fn friend_download_dir(settings: &Settings) -> String {
    let d = settings.download_dir.trim();
    if !d.is_empty() {
        return d.to_string();
    }
    std::env::var("HOME")
        .map(|h| format!("{h}/Downloads"))
        .unwrap_or_else(|_| ".".to_string())
}

fn structural_sig(p: &Pair) -> String {
    format!(
        "{}|{:?}|{}|{}",
        p.folder,
        p.role,
        p.secret,
        p.two_way
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

    #[test]
    fn parses_single_file_accept_prompt() {
        // Exact format croc v10.4.4 emits (captured from a real transfer).
        let (names, total) = parse_accept_prompt("Accept 'dbtest-src.txt' (15 B)? (Y/n) ").unwrap();
        assert_eq!(names, vec!["dbtest-src.txt".to_string()]);
        assert_eq!(total, 15);
    }

    #[test]
    fn parses_multi_file_accept_prompt() {
        let (names, total) = parse_accept_prompt("Accept 3 files (10.5 MB)? (Y/n)").unwrap();
        assert_eq!(names, vec!["3 files".to_string()]);
        assert_eq!(total, 10_500_000);
    }

    #[test]
    fn ignores_non_prompt_lines() {
        assert!(parse_accept_prompt("securing channel...").is_none());
        assert!(parse_accept_prompt(" file 45% |##| (4/10 MB, 5 MB/s) [1s:2s]").is_none());
    }

    #[test]
    fn parses_prompt_with_spaces_in_name() {
        let (names, total) =
            parse_accept_prompt("Accept 'Q3 Report.pdf' (2.3 MB)? (Y/n)").unwrap();
        assert_eq!(names, vec!["Q3 Report.pdf".to_string()]);
        assert_eq!(total, 2_300_000);
    }
}
