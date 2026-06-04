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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Notify;

use crate::croc::croc_binary_path;
use crate::models::{DeleteMode, FolderState, FolderStatus, Pair, Settings};
use crate::{pairing, AppState};

const STAGING_DIR: &str = ".dropbeam-incoming";
const CONNECT_TIMEOUT_SECS: u64 = 75;
const MAX_BACKOFF_SECS: u64 = 30;

/// Manages all active Shared Drop Folders.
pub struct SyncManager {
    app: AppHandle,
    config_dir: PathBuf,
    handles: Mutex<HashMap<String, PairHandle>>,
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
    detail: Option<String>,
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        StatusSnapshot {
            state: FolderState::Idle,
            sending_file: None,
            percent: 0.0,
            detail: None,
        }
    }
}

impl SyncManager {
    pub fn new(app: AppHandle, config_dir: PathBuf) -> Arc<Self> {
        Arc::new(SyncManager {
            app,
            config_dir,
            handles: Mutex::new(HashMap::new()),
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
                    detail: s.detail,
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
                    move |pct| {
                        if let Ok(mut s) = status_cb.lock() {
                            s.percent = pct;
                            s.state = FolderState::Sending;
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
                    move |pct| {
                        if let Ok(mut s) = status_cb.lock() {
                            s.percent = pct;
                            s.state = FolderState::Receiving;
                            s.sending_file = None;
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
            detail: s.detail,
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

static RE_PCT: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(\d+)%\s+\|").unwrap());

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
    on_percent: impl Fn(f64) + Send + 'static,
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
                                if let Some(pct) = parse_pct(&line) {
                                    connected = true;
                                    on_percent(pct);
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
    on_percent: impl Fn(f64) + Send + 'static,
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
                                if let Some(pct) = parse_pct(&line) {
                                    on_percent(pct);
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

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parse_pct(raw: &[u8]) -> Option<f64> {
    let s = String::from_utf8_lossy(raw);
    let caps = RE_PCT.captures(s.trim())?;
    caps.get(1)?.as_str().parse::<f64>().ok()
}

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
