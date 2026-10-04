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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::models::{
    DeleteMode, FolderState, FolderStatus, Friend, Locality, Pair, Settings, VerifyResult,
};
use crate::{friends, pairing, AppState};

const MAX_BACKOFF_SECS: u64 = 30;

/// Manages all active Shared Drop Folders and friend inbox listeners.
pub struct SyncManager {
    app: AppHandle,
    viewer_warning_at: Mutex<HashMap<String, Instant>>,
    config_dir: PathBuf,
    handles: Mutex<HashMap<String, PairHandle>>,
    friend_handles: Mutex<HashMap<String, FriendHandle>>,
    /// Serializes `reconcile()`. It runs from the startup thread, beacon handlers
    /// and commands; two overlapping runs could both see "no handle" for a pair and
    /// both `start_pair` it — the second insert orphaned the first worker, which
    /// then ignored Pause/remove and kept syncing until restart.
    reconcile_lock: Mutex<()>,
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
    queue: Arc<Mutex<SendQueue>>,
    inbound: Arc<Mutex<HashSet<String>>>,
    status: Arc<Mutex<StatusSnapshot>>,
    /// Signatures the peer answered "kept" for (see `load_skipped`).
    skipped: Arc<Mutex<HashMap<String, String>>>,
    /// Files received but not yet announced (History row + notification) — one
    /// announcement per receive BURST instead of one per 32-file batch.
    recv_burst: Arc<Mutex<(Vec<String>, u64)>>,
    /// Loop-guard for files we just wrote/removed ourselves (shared with the
    /// listener + collector so an iroh receive lands without echoing back).
    self_deleted: Arc<Mutex<HashMap<String, Instant>>>,
    /// Mirror deletes queued for this link's peer; the control beacon flushes
    /// them. Stored here so a group delete can be FANNED to every other link.
    pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
    /// Wakes this link's control sender to flush a freshly-queued delete now.
    control_wake: Arc<Notify>,
    request_snapshot_reply: Arc<AtomicBool>,
    force_snapshot: Arc<AtomicBool>,
    /// Tombstones (rel → deleted-at ms): every deletion we've observed locally or
    /// adopted from the peer. Rides the reconcile beacon so a delete propagates
    /// even if the live event was missed, and prevents a deleted file from being
    /// resurrected by the peer's add-reconcile. Persisted across restarts.
    tombstones: Arc<Mutex<HashMap<String, u64>>>,
    /// Recent intra-folder renames/moves (`from_rel → (to_rel, size, mtime, ts)`)
    /// to send to this link's peer on the control beacon, so they relocate instead
    /// of re-downloading. Persisted; idempotent + content-verified on apply.
    moves: Arc<Mutex<HashMap<String, MoveRec>>>,
    /// inode → (rel, size) for files currently on disk — how the collector
    /// recognizes a moved file (same inode, new path) as the SAME bytes.
    ino_index: Arc<Mutex<HashMap<u64, (String, u64, u64)>>>,
    /// Sync paused for this folder (live flag the workers read). Mirrors the
    /// persisted `Pair.paused`; updated by the user toggle + the peer's beacon.
    paused: Arc<AtomicBool>,
    /// The most recent full snapshot the peer beaconed (its file set + tombstones)
    /// plus the local ms when it landed. The "Verify" button reads this to compare
    /// the two folders WITHOUT inventing a new sync path — it's exactly what the
    /// self-heal reconcile already runs on. `None` until the peer beacons once.
    last_peer_snapshot: Arc<Mutex<Option<(Reconcile, u64)>>>,
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
    /// The local folder itself is gone (renamed, trashed, or on a drive that isn't
    /// mounted) — the filesystem watch couldn't be registered because the path does
    /// not exist. Set at start_pair, cleared when the watch succeeds. Surfaced to
    /// the user through `detail` so the card says WHY nothing is syncing instead of
    /// the truth living only in a log line.
    folder_missing: bool,
    /// How many files the peer reported in its last reconcile snapshot — so the UI
    /// can show "both have N files, in sync" and the user can SEE the folders match.
    peer_files: u32,
    /// Aggregate progress for the CURRENT send burst (a folder drop = one burst):
    /// total files queued this burst, and how many are done. Lets the UI show ONE
    /// progress bar ("12 of 50 files") instead of a card flashing once per file.
    session_total_files: u32,
    session_done_files: u32,
    /// Something the user should know that isn't an error (e.g. files the peer
    /// can't hold under their names).
    warning: Option<String>,
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
            folder_missing: false,
            peer_files: 0,
            session_total_files: 0,
            session_done_files: 0,
            warning: None,
        }
    }
}

/// A removal to propagate to the mirror peer.
#[derive(Clone)]
struct DeleteEvent {
    rel: String,
    ts: u64,
    /// The version (size, mtime) we last had — the peer deletes only THAT version.
    known: Option<(u64, u64)>,
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
    /// The version (size, mtime) each tombstone deleted, from a peer that tracks
    /// them. `None` = a legacy peer (tombstones apply by timestamp).
    pub tomb_versions: Option<HashMap<String, (u64, u64)>>,
}

impl SyncManager {
    pub fn new(app: AppHandle, config_dir: PathBuf) -> Arc<Self> {
        let _ = sync_config_dir().set(config_dir.clone());
        Arc::new(SyncManager {
            app,
            viewer_warning_at: Mutex::new(HashMap::new()),
            config_dir,
            handles: Mutex::new(HashMap::new()),
            friend_handles: Mutex::new(HashMap::new()),
            reconcile_lock: Mutex::new(()),
        })
    }

    /// Bring running folders in line with what's persisted on disk.
    /// Synchronous filesystem work: async callers must use spawn_blocking.
    pub fn reconcile(self: &Arc<Self>) {
        let _serial = self.reconcile_lock.lock().unwrap_or_else(|e| e.into_inner());
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
            let (existing_sig, was_viewer) = self
                .handles
                .lock()
                .unwrap()
                .get(&pair.id)
                .map(|h| (Some(h.sig.clone()), h.config.lock().unwrap().i_am_viewer))
                .unwrap_or((None, false));
            // Promoted from viewer to editor: whatever this device changed WHILE it
            // was read-only must not be pushed to everyone just because the role
            // flipped. Baseline the current files as already-known; only edits made
            // from now on (a deliberate act) sync out.
            if was_viewer && !pair.i_am_viewer && pair.mirror {
                baseline_on_promotion(&self.config_dir, &pair);
            }
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

    /// Drive a REAL, trustworthy verification for ONE folder link and report whether
    /// the two folders are identical. Reuses the existing reconcile/manifest-exchange
    /// plumbing — it does NOT invent a new sync path:
    ///   1. Wake this link's control sender so it re-beacons our manifest now, and
    ///      ask the peer (via the same beacon) to send theirs back promptly.
    ///   2. Wait (bounded) for a FRESH peer snapshot to land on the control channel.
    ///   3. Compare our live manifest against that snapshot using the same per-file
    ///      signature rule the self-heal reconcile uses (`reconcile_plan` for the
    ///      delete/push decisions, plus the reverse direction for files the peer has
    ///      that we lack), and count matches + differences.
    /// The differences it reports are exactly what the background reconcile is
    /// already converging, so the UI can honestly say "syncing them now".
    pub async fn verify_folder(self: &Arc<Self>, pair_id: &str) -> VerifyResult {
        let unavailable = || VerifyResult {
            peer_online: false, compared: false, identical: false, matched: 0,
            differences: 0, missing_on_peer: 0, missing_locally: 0,
            pending_deletes: 0, local_files: 0, peer_files: 0,
        };
        // Grab the per-link state we need (or bail with "couldn't compare" if this
        // isn't a folder we're actively managing — e.g. a non-mirror pair).
        let (folder, control_wake, tombstones, last_snapshot, snapshot_before, status) = {
            let Ok(handles) = self.handles.lock() else { return unavailable(); };
            let Some(h) = handles.get(pair_id) else {
                return VerifyResult {
                    peer_online: false,
                    compared: false,
                    identical: false,
                    matched: 0,
                    differences: 0,
                    missing_on_peer: 0,
                    missing_locally: 0,
                    pending_deletes: 0,
                    local_files: 0,
                    peer_files: 0,
                };
            };
            h.request_snapshot_reply.store(true, Ordering::SeqCst);
            h.force_snapshot.store(true, Ordering::SeqCst);
            let folder = match h.config.lock() {
                Ok(config) => config.folder.clone(), Err(_) => return unavailable(),
            };
            let before = match h.last_peer_snapshot.lock() {
                Ok(snapshot) => snapshot.as_ref().map(|(_, ts)| *ts),
                Err(_) => return unavailable(),
            };
            (
                folder,
                h.control_wake.clone(),
                h.tombstones.clone(),
                h.last_peer_snapshot.clone(),
                before,
                h.status.clone(),
            )
        };

        // Kick BOTH sides to re-exchange manifests now (same mechanism as verify_now,
        // scoped to this link). The peer, on receiving our beacon, beacons back its
        // own snapshot, which lands in `last_peer_snapshot`.
        control_wake.notify_one();

        // Wait up to 40s (each control sender has a 15s debounce floor) for
        // a snapshot newer than the one we had before we
        // nudged — so we compare against a genuinely fresh exchange, not a stale one.
        // Polls cheaply; returns as soon as a fresh snapshot arrives.
        let deadline = Instant::now() + Duration::from_secs(40);
        let (peer_snapshot, got_fresh) = loop {
            let cur = match last_snapshot.lock() {
                Ok(snapshot) => snapshot.clone(), Err(_) => return unavailable(),
            };
            let is_fresh = match (&cur, snapshot_before) {
                (Some((_, ts)), Some(before)) => *ts > before,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if is_fresh {
                break (cur.map(|(rec, _)| rec), true);
            }
            if Instant::now() >= deadline {
                // No fresh exchange — fall back to whatever (possibly stale) snapshot
                // we have so we can still report a best-effort answer, but flag that
                // we couldn't confirm a live round-trip.
                break (cur.map(|(rec, _)| rec), false);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        };

        let Some(rec) = peer_snapshot else {
            // Never heard from the peer at all — be honest that we couldn't compare.
            let local = live_manifest_async(folder.clone()).await;
            return VerifyResult {
                peer_online: status.lock().map(|s| s.peer_online).unwrap_or(false),
                compared: false,
                identical: false,
                matched: 0,
                differences: 0,
                missing_on_peer: 0,
                missing_locally: 0,
                pending_deletes: 0,
                local_files: local.len() as u32,
                peer_files: 0,
            };
        };

        let mine = live_manifest_async(folder.clone()).await;
        let my_tomb = match tombstones.lock() {
            Ok(tombstones) => tombstones.clone(), Err(_) => return unavailable(),
        };
        let mut result = compute_verify(&mine, &rec, &my_tomb, now_ms());
        result.compared = got_fresh;
        result.peer_online = status.lock().map(|s| s.peer_online).unwrap_or(false);
        log::info!("verify folder {pair_id}: online={}, fresh={}, differences={}",
            result.peer_online, got_fresh, result.differences);
        result
    }

    /// Reply through this folder's existing control sender, never a separate probe.
    /// The reply does not request another reply, so idle peers cannot ping-pong.
    pub fn request_folder_snapshot(&self, pair_id: &str, sender_eid: &str) {
        let Ok(handles) = self.handles.lock() else { return; };
        if let Some(h) = handles.get(pair_id) {
            if h.config.lock().ok().is_some_and(|c| c.endpoint_id.as_deref() == Some(sender_eid)) {
                h.force_snapshot.store(true, Ordering::SeqCst);
                h.control_wake.notify_one();
            }
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
                    session_total_files: s.session_total_files,
                    session_done_files: s.session_done_files,
                    paused: h.paused.load(Ordering::Relaxed),
                    conn_detail: None,
                    folder_missing: s.folder_missing,
                    warning: s.warning.clone(),
                }
            })
            .map(apply_missing_folder)
            .collect()
    }

    fn start_pair(self: &Arc<Self>, pair: Pair) {
        let _walk = crate::fs_walk::Watch::new("sync startup", Path::new(&pair.folder));
        // One-time cleanup of the v0.43.x history-leak: a VISIBLE `dropbeam-history`
        // (no leading dot) inside a shared folder is always an artifact of the old
        // receive-side de-dot bug — the real archive is the hidden `.dropbeam-history`.
        // Remove it so it stops polluting the folder + syncing. Never touches the
        // hidden real one.
        {
            let leaked = Path::new(&pair.folder).join("dropbeam-history");
            if leaked.is_dir() {
                match std::fs::remove_dir_all(&leaked) {
                    Ok(()) => log::warn!(
                        "swept leaked visible dropbeam-history from {}",
                        pair.folder
                    ),
                    Err(e) => log::warn!("could not sweep leaked dropbeam-history: {e}"),
                }
            }
        }
        // Sweep crash-orphaned "<name>.dropbeam-incoming" placeholders for EVERY
        // pair, not just sender-role ones — seed_existing's sweep only runs in
        // the sender worker, so a receive-only pair kept its litter forever. Only
        // stale ones go (>10 min): a live receive may legitimately own a fresh
        // placeholder right now.
        {
            for f in list_files_rec(Path::new(&pair.folder)) {
                if f.to_string_lossy().ends_with(".dropbeam-incoming") {
                    sweep_stale_placeholder(&f);
                }
            }
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(Notify::new());
        let wake = Arc::new(Notify::new());
        let queue = Arc::new(Mutex::new(SendQueue::default()));
        let inbound = Arc::new(Mutex::new(load_manifest(&self.config_dir, &pair.id)));
        let skipped = Arc::new(Mutex::new(load_skipped(&self.config_dir, &pair.id)));
        let status = Arc::new(Mutex::new(StatusSnapshot::default()));
        if let Ok(mut s) = status.lock() {
            s.warning = skipped_warning(&skipped.lock().unwrap(), &peer_label(&pair));
        }
        let config = Arc::new(Mutex::new(pair.clone()));
        let sig = structural_sig(&pair);

        let mut watcher = None;

        // Total-sync (mirror) state: delete events queued for the peer, a loop
        // guard for files WE removed (so applying a remote delete or an overwrite
        // never echoes back), and a wake to flush deletes to the peer promptly.
        let pending_deletes: Arc<Mutex<Vec<DeleteEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let self_deleted: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let control_wake = Arc::new(Notify::new());
        let request_snapshot_reply = Arc::new(AtomicBool::new(false));
        let force_snapshot = Arc::new(AtomicBool::new(false));
        let tombstones: Arc<Mutex<HashMap<String, u64>>> =
            Arc::new(Mutex::new(load_tombstones(&self.config_dir, &pair.id)));
        // Move/rename detection state. The inode index seeds from what's on disk so
        // a file moved right after launch is still recognized; it's refreshed each
        // control round. Recorded moves ride the control beacon as a pure optimization
        // (the file's own deletion is still propagated as the reliable backstop).
        let moves: Arc<Mutex<HashMap<String, MoveRec>>> =
            Arc::new(Mutex::new(load_moves(&self.config_dir, &pair.id)));
        let ino_index: Arc<Mutex<HashMap<u64, (String, u64, u64)>>> = Arc::new(Mutex::new(
            live_manifest(&pair.folder)
                .into_iter()
                .filter(|(_, e)| e.inode != 0)
                .map(|(rel, e)| (e.inode, (rel, e.size, e.mtime)))
                .collect(),
        ));
        let paused = Arc::new(AtomicBool::new(pair.paused));
        let skip_current = Arc::new(AtomicBool::new(false));

        if pairing::runs_sender(&pair) || pair.i_am_viewer {
            // Filesystem watcher → candidate channel. We deliberately do NOT trust
            // the event *kind* to tell adds from deletes: macOS reports a
            // "Move to Trash" as a rename, not a Remove, so kind-based routing
            // silently dropped trashed-file deletes. Instead we forward every
            // touched path and let the collector classify by whether the file
            // still exists after a short settle.
            let (evt_tx, evt_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
            let folder = pair.folder.clone();
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(event) => {
                        use notify::EventKind;
                        // The OS dropped events (FSEvents MustScanSubDirs / inotify
                        // queue overflow): individual deletes may be lost. An EMPTY
                        // path tells the collector to re-diff the folder (D5).
                        if event.need_rescan() {
                            let _ = evt_tx.send(PathBuf::new());
                        }
                        if matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        ) {
                            for path in event.paths {
                                let _ = evt_tx.send(path);
                            }
                        }
                    }
                    Err(_) => {
                        let _ = evt_tx.send(PathBuf::new());
                    }
                }
            }) {
                Ok(mut w) => {
                    use notify::Watcher;
                    match w.watch(Path::new(&folder), notify::RecursiveMode::Recursive) {
                        Ok(()) => {
                            // Watching again — forget the failure so a folder that
                            // disappears a SECOND time is reported afresh.
                            clear_watch_failure(&folder);
                            watcher = Some(w);
                        }
                        Err(e) => {
                            // Log once per folder per process: start_pair re-runs on
                            // every structural change, and a folder that's simply gone
                            // fails identically every time (74 copies of one line in
                            // the diagnostics digest). The condition itself is not
                            // swallowed — it goes into the pair's status below.
                            if note_watch_failure(&folder) {
                                log::warn!("watch failed for {folder}: {e}");
                            }
                            if !Path::new(&folder).exists() {
                                if let Ok(mut s) = status.lock() {
                                    s.folder_missing = true;
                                }
                            }
                        }
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
                moves.clone(),
                ino_index.clone(),
            );

            if pairing::runs_sender(&pair) {
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
                    paused.clone(),
                    skipped.clone(),
                );

                // Seed the queue with any files already sitting in the folder — off
                // this thread: the scan includes a write-completion check (D2).
                let (f, i, q, w) = (pair.folder.clone(), inbound.clone(), queue.clone(), wake.clone());
                tauri::async_runtime::spawn_blocking(move || {
                    seed_existing(&f, &i, &q, &w);
                });
            }
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
            request_snapshot_reply.clone(),
            force_snapshot.clone(),
            pending_deletes.clone(),
            control_wake.clone(),
            tombstones.clone(),
            moves.clone(),
            ino_index.clone(),
            paused.clone(),
            inbound.clone(),
            queue.clone(),
            skipped.clone(),
            self_deleted.clone(),
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
            skipped,
            recv_burst: Arc::new(Mutex::new((Vec::new(), 0))),
            self_deleted,
            pending_deletes,
            control_wake,
            request_snapshot_reply,
            force_snapshot,
            tombstones,
            moves,
            ino_index,
            paused,
            last_peer_snapshot: Arc::new(Mutex::new(None)),
            skip_current,
            _watcher: watcher,
        };
        let replaced = self.handles.lock().unwrap().insert(pair.id.clone(), handle);
        if let Some(old) = replaced {
            // Never leave a superseded worker running unowned (see reconcile_lock).
            old.stopped.store(true, Ordering::SeqCst);
            old.stop_notify.notify_waiters();
            old.wake.notify_waiters();
        }
        self.emit_status(&pair.id);
    }

    /// One warning per physical folder per minute, including groups with many links.
    fn warn_viewer_change(&self, folder: &str) {
        let Ok(mut warnings) = self.viewer_warning_at.lock() else { return; };
        if viewer_warning_due(warnings.get(folder).copied(), Instant::now()) {
            warnings.insert(folder.to_string(), Instant::now());
            drop(warnings);
            let _ = self.app.emit("folder://viewer-change", folder);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_collector(
        self: Arc<Self>,
        mut evt_rx: tokio::sync::mpsc::UnboundedReceiver<PathBuf>,
        config: Arc<Mutex<Pair>>,
        stopped: Arc<AtomicBool>,
        wake: Arc<Notify>,
        queue: Arc<Mutex<SendQueue>>,
        inbound: Arc<Mutex<HashSet<String>>>,
        pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
        self_deleted: Arc<Mutex<HashMap<String, Instant>>>,
        control_wake: Arc<Notify>,
        tombstones: Arc<Mutex<HashMap<String, u64>>>,
        moves: Arc<Mutex<HashMap<String, MoveRec>>>,
        ino_index: Arc<Mutex<HashMap<u64, (String, u64, u64)>>>,
    ) {
        let manager = self.clone();
        let config_dir = self.config_dir.clone();
        let pair_id_c = config.lock().unwrap().id.clone();
        let debounce: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        // At most one overflow re-diff in flight per folder.
        let rescan_running = Arc::new(AtomicBool::new(false));
        tauri::async_runtime::spawn(async move {
            while let Some(path) = evt_rx.recv().await {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let (folder, mirror, viewer) = {
                    let c = config.lock().unwrap();
                    (c.folder.clone(), c.mirror, c.i_am_viewer)
                };
                // Watcher overflow: deletes may have been dropped — re-diff the
                // folder against what we know (D5). Mirror editors only.
                if path.as_os_str().is_empty() {
                    if mirror && !viewer && !rescan_running.swap(true, Ordering::SeqCst) {
                        let (m, pid, flag, st) =
                            (manager.clone(), pair_id_c.clone(), rescan_running.clone(), stopped.clone());
                        tauri::async_runtime::spawn(async move {
                            m.propagate_missed_deletes(&pid, now_ms(), &st).await;
                            flag.store(false, Ordering::SeqCst);
                        });
                    }
                    continue;
                }
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
                let moves2 = moves.clone();
                let ino2 = ino_index.clone();
                let manager2 = manager.clone();
                tauri::async_runtime::spawn(async move {
                    let body = async {
                    // Classify by EXISTENCE, not by the OS event kind. A file that
                    // is present is an add/change; one that's gone is a delete —
                    // works no matter how macOS labels a trash/move/rename.
                    if Path::new(&p).is_file() {
                        // ── ADD / CHANGE ──────────────────────────────────────
                        if !is_sendable_candidate(&p, &folder2, &inbound2) {
                            return;
                        }
                        // Note first sight (platforms without ctime, see D15) BEFORE
                        // the settle wait, so a moved-in old file reads as new.
                        note_appeared_now(Path::new(&p));
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
                        if viewer {
                            manager2.warn_viewer_change(&folder2);
                            return;
                        }
                        // ── MOVE / RENAME detection (mirror folders) ─────────────
                        // If this file's inode previously lived at a DIFFERENT path
                        // that's now gone, the user moved/renamed it inside the
                        // folder. Record a move op (rides the control beacon, applied
                        // before the delete) so the peer relocates its copy instead of
                        // re-uploading the bytes. We DON'T suppress the old path's
                        // deletion — it's the reliable backstop if the move is lost or
                        // the peer is older. Inode+size match = same file (no false
                        // positives for a same-volume move, the case the user hits).
                        if mirror
                            && handle_move_candidate(&p, &folder2, &cfgdir2, &pidc2, &moves2, &ino2, &cw)
                        {
                            return; // recorded as a move — don't enqueue a byte send
                        }
                        queue2.lock().unwrap().push_back(p.clone());
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
                        // sender re-checks each file's size+mtime around the send (D2),
                        // so a file still growing is re-sent, never marked done.
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        if stopped2.load(Ordering::SeqCst) {
                            return;
                        }
                        let scan_dir = PathBuf::from(&p);
                        let files = tokio::task::spawn_blocking(move || list_files_rec(&scan_dir))
                            .await.expect("directory scan panicked");
                        if viewer {
                            if files.iter().any(|f| is_sendable_candidate(&f.to_string_lossy(), &folder2, &inbound2)) {
                                manager2.warn_viewer_change(&folder2);
                            }
                            return;
                        }
                        let had_files = !files.is_empty();
                        // Candidate checks stat every file — do them before taking
                        // the queue lock, then one O(1)-per-item push.
                        let cands: Vec<String> = files
                            .iter()
                            .map(|f| f.to_string_lossy().to_string())
                            .filter(|fp| is_sendable_candidate(fp, &folder2, &inbound2))
                            .collect();
                        for c in &cands {
                            note_appeared_now(Path::new(c));
                        }
                        let mut any = false;
                        {
                            let mut q = queue2.lock().unwrap();
                            for fp in cands {
                                any |= q.push_back(fp);
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
                    } else if mirror || viewer {
                        // ── DELETE (total-sync only; path is truly GONE) ──────
                        let Some(rel) = rel_path_of(&p, &folder2) else {
                            return;
                        };
                        if rel.is_empty()
                            || rel.ends_with(".dropbeam-incoming")
                            || rel.split('/').any(|c| c.starts_with('.'))
                            || crate::iroh_net::is_control_rel(&rel)
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
                        // An editor's atomic save is unlink+rename, and a freshly
                        // RECEIVED file is momentarily gone while the receive side
                        // archives-then-rewrites it (incoming-wins replace). Re-verify
                        // over several seconds and ABORT the instant the path returns
                        // as anything — a file (a save / re-receive) or a directory
                        // (recreated/renamed). This is what stops a rapid SECOND drop
                        // into a subfolder from being misread as a delete and
                        // tombstoned (then wrongly applied once the freshness window
                        // lapses). Deletes aren't latency-critical, so a few extra
                        // seconds of settle is a cheap price for not wiping live data.
                        let mut reappeared = false;
                        for _ in 0..5 {
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                            if stopped2.load(Ordering::SeqCst) {
                                return;
                            }
                            if Path::new(&p).exists() {
                                reappeared = true;
                                break;
                            }
                        }
                        if reappeared {
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
                        if viewer {
                            manager2.warn_viewer_change(&folder2);
                            return;
                        }
                        // ROOT GUARD (D5/D6): an unplugged drive / unmounted share
                        // makes EVERY file "disappear". That is never a delete — with
                        // the folder itself gone, propagate nothing.
                        if !folder_available(&folder2) {
                            log::warn!("folder {folder2} is not available — not treating {rel:?} as deleted");
                            return;
                        }
                        // Expand a DIRECTORY deletion into its children. macOS fires
                        // one Remove event for the folder, not one per file inside —
                        // so propagating only the folder rel left every file behind
                        // on the peer. We recover the children from the manifest
                        // (the files we know lived under `rel/`), tombstone + queue a
                        // delete for each, AND for the folder rel itself (a real file
                        // delete has no children, so it just tombstones itself). Each
                        // carries the exact version we had, so the peer deletes only
                        // that version — never a newer edit, never a file we never saw.
                        let (mut targets, known_sigs): (Vec<(String, Option<(u64, u64)>)>, Vec<String>) = {
                            let inb = inbound2.lock().unwrap();
                            let prefix = format!("{rel}/");
                            let mut latest: HashMap<String, (u64, u64)> = HashMap::new();
                            let mut sigs = Vec::new();
                            for sig in inb.iter() {
                                let Some(r) = sig_rel(sig) else { continue };
                                if r != rel && !r.starts_with(&prefix) {
                                    continue;
                                }
                                sigs.push(sig.clone());
                                let mut it = sig.rsplitn(3, '|');
                                let mt = it.next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
                                let sz = it.next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
                                let e = latest.entry(r).or_insert((sz, mt));
                                if mt >= e.1 {
                                    *e = (sz, mt);
                                }
                            }
                            (latest.into_iter().map(|(r, v)| (r, Some(v))).collect(), sigs)
                        };
                        // DATA-LOSS GUARD: a directory delete that would wipe MANY
                        // files is the dangerous case. A folder REPLACE (drop a new
                        // version over an existing one) shows up as remove-then-copy,
                        // and a multi-GB copy can outlast the 1200ms settle above — so
                        // re-verify over a longer window. If the directory (or any of
                        // its files) re-materializes, it's a replace/re-drop, NOT a
                        // delete: bail and let the live ADD path + reconcile sync it.
                        if targets.len() > 1 {
                            let prefix = format!("{rel}/");
                            for _ in 0..12 {
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                                if stopped2.load(Ordering::SeqCst) {
                                    return;
                                }
                                let reappeared = Path::new(&p).exists()
                                    || targets.iter().any(|(t, _)| {
                                        (t == &rel || t.starts_with(&prefix))
                                            && Path::new(&folder2).join(t).exists()
                                    });
                                if reappeared {
                                    log::warn!(
                                        "folder delete of {rel:?} ({} files) ABORTED — path \
                                         reappeared (folder replace/re-drop, not a delete)",
                                        targets.len()
                                    );
                                    return;
                                }
                            }
                            if !folder_available(&folder2) {
                                return; // the whole folder went away meanwhile
                            }
                        }
                        // Children first, the directory rel LAST: the peer applies them
                        // in order and only removes the directory once it's empty. A
                        // name we never had a synced version of travels WITHOUT one,
                        // which (new peers) can only ever remove an EMPTY directory —
                        // never a file of theirs we never saw.
                        targets.sort_by(|a, b| a.0.cmp(&b.0));
                        if !targets.iter().any(|(r, _)| r == &rel) {
                            targets.push((rel.clone(), None));
                        } else if let Some(pos) = targets.iter().position(|(r, _)| r == &rel) {
                            let own = targets.remove(pos);
                            targets.push(own);
                        }
                        let ts = now_ms();
                        {
                            let mut tomb_changed = false;
                            let mut pend = pd.lock().unwrap();
                            for (t, known) in &targets {
                                tomb_changed |= note_tombstone(&tomb2, t, ts);
                                if let Some(v) = known {
                                    note_tomb_version(&pidc2, t, *v, ts);
                                }
                                if !pend.iter().any(|d| &d.rel == t) {
                                    pend.push(DeleteEvent { rel: t.clone(), ts, known: *known });
                                }
                            }
                            drop(pend);
                            // ONE write for the whole delete batch (a big subfolder
                            // used to rewrite the entire tombstone file per target).
                            if tomb_changed {
                                persist_tombstones(&cfgdir2, &pidc2, &tomb2);
                                persist_tomb_versions(&pidc2);
                            }
                        }
                        record_deleted_versions(&cfgdir2, &folder2, &known_sigs, ts);
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
                    };
                    body.await;
                    // Debounce-map cleanup: drop this path's entry unless a newer
                    // event for it is in flight (the map used to grow forever).
                    let mut d = debounce2.lock().unwrap();
                    if d.get(&p).copied() == Some(gen) {
                        d.remove(&p);
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
        queue: Arc<Mutex<SendQueue>>,
        inbound: Arc<Mutex<HashSet<String>>>,
        status: Arc<Mutex<StatusSnapshot>>,
        skip_current: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        skipped: Arc<Mutex<HashMap<String, String>>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        tauri::async_runtime::spawn(async move {
            let mut current = String::new();
            let mut offline_attempts: u32 = 0;
            // Files delivered in the CURRENT burst — reset whenever the queue
            // drains, so "12 of 50" tracks one folder drop, not all time.
            let mut session_done: u32 = 0;
            // Bytes delivered + when the burst started, for the end-of-drop summary
            // (total size, duration, average speed) — same as the Send/Receive tab.
            let mut session_bytes: u64 = 0;
            let mut session_start: Option<Instant> = None;
            // Consecutive idle rescans that queued nothing — drives the
            // seed_existing backoff below (fast after activity, slow when quiet).
            let mut idle_scans: u32 = 0;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                // PAUSE GATE: while this folder is paused, park the sender entirely —
                // no uploads. Local edits keep queuing; Resume (notifies `wake`) flushes
                // them and the normal reconcile merges both sides.
                if paused.load(Ordering::Relaxed) {
                    set_status(&status, FolderState::Idle, None, 0.0, None);
                    manager.emit_status(&pair_id);
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    }
                    continue;
                }
                // MISSING-FOLDER GATE (D6): the drive is unplugged / the folder was
                // moved. Nothing can be read or sent; don't churn the queue (every
                // queued file would read as "deleted" and be dropped).
                let folder_now = config.lock().unwrap().folder.clone();
                if !folder_available(&folder_now) {
                    if let Ok(mut s) = status.lock() {
                        s.folder_missing = true;
                    }
                    manager.emit_status(&pair_id);
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    }
                    continue;
                }
                let next = queue.lock().unwrap().front().cloned();
                let Some(file) = next else {
                    session_done = 0;
                    session_bytes = 0;
                    session_start = None;
                    if let Ok(mut s) = status.lock() {
                        s.session_total_files = 0;
                        s.session_done_files = 0;
                    }
                    set_status(&status, FolderState::Idle, None, 0.0, None);
                    manager.emit_status(&pair_id);
                    // While idle, periodically re-scan for any local file that never
                    // reached the peer — an initial send that got interrupted, or
                    // files that already existed when the folder was paired. Re-queuing
                    // them is how a total-sync folder reliably converges to the SAME
                    // set of files on both computers (seed_existing skips anything
                    // already delivered or queued, so this is cheap + idempotent).
                    // BACKOFF: each rescan that finds nothing doubles the wait
                    // (45s → 90s → 180s cap) so a quiet thousands-file folder isn't
                    // re-walked every 45s forever; any real work snaps it back to
                    // 45s. The cap MUST stay under the control beacon's ~300s
                    // wake_sender kick — that wake restarts this sleep, so a cap
                    // ≥300s would starve the rescan entirely while the peer is up.
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(45u64 << idle_scans.min(2))) => {
                            let folder = config.lock().unwrap().folder.clone();
                            let scan_inbound = inbound.clone();
                            let scan_queue = queue.clone();
                            let scan_wake = wake.clone();
                            if tokio::task::spawn_blocking(move || {
                                seed_existing(&folder, &scan_inbound, &scan_queue, &scan_wake)
                            }).await.expect("seed scan panicked") {
                                idle_scans = 0;
                            } else {
                                idle_scans = idle_scans.saturating_add(1);
                            }
                        }
                    }
                    continue;
                };

                // A file removed from disk (e.g. user deleted it) is the ONLY reason
                // we ever drop a queued item without delivering.
                if !Path::new(&file).exists() {
                    queue.lock().unwrap().remove(&file);
                    continue;
                }
                // Real work in the queue → snap the idle rescan back to 45s.
                idle_scans = 0;
                if file != current {
                    current = file.clone();
                    offline_attempts = 0;
                }

                // PRESENCE GATE: don't dial-spam a peer we believe is offline. The
                // control beacon already probes presence on its own backoff and sets
                // `peer_online`; until that flips true we show a STABLE "waiting"
                // status instead of flapping Sending↔Waiting every few seconds and
                // burning CPU on doomed 12-second dials (the user's "it loops sending
                // over and over while my friend's computer is off"). We re-check the
                // cheap presence flag every 5s and send the instant the peer is back.
                if !status.lock().map(|s| s.peer_online).unwrap_or(false) {
                    let already_waiting = status
                        .lock()
                        .map(|s| matches!(s.state, FolderState::Waiting))
                        .unwrap_or(false);
                    if !already_waiting {
                        let label = {
                            let c = config.lock().unwrap();
                            peer_label(&c)
                        };
                        set_status(
                            &status,
                            FolderState::Waiting,
                            None,
                            0.0,
                            Some(format!("Waiting for {label} to come online")),
                        );
                        manager.emit_status(&pair_id);
                        // A fresh drop just parked behind a possibly-STALE offline
                        // flag. Kick the control beacon ONCE so it probes now instead
                        // of waiting out the remainder of its 60/120/300s offline
                        // backoff — if the peer is actually reachable, the beacon
                        // lands, flips peer_online, and wake_sender unparks us within
                        // seconds. The beacon stays the SOLE prober (we never dial
                        // from here), and this fires only on the Idle→Waiting
                        // transition of a new drop — not per 5s poll — so a truly
                        // offline peer costs one extra dial, not a flap loop.
                        manager.nudge_reconcile(&pair_id);
                    }
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = stop_notify.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                    continue;
                }

                // BATCH small files into one push. Every send pays a fresh dial +
                // direct-path wait + header/ack round-trips; one-at-a-time, a
                // 300-photo folder spends 10+ minutes on pure per-file overhead
                // over a WAN. The wire protocol has always supported multi-item
                // bodies (the header carries an `items` array and the receiver
                // loops it), so consecutive small files ride together. Big files
                // stay SOLO — that keeps their parallel-streams + resume path.
                const BATCH_MAX_FILES: usize = 32;
                const BATCH_FILE_CAP: u64 = 4 * 1024 * 1024; // only files ≤4 MiB batch
                const BATCH_BYTES_CAP: u64 = 64 * 1024 * 1024;
                let head_size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(u64::MAX);
                let mut batch: Vec<String> = vec![file.clone()];
                if head_size <= BATCH_FILE_CAP {
                    let mut bytes = head_size;
                    let q = queue.lock().unwrap();
                    for cand in q.iter().skip(1) {
                        if batch.len() >= BATCH_MAX_FILES || bytes >= BATCH_BYTES_CAP {
                            break;
                        }
                        let Ok(m) = std::fs::metadata(cand) else { break };
                        if m.len() > BATCH_FILE_CAP || bytes + m.len() > BATCH_BYTES_CAP {
                            break;
                        }
                        bytes += m.len();
                        batch.push(cand.clone());
                    }
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
                // Drop any batch entry we can't root under the folder. For a normally
                // queued path this never fires (its string is under the folder), but
                // if it ever does, NOT sending the file must also mean NOT popping /
                // auto-deleting it as if delivered — otherwise auto-delete could wipe a
                // file the peer never received. Remove it from the queue so it doesn't
                // spin; the local copy stays put.
                batch.retain(|f| {
                    if crate::iroh_net::folder_rel(Path::new(f), &pair.folder).is_some() {
                        return true;
                    }
                    log::warn!("folder send: dropping un-rootable queued path {f}");
                    queue.lock().unwrap().remove(f);
                    false
                });
                if batch.is_empty() {
                    continue;
                }
                let file = batch[0].clone();
                // D2: the exact version (size, mtime) of each file as we START the
                // send. If a file is still being written and changes under us, the
                // version that arrived isn't the one on disk: it must not be recorded
                // as delivered, and must NEVER trip auto-delete.
                let pre: HashMap<String, (u64, u64)> = batch
                    .iter()
                    .filter_map(|f| std::fs::metadata(f).ok().map(|m| (f.clone(), (m.len(), meta_mtime(&m)))))
                    .collect();
                // Size of this batch (for the burst's running byte total) + start the
                // burst clock on the first file so the end-of-drop summary can report
                // total bytes, elapsed time, and average speed.
                let batch_bytes: u64 = pre.values().map(|(sz, _)| *sz).sum();
                if session_start.is_none() {
                    session_start = Some(Instant::now());
                }
                let name = if batch.len() > 1 {
                    format!("{} (+{} more)", file_name_of(&file), batch.len() - 1)
                } else {
                    file_name_of(&file)
                };

                set_status(&status, FolderState::Sending, Some(name.clone()), 0.0, None);
                // Aggregate burst progress: total = already done + everything still
                // queued (incl. this batch). Drives the single "12 of 50 files" bar.
                {
                    let remaining = queue.lock().unwrap().len() as u32;
                    if let Ok(mut s) = status.lock() {
                        s.session_done_files = session_done;
                        s.session_total_files = session_done + remaining;
                    }
                }
                manager.emit_status(&pair_id);

                // Always ATTEMPT the direct push (try_iroh_folder_send has its own
                // 12s dial timeout and returns cheaply if the peer is unreachable or
                // we don't know their key yet). Driving the result off the actual
                // dial — not the cached presence flag — means a queued file lands
                // the instant the peer is reachable.
                let outcome = manager
                    .try_iroh_folder_send(&pair, &settings, &batch, &status, &stopped, &skip_current)
                    .await;

                // The user hit "Stop" on this transfer: it was aborted above. Move
                // it to the back of the queue (never dropped — reconcile + the queue
                // will bring it back) so the rest of the folder keeps flowing NOW.
                if skip_current.swap(false, Ordering::SeqCst) {
                    let mut q = queue.lock().unwrap();
                    for f in &batch {
                        q.rotate_to_back(f);
                    }
                    drop(q);
                    current = String::new();
                    offline_attempts = 0;
                    set_status(&status, FolderState::Idle, None, 0.0, None);
                    manager.emit_status(&pair_id);
                    continue;
                }

                let report = match outcome {
                    FolderSendOutcome::Report(r) => r,
                    FolderSendOutcome::Refused(reason) => {
                        // The peer turned the push away before reading it. Nothing was
                        // delivered; keep everything queued and wait — honestly.
                        offline_attempts = offline_attempts.saturating_add(1);
                        let peer = peer_label(&pair);
                        let detail = match reason.as_str() {
                            "paused" => format!("{peer} has paused this folder"),
                            "folder-missing" => format!("{peer}'s copy of this folder isn't available (drive disconnected?)"),
                            "viewer" => format!("{peer} has this folder as view-only for you"),
                            "unknown-pair" | "not-member" => format!("{peer} isn't sharing this folder with you right now"),
                            _ => format!("Couldn't reach {peer} just now — retrying"),
                        };
                        set_status(&status, FolderState::Waiting, Some(name.clone()), 0.0, Some(detail));
                        manager.emit_status(&pair_id);
                        // Long, steady back-off: these states change on a human
                        // timescale; the peer's beacon wakes us sooner when it does.
                        tokio::select! {
                            _ = wake.notified() => {}
                            _ = stop_notify.notified() => break,
                            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                        }
                        continue;
                    }
                    FolderSendOutcome::Failed => {
                        offline_attempts = offline_attempts.saturating_add(1);
                        // We reached this arm by PASSING the presence gate (peer_online
                        // was true), then the actual push failed — the dial timed out,
                        // the stall watchdog abandoned a frozen stream, the handshake
                        // raced, or the receiver never confirmed what it kept. That is
                        // NOT the same as "peer is off": surface an honest, distinct
                        // detail so the user can tell the two apart.
                        set_status(
                            &status,
                            FolderState::Waiting,
                            Some(name.clone()),
                            0.0,
                            Some(format!("Couldn't reach {} just now — retrying", peer_label(&pair))),
                        );
                        manager.emit_status(&pair_id);
                        // Don't let ONE file that keeps failing block every other file
                        // forever: after a few tries, rotate it to the BACK so the
                        // rest of the queue gets a turn. Never dropped.
                        if offline_attempts >= 3 {
                            let mut q = queue.lock().unwrap();
                            if q.len() > batch.len() {
                                for f in &batch {
                                    q.rotate_to_back(f);
                                }
                                drop(q);
                                current = String::new();
                                offline_attempts = 0;
                                continue;
                            }
                        }
                        if wait_backoff(&stop_notify, &stopped, offline_attempts).await {
                            break;
                        }
                        continue;
                    }
                };
                set_peer_online(&status, true);

                // Per-item outcomes (D1). Only `Landed` items whose bytes on disk are
                // still the version we sent (D2) count as delivered.
                let settled = settle_batch(&batch, &pre, &report);
                let delivered = settled.delivered.clone();
                let retry = settled.retry.clone();
                let kept_any = !settled.kept.is_empty();
                {
                    let mut q = queue.lock().unwrap();
                    let mut inb = inbound.lock().unwrap();
                    let mut sk = skipped.lock().unwrap();
                    for f in &settled.delivered {
                        q.remove(f);
                        q.note_ok(f);
                        if let Some(sig) = file_sig(f, &pair.folder) {
                            inb.insert(sig.clone());
                            sk.remove(&sig);
                        }
                    }
                    for (f, why) in &settled.kept {
                        // Deliberately not landed (peer keeps a newer copy, the name
                        // can't exist on their disk, …): don't resend THIS version
                        // and NEVER auto-delete it.
                        q.remove(f);
                        if let Some(sig) = file_sig(f, &pair.folder) {
                            inb.insert(sig.clone());
                            sk.insert(sig, why.clone());
                        }
                        log::info!("folder send: peer kept its own copy of {} ({why})", file_name_of(f));
                    }
                    for f in &settled.gone {
                        q.remove(f);
                    }
                    for f in &settled.retry {
                        if !q.note_failed(f) {
                            q.rotate_to_back(f);
                        }
                    }
                }
                {
                    let snapshot = inbound.lock().unwrap().clone();
                    manager.persist_manifest(&pair_id, &snapshot);
                }
                if kept_any || !delivered.is_empty() {
                    let snap = skipped.lock().unwrap().clone();
                    save_skipped(&manager.config_dir, &pair_id, &snap);
                    if let Ok(mut s) = status.lock() {
                        s.warning = skipped_warning(&snap, &peer_label(&pair));
                    }
                }
                if delivered.is_empty() {
                    if !retry.is_empty() {
                        offline_attempts = offline_attempts.saturating_add(1);
                        set_status(
                            &status,
                            FolderState::Waiting,
                            Some(name.clone()),
                            0.0,
                            Some(format!("{} couldn't save some files yet — retrying", peer_label(&pair))),
                        );
                        manager.emit_status(&pair_id);
                        if wait_backoff(&stop_notify, &stopped, offline_attempts).await {
                            break;
                        }
                    }
                    continue;
                }
                offline_attempts = 0;
                // Auto-delete ONLY files the peer confirmed landed, whose bytes didn't
                // change mid-send, and only with a receiver that reports per item (a
                // legacy bare "ok" can't vouch for each file).
                if pair.auto_delete && report.legacy {
                    log::warn!(
                        "folder send: peer is an older DropBeam (no per-file receipt) — \
                         keeping {} local file(s) instead of deleting after delivery",
                        delivered.len()
                    );
                }
                for f in auto_delete_targets(&settled, report.legacy, pair.auto_delete) {
                    delete_local(&f, pair.delete_mode);
                }
                let delivered_bytes: u64 = delivered.iter().filter_map(|f| pre.get(f)).map(|(sz, _)| *sz).sum();
                let _ = batch_bytes;
                session_done += delivered.len() as u32;
                let remaining = queue.lock().unwrap().len();
                set_status(&status, FolderState::Idle, None, 0.0, None);
                // Hold the burst counters across the gap before the next file
                // (set_status leaves these new fields alone) so the bar shows
                // a steady "N of M", not a reset between files.
                if let Ok(mut s) = status.lock() {
                    s.session_done_files = session_done;
                    s.session_total_files = session_done + remaining as u32;
                }
                manager.emit_status(&pair_id);
                session_bytes += delivered_bytes;
                // Cue the UI to optionally play a sound + flash the HUD.
                // `remaining` lets the UI ding ONCE when the whole drop is
                // done, not once per file.
                // Relative names of the files this batch moved, for the
                // chat timeline (GitHub #23). Additive — older UIs ignore it.
                let synced_names: Vec<String> = delivered
                    .iter()
                    .filter_map(|f| crate::iroh_net::folder_rel(Path::new(f), &pair.folder))
                    .collect();
                let _ = manager.app.emit(
                    "folder-synced",
                    serde_json::json!({ "pairId": pair_id, "direction": "send", "remaining": remaining, "files": synced_names, "bytes": delivered_bytes }),
                );
                // Whole drop finished → emit a completion summary (files, total
                // bytes, elapsed, average speed) so the folder card can show it
                // like the Send/Receive tab does, then re-check convergence.
                if remaining == 0 {
                    let elapsed_ms = session_start
                        .map(|t| t.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    let avg_bps = if elapsed_ms > 0 {
                        (session_bytes as f64) / (elapsed_ms as f64 / 1000.0)
                    } else {
                        0.0
                    };
                    let _ = manager.app.emit(
                        "folder-complete",
                        serde_json::json!({
                            "pairId": pair_id,
                            "direction": "send",
                            "files": session_done,
                            "bytes": session_bytes,
                            "durationMs": elapsed_ms,
                            "avgBps": avg_bps,
                        }),
                    );
                    manager.nudge_reconcile(&pair_id);
                    // Start the next drop's summary fresh — don't let a second
                    // drop landing back-to-back inflate the next recap.
                    session_done = 0;
                    session_bytes = 0;
                    session_start = None;
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
        request_snapshot_reply: Arc<AtomicBool>,
        force_snapshot: Arc<AtomicBool>,
        pending_deletes: Arc<Mutex<Vec<DeleteEvent>>>,
        control_wake: Arc<Notify>,
        tombstones: Arc<Mutex<HashMap<String, u64>>>,
        moves: Arc<Mutex<HashMap<String, MoveRec>>>,
        ino_index: Arc<Mutex<HashMap<u64, (String, u64, u64)>>>,
        paused: Arc<AtomicBool>,
        inbound: Arc<Mutex<HashSet<String>>>,
        queue: Arc<Mutex<SendQueue>>,
        skipped: Arc<Mutex<HashMap<String, String>>>,
        self_deleted: Arc<Mutex<HashMap<String, Instant>>>,
    ) {
        let manager = self.clone();
        let pair_id = config.lock().unwrap().id.clone();
        let _ = (&queue, &self_deleted);
        // D5: deletes made while DropBeam was CLOSED never produced a watcher event.
        // Diff what we knew against the disk once at startup (re-verified after a
        // settle), and propagate the real ones — stamped with the last time we saw
        // the files, so a peer's NEWER edit made meanwhile still wins.
        {
            let (m, pid, st) = (self.clone(), pair_id.clone(), stopped.clone());
            let ts = load_last_seen(&self.config_dir, &pair_id);
            tauri::async_runtime::spawn(async move {
                if let Some(ts) = ts {
                    m.propagate_missed_deletes(&pid, ts, &st).await;
                }
            });
        }
        tauri::async_runtime::spawn(async move {
            // One-time cleanup of the croc-era `.ctrl-out-<pair>.json` — it was written
            // every round and read by NOTHING (the real control payload dials the peer
            // directly). Older builds left one behind.
            let _ = std::fs::remove_file(
                manager.config_dir.join(format!(".ctrl-out-{pair_id}.json")),
            );
            let mut offline_streak: u32 = 0;
            // Prior reconcile manifest (rel → entry), kept across rounds so the OS-agnostic
            // move detector can spot a vanished→appeared (Windows) relocation by size+mtime.
            // Task-local; no shared state needed.
            let mut prev_manifest: HashMap<String, FileEntry> = HashMap::new();
            // When the previous round STARTED — the debounce floor below keys off it.
            let mut last_round_start: Option<std::time::Instant> = None;
            loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                // DEBOUNCE: a big drop lands in many batches and every ingest nudges
                // control_wake, so without a floor this loop runs back-to-back rounds
                // (each = TWO whole-folder walks + a full manifest send) for the entire
                // multi-minute transfer, competing with the transfer itself for disk.
                // Cap round frequency at ~1/15s — a burst of nudges coalesces into ONE
                // round shortly after the drop settles. Skipped when a delete is
                // pending (deletes must propagate briskly, unchanged).
                if let Some(t0) = last_round_start {
                    let since = t0.elapsed();
                    let floor = Duration::from_secs(15);
                    if since < floor && pending_deletes.lock().unwrap().is_empty() {
                        tokio::select! {
                            _ = tokio::time::sleep(floor - since) => {}
                            _ = stop_notify.notified() => break,
                        }
                        if stopped.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                }
                last_round_start = Some(std::time::Instant::now());
                let (pair, settings) = {
                    let p = config.lock().unwrap().clone();
                    let s = manager
                        .app
                        .try_state::<Arc<AppState>>()
                        .map(|st| st.settings.lock().unwrap().clone())
                        .unwrap_or_default();
                    (p, s)
                };
                // While paused, the beacon still flows (presence + the pause state,
                // so the peer converges to paused) but carries NO sync payload — no
                // deletes, no moves, no reconcile snapshot — so nothing propagates.
                let is_paused = paused.load(Ordering::Relaxed);
                let pause_epoch = pair.pause_epoch;
                let my_name = if settings.display_name.trim().is_empty() {
                    "DropBeam user".to_string()
                } else {
                    settings.display_name.clone()
                };
                let dels: Vec<DeleteEvent> = pending_deletes.lock().unwrap().clone();

                // D6: is the folder actually there? An unplugged drive must never be
                // advertised as "I have nothing" (the peer would re-push everything
                // into a path that recreates on the boot disk), and when it comes
                // back the watcher has to be re-registered — restart the link.
                let root_ok = folder_available(&pair.folder);
                let was_missing = status.lock().map(|s| s.folder_missing).unwrap_or(false);
                if root_ok && was_missing {
                    log::info!("folder {} is back — restarting its sync", pair.folder);
                    let m = manager.clone();
                    let pid = pair_id.clone();
                    let _ = tokio::task::spawn_blocking(move || m.restart_pair(&pid)).await;
                    break;
                }
                if !root_ok && !was_missing {
                    if let Ok(mut s) = status.lock() {
                        s.folder_missing = true;
                    }
                    manager.emit_status(&pair_id);
                }

                // The self-heal reconcile snapshot: our full current file set +
                // tombstones, so the peer can converge to identical (mirror only).
                let forced = force_snapshot.swap(false, Ordering::SeqCst);
                let request_reply = request_snapshot_reply.swap(false, Ordering::SeqCst);
                let reconcile_json = if pair.mirror && root_ok && (!is_paused || forced) {
                    let lm = live_manifest_async(pair.folder.clone()).await;
                    // "Suddenly empty" (a mount point whose drive isn't mounted, a
                    // folder swapped for an empty one): withhold the snapshot rather
                    // than tell the peer we have nothing.
                    let known_rels = {
                        let inb = inbound.lock().unwrap();
                        inb.iter().filter_map(|s| sig_rel(s)).collect::<HashSet<String>>().len()
                    };
                    let suspicious = lm.is_empty() && known_rels >= 5;
                    if !suspicious {
                        // We just saw every file that's here — the D5 baseline.
                        save_last_seen(&manager.config_dir, &pair_id, now_ms());
                        // Forget "peer kept it" notes for versions that no longer
                        // exist here (renamed/edited/deleted) so the warning is live.
                        let mut sk = skipped.lock().unwrap();
                        let before = sk.len();
                        sk.retain(|sig, _| {
                            let mut it = sig.rsplitn(3, '|');
                            let mt = it.next().and_then(|x| x.parse::<u64>().ok());
                            let sz = it.next().and_then(|x| x.parse::<u64>().ok());
                            let rel = it.next().map(norm_rel);
                            match (rel, sz, mt) {
                                (Some(r), Some(sz), Some(mt)) => lm.get(&r).is_some_and(|e| e.size == sz && e.mtime == mt),
                                _ => false,
                            }
                        });
                        if sk.len() != before {
                            let snap = sk.clone();
                            drop(sk);
                            save_skipped(&manager.config_dir, &pair_id, &snap);
                            if let Ok(mut s) = status.lock() {
                                s.warning = skipped_warning(&snap, &peer_label(&pair));
                            }
                        }
                    }
                    let lm = if suspicious {
                        log::warn!("folder {} looks empty but we know {known_rels} files — not advertising an empty snapshot", pair.folder);
                        None
                    } else {
                        Some(lm)
                    };
                    if let Some(lm) = lm {
                    // Refresh the inode index from disk each round so move detection
                    // also covers files that were RECEIVED this session (those skip
                    // the live add-branch that normally warms the index).
                    if !is_paused && !pair.i_am_viewer {
                        // Catch moves the LIVE add-branch raced past: the rebuild below
                        // overwrites an inode's old path with its new one, erasing the
                        // move. Detect them FIRST by comparing the prior index against
                        // the live manifest, so a relocated file is sent as a rename op
                        // (the peer moves its copy) instead of a re-upload that orphans
                        // the old path and reconciles back to us as a DUPLICATE.
                        let detected =
                            detect_moves_from_index(&ino_index.lock().unwrap(), &prev_manifest, &lm);
                        // Surface moves in the chat timeline ("You moved X → Y") — they
                        // were previously invisible there (only file ADDS emitted). One
                        // batched event per reconcile round, so a bulk reorg is one row.
                        let mut moved_pairs: Vec<serde_json::Value> = Vec::new();
                        for (from, to, sz, mt) in detected {
                            note_move(
                                &manager.config_dir, &pair_id, &moves, &from, &to, sz, mt, now_ms(),
                            );
                            log::info!("folder move detected (reconcile): {from:?} → {to:?}");
                            moved_pairs.push(serde_json::json!({ "from": from, "to": to }));
                        }
                        if !moved_pairs.is_empty() {
                            let _ = manager.app.emit(
                                "folder-synced",
                                serde_json::json!({ "pairId": pair_id, "direction": "send", "action": "moved", "moves": moved_pairs }),
                            );
                        }
                        let mut idx = ino_index.lock().unwrap();
                        idx.clear();
                        for (rel, e) in &lm {
                            if e.inode != 0 {
                                idx.insert(e.inode, (rel.clone(), e.size, e.mtime));
                            }
                        }
                    }
                    // Remember this round's manifest so next round's OS-agnostic detector
                    // can spot a vanished→appeared (Windows) move by size+mtime.
                    prev_manifest = lm.clone();
                    let files: serde_json::Map<String, serde_json::Value> = lm
                        .iter()
                        .map(|(rel, e)| (rel.clone(), serde_json::json!([e.size, e.mtime])))
                        .collect();
                    let tombs: serde_json::Map<String, serde_json::Value> = tombstones
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(rel, ts)| (rel.clone(), serde_json::json!(ts)))
                        .collect();
                    let folder = pair.folder.clone();
                    let empty_dirs = tokio::task::spawn_blocking(move || live_empty_dirs(&folder))
                        .await.expect("empty directory scan panicked");
                    let tvers: serde_json::Map<String, serde_json::Value> = tomb_versions_of(&pair_id)
                        .into_iter()
                        .map(|(rel, (sz, mt))| (rel, serde_json::json!([sz, mt])))
                        .collect();
                    Some(serde_json::json!({ "files": files, "tombstones": tombs, "tombVersions": tvers, "emptyDirs": empty_dirs, "requestReply": request_reply }))
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Dial the peer directly and hand them the control payload.
                // Success means they're online AND received it (deletes included).
                let iroh_ok = match (pair.endpoint_id.clone(), manager.iroh_endpoint()) {
                    (Some(eid), Some(ep)) => {
                        // Paused → no deletes, no moves (hold them until Resume).
                        let del_pairs: Vec<(String, u64, Option<(u64, u64)>)> = if is_paused {
                            Vec::new()
                        } else {
                            dels.iter().map(|d| (d.rel.clone(), d.ts, d.known)).collect()
                        };
                        // Recent intra-folder renames ride the beacon ALONGSIDE the
                        // deletes (applied first on the peer) so a moved file relocates
                        // in place instead of re-downloading. Drop expired ones.
                        let move_ops: Vec<(String, String, u64, u64)> = if is_paused {
                            Vec::new()
                        } else {
                            let cutoff = now_ms().saturating_sub(MOVE_TTL_MS);
                            moves
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|(_, (_, _, _, ts))| *ts >= cutoff)
                                .map(|(from, (to, sz, mt, _))| (from.clone(), to.clone(), *sz, *mt))
                                .collect()
                        };
                        let (group_id, roster, owner_eid, role_epoch) =
                            build_group_roster(&manager.config_dir, &pair, &ep, &my_name);
                        let removed = if group_id.is_empty() {
                            Vec::new()
                        } else {
                            pairing::removed_members(&manager.config_dir, &group_id)
                        };
                        let ok = crate::iroh_net::send_folder_ctrl(
                            &ep, &eid, &pair_id, &my_name, &del_pairs, &move_ops, &group_id, &roster,
                            owner_eid.as_deref(), role_epoch, is_paused, pause_epoch, false, &removed,
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
                    // Peer is reachable → kick the file-sender so a drop parked behind
                    // the presence gate starts immediately (no 5s poll wait).
                    manager.wake_sender(&pair_id);
                    manager.emit_status(&pair_id);
                    if !dels.is_empty() && !is_paused {
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
                &ep, &eid, &pid, &my_name, &[], &[], "", &[], None, 0, false, 0, true, &[],
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
        sender_eid: Option<&str>,
    ) {
        // Surface the user's own LABEL for this peer if they set one (resolved by
        // the peer's stable endpoint id), otherwise the name the peer broadcast.
        let (eid, secret, role) = {
            let p = config.lock().unwrap();
            (sender_eid.filter(|e| !e.is_empty()).map(str::to_string)
                .or_else(|| p.endpoint_id.clone()), p.secret.clone(), p.role)
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
        // (rel, deleted-at ms, the version the deleter last had — newer peers).
        deletes: &[(String, u64, Option<(u64, u64)>)],
        // Intra-folder renames (from, to, size, mtime) — applied BEFORE deletes so a
        // moved file relocates in place instead of being re-downloaded.
        moves: &[(String, String, u64, u64)],
        group_id: &str,
        members: &[(String, String, bool)],
        // Role authority: the beacon sender's claimed owner + role epoch.
        owner_eid: Option<&str>,
        role_epoch: u64,
        // Shared pause switch from the beacon: whether the SENDER has it paused + the
        // epoch of that toggle. Newest epoch wins so both sides converge.
        peer_paused: bool,
        peer_pause_epoch: u64,
        reconcile: Option<&Reconcile>,
        unshared: bool,
        // The iroh connection's REMOTE endpoint id — i.e. who actually sent this
        // beacon. Used to key an inviter's still-unkeyed link (the original invite
        // whose folder-hello hasn't landed) so the roster gossip below recognizes the
        // sender as already-meshed instead of adding them a SECOND time.
        sender_eid: Option<&str>,
        // Group members the OWNER removed (honored only from the owner — D13).
        removed: Option<&[String]>,
        // The sender uses versioned deletes (every file entry names its version).
        deletes_versioned: bool,
    ) {
        let (config, status, self_deleted, tombstones, queue, wake, inbound, paused_flag, last_peer_snapshot, skipped) = {
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
                h.paused.clone(),
                h.last_peer_snapshot.clone(),
                h.skipped.clone(),
            )
        };
        // Adopt the shared pause state if the peer's toggle is NEWER than ours (the
        // newest-wins rule that makes pause a clean shared switch). Fans across the
        // whole folder's group links + wakes the workers so it takes effect at once.
        // An epoch more than a day in the FUTURE is ignored outright: a wild clock or
        // a hostile peer stamping u64::MAX would otherwise win every toggle forever.
        if peer_pause_epoch > now_ms().saturating_add(24 * 3600 * 1000) {
            log::warn!("ignoring a far-future pause epoch from the peer on {pair_id}");
        } else if peer_pause_epoch > sane_epoch(config.lock().unwrap().pause_epoch) {
            self.set_paused(pair_id, peer_paused, peer_pause_epoch);
        }
        // While WE are paused, ignore the peer's sync payload entirely (deletes,
        // moves, reconcile) — nothing changes our folder until Resume.
        let locally_paused = paused_flag.load(Ordering::Relaxed);
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
        // KEY THE LINK from the beacon's true sender. This beacon arrived on THIS
        // link from `sender_eid`, so that IS this link's peer. An inviter's original
        // invite link starts with endpoint_id=None until the newcomer's folder-hello
        // lands — and the periodic control beacon often beats that hello. Without
        // keying here, the roster gossip below runs `ensure_member` for the sender's
        // eid, doesn't find it on the still-unkeyed invite link, and creates a SECOND
        // link to the same person (the "adds them twice" bug). Keying from the
        // reliable beacon closes that race regardless of whether the hello arrives.
        if let Some(seid) = sender_eid {
            // Guarded + atomic: keys ONLY a genuinely-unkeyed link, and refuses if
            // that eid is already a member of this group (so a leaked invite pair_id
            // can't be replayed to claim a second slot).
            if pairing::key_unkeyed_group_link(&self.config_dir, pair_id, seid) {
                // Keying may have revealed a pre-existing duplicate link to this same
                // person (created before this fix) — collapse it now.
                pairing::dedup_group_links(&self.config_dir);
                self.clone().reconcile();
                let _ = self.app.emit("pairs://changed", ());
            }
            // Reconcile may replace the handle; update this callback's snapshot
            // before name processing so it cannot take the legacy friend path.
            if let Some(pair) = pairing::load(&self.config_dir).into_iter().find(|p| p.id == pair_id) {
                if let Ok(mut config) = config.lock() {
                    config.endpoint_id = pair.endpoint_id;
                }
            }
        }
        // Presence + name (also links the friend), same as the croc path. If no
        // name rode along, still mark them online — we just heard from them.
        let name = name.trim();
        if !name.is_empty() {
            self.on_peer_hello(pair_id, &config, name, &status, sender_eid);
        } else {
            set_peer_online(&status, true);
            self.emit_status(pair_id);
        }
        // We just heard from the peer → they're online. Kick the file-sender so any
        // drop parked behind the presence gate goes out now, not on the next poll.
        self.wake_sender(pair_id);
        // Mirror-mode delete propagation (apply_remote_delete is idempotent, so a
        // re-delivered delete is a harmless no-op). Role flags are read AFTER the
        // role block below, so this beacon's deletes/reconcile use the up-to-date
        // role even when the SAME beacon also demotes the peer to a viewer.
        let (folder, mirror) = {
            let p = config.lock().unwrap();
            (p.folder.clone(), p.mirror)
        };
        // Only trust the roster/group on a beacon whose group_id MATCHES this
        // link's own group_id — a peer can't make us create links under some other
        // group id (defense in depth; the eids still only ever point at this folder).
        let my_group = config.lock().unwrap().group_id.clone();
        let group_ok = !group_id.is_empty() && my_group.as_deref() == Some(group_id);

        // Multi-person folders: apply the roster + per-member roles FIRST, before we
        // touch any files. The beacon carries the group roster, so mesh with any
        // member we don't have a link to yet (gossip — converges the whole group and
        // self-heals if someone was offline when a person joined), then apply the
        // owner's role assignment. Doing this up front means a beacon that demotes a
        // peer to read-only takes effect for THIS beacon's deletes/reconcile, not one
        // cycle late. A classic 1:1 folder has an empty group/roster → no-op here.
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
                for (eid, mname, _viewer) in members {
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
                // Apply per-member roles from the roster: who's a viewer (read-only).
                // Owner-authoritative + monotonic epoch (apply_group_roles enforces
                // both), so only the owner's newest assignment ever takes effect.
                let roles: std::collections::HashMap<String, bool> =
                    members.iter().map(|(e, _, v)| (e.clone(), *v)).collect();
                let roles_changed = pairing::apply_group_roles(
                    &self.config_dir,
                    group_id,
                    &my_eid,
                    owner_eid,
                    role_epoch,
                    &roles,
                );
                if added || roles_changed {
                    self.clone().reconcile();
                    let _ = self.app.emit("pairs://changed", ());
                }
            }
        }

        // D13: the folder OWNER's list of removed members is authoritative — drop our
        // links to them (so they stop syncing with us too) and refuse to re-mesh
        // with them via anyone's roster. Ignored from anyone but the owner.
        if let (true, Some(removed)) = (group_ok, removed) {
            if let Some(seid) = sender_eid {
                let my_eid = self.iroh_endpoint().map(|ep| ep.id().to_string()).unwrap_or_default();
                let dropped = pairing::apply_owner_removals(&self.config_dir, group_id, seid, removed, &my_eid);
                if !dropped.is_empty() {
                    for (id, _) in &dropped {
                        self.announce_unshare(id);
                    }
                    pairing::remove_links(&self.config_dir, &dropped.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>());
                    self.clone().reconcile();
                    let _ = self.app.emit("pairs://changed", ());
                }
            }
        }

        // Role flags fresh from disk — apply_group_roles just wrote pairs.json, and
        // the in-memory worker handle isn't refreshed until the reconcile lands, so
        // re-read rather than trust the stale `config` snapshot.
        let (peer_is_viewer, i_am_viewer) = pairing::pair_roles(&self.config_dir, pair_id)
            .unwrap_or_else(|| {
                let p = config.lock().unwrap();
                (p.peer_is_viewer, p.i_am_viewer)
            });

        // Intra-folder renames FIRST (before deletes): relocate our copy in place so
        // the matching delete of the old path is a harmless no-op (no re-download).
        // Content-verified, so a stale/mis-detected move never touches the wrong file;
        // if it doesn't apply, the delete + reconcile backstop still converge. A
        // viewer peer can't reorganize our folder.
        if mirror && !peer_is_viewer && !locally_paused && !moves.is_empty() {
            let mut moved_any = false;
            let mut moved_pairs: Vec<serde_json::Value> = Vec::new();
            for (from, to, size, mtime) in moves {
                if apply_remote_move(
                    &folder, from, to, *size, *mtime, &self_deleted, &inbound, DELETE_GRACE_MS,
                ) {
                    moved_any = true;
                    moved_pairs.push(serde_json::json!({ "from": from, "to": to }));
                    let from_n = norm_rel(from);
                    inbound
                        .lock()
                        .unwrap()
                        .retain(|sig| sig_rel(sig).as_deref() != Some(from_n.as_str()));
                }
            }
            if moved_any {
                let _ = self.app.emit("folder-history://changed", pair_id);
                // Surface the peer's reorganization in the chat timeline too, so moves
                // are bidirectional like adds ("<name> moved X → Y"). Provenance name
                // mirrors note_received; empty → null so the UI falls back to the label.
                let from_name = {
                    let p = config.lock().unwrap();
                    p.endpoint_id
                        .as_deref()
                        .and_then(|e| friends::label_for_endpoint(&self.config_dir, e))
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| p.peer_name.clone())
                };
                let from_opt = (!from_name.trim().is_empty()).then(|| from_name);
                let _ = self.app.emit(
                    "folder-synced",
                    serde_json::json!({ "pairId": pair_id, "direction": "receive", "action": "moved", "moves": moved_pairs, "from": from_opt }),
                );
            }
        }

        // A VIEWER peer must never delete our files: ignore deletes coming FROM a
        // read-only member (they shouldn't be changing the folder at all).
        if mirror && !peer_is_viewer && !locally_paused && !deletes.is_empty() && folder_available(&folder) {
            let mut applied: Vec<(String, u64, Option<(u64, u64)>)> = Vec::new();
            let mut tomb_changed = false;
            let mut deleted_sigs: Vec<String> = Vec::new();
            // D4: a directory delete removes only what the deleter LISTED (its
            // children ride the same beacon, each with the version it had) — never a
            // file it never saw. Files first, directories after, so a directory is
            // only removed once its listed children are gone and it's empty.
            let listed: HashSet<String> = deletes.iter().map(|(r, _, _)| norm_rel(r)).collect();
            let (dirs, files): (Vec<_>, Vec<_>) = deletes
                .iter()
                .partition(|(r, _, _)| Path::new(&folder).join(norm_rel(r)).is_dir());
            for (rel, ts, known) in files.into_iter().chain(dirs) {
                let mut removed_rels: Vec<String> = Vec::new();
                let abs = Path::new(&folder).join(norm_rel(rel));
                let sig = file_sig(&abs.to_string_lossy(), &folder);
                let guard = DeleteGuard { ts: *ts, known: *known, listed: Some(&listed), require_known: deletes_versioned };
                let did = apply_remote_delete(&folder, rel, &self_deleted, &mut removed_rels, DELETE_GRACE_MS, &guard);
                // If the target still exists — freshly dropped, a NEWER local edit,
                // a different file under a case-variant name, or a directory with
                // unlisted files — do NOT tombstone or forward it: that would just
                // spread a delete that doesn't apply here. Let the local copy win.
                if !did && std::fs::symlink_metadata(&abs).is_ok() {
                    continue;
                }
                if did {
                    if let Some(sig) = sig {
                        deleted_sigs.push(sig);
                    }
                }
                // Tombstone the rel(s) so reconcile won't resurrect them and so the
                // delete keeps propagating across a group. Always tombstone the
                // named rel (even a no-op re-delivery) at the peer's timestamp.
                tomb_changed |= note_tombstone(&tombstones, rel, *ts);
                if let Some(v) = known {
                    note_tomb_version(pair_id, rel, *v, *ts);
                }
                for r in &removed_rels {
                    tomb_changed |= note_tombstone(&tombstones, r, *ts);
                }
                if did {
                    applied.push((rel.clone(), *ts, *known));
                    // These files are gone by the peer's hand: forget them in the
                    // loop-guard manifest too (else they'd read as "known but
                    // missing" forever — the suspicious-empty check would withhold
                    // our snapshot, and the startup scan would re-propagate them).
                    let gone: HashSet<String> = removed_rels.iter().cloned().chain(std::iter::once(norm_rel(rel))).collect();
                    inbound.lock().unwrap().retain(|sig| sig_rel(sig).map_or(true, |r| !gone.contains(&norm_rel(&r))));
                }
            }
            // ONE write for the whole beacon's deletes (a 1,000-file bulk delete used
            // to rewrite the entire tombstone file per entry — gigabytes of IO).
            if tomb_changed {
                persist_tombstones(&self.config_dir, pair_id, &tombstones);
                persist_tomb_versions(pair_id);
            }
            record_deleted_versions(&self.config_dir, &folder, &deleted_sigs, now_ms());
            if !applied.is_empty() {
                let snapshot = inbound.lock().unwrap().clone();
                self.persist_manifest(pair_id, &snapshot);
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

        // Stash the peer's full snapshot for the "Verify" button to compare against
        // — recorded even while paused (so a Verify is honest about the current
        // divergence) and even for a viewer peer (Verify only READS, never deletes).
        if let Some(rec) = reconcile {
            *last_peer_snapshot.lock().unwrap() = Some((rec.clone(), now_ms()));
        }
        // Self-heal reconcile: the peer told us its full file set + tombstones.
        // Apply any deletes we missed, and queue any files the peer is missing —
        // the bulletproof double-check that both folders converge to identical.
        // (Skipped while paused — convergence resumes on Resume.)
        if mirror && !locally_paused {
            if let Some(rec) = reconcile {
                // Record the peer's file count for the "both have N files, in sync"
                // visibility indicator.
                if let Ok(mut s) = status.lock() {
                    s.peer_files = rec.files.len() as u32;
                }
                if folder_available(&folder) {
                    self.reconcile_apply(
                        pair_id, &folder, rec, &self_deleted, &tombstones, &queue, &wake, &inbound,
                        &skipped, !peer_is_viewer, !i_am_viewer,
                    );
                }
                self.emit_status(pair_id);
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
        queue: &Arc<Mutex<SendQueue>>,
        wake: &Arc<Notify>,
        inbound: &Arc<Mutex<HashSet<String>>>,
        skipped: &Arc<Mutex<HashMap<String, String>>>,
        // Per-member roles: don't apply a VIEWER peer's deletes/tombstones (they
        // can't change the folder); don't push if WE are a viewer (read-only).
        apply_peer_deletes: bool,
        push_missing: bool,
    ) {
        let mine = live_manifest(folder);
        let my_tomb = tombstones.lock().unwrap().clone();
        let plan = reconcile_plan_ci(
            &mine,
            &rec.files,
            &rec.tombstones,
            &my_tomb,
            now_ms(),
            folder_case_insensitive(folder),
        );

        // 1) Apply peer tombstones we missed: delete a local file the peer deleted
        //    AFTER our copy. Archive first (recoverable from history).
        if apply_peer_deletes && !plan.delete.is_empty() {
            log::warn!(
                "reconcile[{pair_id}]: applying {} peer delete(s){}",
                plan.delete.len(),
                if plan.delete.len() <= 12 {
                    format!(": {:?}", plan.delete)
                } else {
                    String::new()
                }
            );
        }
        let mut deleted_any = false;
        if apply_peer_deletes {
            let mut tomb_changed = false;
            let mut deleted_sigs: Vec<String> = Vec::new();
            for rel in &plan.delete {
                let Some(tomb_ts) = rec.tombstones.get(rel).copied().or_else(|| {
                    rec.tombstones.iter().find(|(k, _)| norm_rel(k) == *rel).map(|(_, v)| *v)
                }) else {
                    continue;
                };
                let mut removed: Vec<String> = Vec::new();
                let sig = file_sig(&Path::new(folder).join(rel).to_string_lossy(), folder);
                // The precise placement version is re-checked inside (D4/D15): the
                // plan's view can be a few ms stale, and on platforms without ctime
                // a moved-in file is only "new" by first sight.
                // A peer that tracks tombstone versions names the version each one
                // deleted; a tombstone it can't vouch for (a directory, a name it
                // never had) never removes a FILE here (D4). Legacy peers: ts rule.
                let Some(guard) = reconcile_delete_guard(rec, rel, tomb_ts) else {
                    log::info!("reconcile[{pair_id}]: keeping {rel:?} — the peer's tombstone doesn't name a version we hold");
                    continue;
                };
                if apply_remote_delete(folder, rel, self_deleted, &mut removed, DELETE_GRACE_MS, &guard) {
                    deleted_any = true;
                    deleted_sigs.extend(sig);
                    for r in &removed {
                        tomb_changed |= note_tombstone(tombstones, r, tomb_ts);
                        inbound
                            .lock()
                            .unwrap()
                            .retain(|sig| sig_rel(sig).as_deref() != Some(r.as_str()));
                    }
                }
            }
            // Adopt EVERY peer tombstone so we forward it and never resurrect, even
            // the ones for files we never had — EXCEPT a rel whose LOCAL file is
            // freshly added (within grace). Adopting+forwarding that would spread a
            // delete that is racing the user's drop. The peer re-advertises its
            // tombstones every cycle, so a GENUINE delete still lands once the file
            // ages past the window. (Skipped entirely for a viewer peer — we never
            // trust a read-only member's deletes.)
            for (rel, &ts) in &rec.tombstones {
                let abs = Path::new(folder).join(norm_rel(rel));
                if file_is_fresh(&abs, DELETE_GRACE_MS) {
                    continue;
                }
                // A local file NEWER than the tombstone survives it — don't adopt (and
                // forward) a delete that doesn't apply to what we hold.
                let version = rec.tomb_versions.as_ref().and_then(|tv| tv.get(rel).copied());
                if abs.is_file() && !delete_allowed(&abs, ts, version) {
                    continue;
                }
                tomb_changed |= note_tombstone(tombstones, rel, ts);
                if let Some(v) = version {
                    note_tomb_version(pair_id, rel, v, ts);
                }
            }
            record_deleted_versions(&self.config_dir, folder, &deleted_sigs, now_ms());
            // ONE write for the whole snapshot (a new member adopting a 9k-entry
            // tombstone set used to rewrite the ~MB file once per entry ≈ GBs of IO).
            if tomb_changed {
                persist_tombstones(&self.config_dir, pair_id, tombstones);
                persist_tomb_versions(pair_id);
            }
            if deleted_any {
                let _ = self.app.emit("folder-history://changed", pair_id);
            }
        }

        // 2) Push files the peer is missing or has an older copy of. Skipped when
        //    WE are a viewer — a read-only member never sends.
        let mut queued = 0usize;
        if push_missing {
            // Versions the peer already answered "kept" for (a name its disk can't
            // hold, a case clash, a version it deleted) aren't re-pushed until the
            // file changes — that's what keeps those from looping every round.
            let sk = skipped.lock().unwrap().clone();
            let cands: Vec<String> = plan
                .push
                .iter()
                .map(|rel| Path::new(folder).join(rel).to_string_lossy().to_string())
                .filter(|abs| Path::new(abs).is_file())
                .filter(|abs| file_sig(abs, folder).map_or(true, |sig| !sk.contains_key(&sig)))
                .collect();
            let mut q = queue.lock().unwrap();
            for abs in cands {
                if q.push_back(abs) {
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
        apply_empty_dirs(folder, &rec.empty_dirs, &my_tomb, &rec.tombstones);
        // Honor a peer's directory delete: remove an empty dir the peer tombstoned.
        // Gated on apply_peer_deletes so a read-only VIEWER can't erase our folder
        // structure; freshness-guarded so a just-created (still-empty) dir isn't
        // removed under a stale tombstone; NFC-normalized to match on-disk names.
        if apply_peer_deletes {
            for (rel, _) in &rec.tombstones {
                let norm = norm_rel(rel);
                // "" would be the folder ROOT itself; "../x" or an absolute rel
                // would reach outside the shared folder.
                if !rel_stays_inside(&norm) {
                    continue;
                }
                let abs = Path::new(folder).join(&norm);
                if abs.is_dir() && !file_is_fresh(&abs, DELETE_GRACE_MS) {
                    self_deleted
                        .lock()
                        .unwrap()
                        .insert(norm.clone(), Instant::now());
                    let _ = std::fs::remove_dir(&abs); // only succeeds if empty
                }
            }
        }
    }

    /// Forward a set of just-applied mirror deletes to every OTHER link in the
    /// same folder group, so a delete propagates across the whole mesh (not just
    /// the one hop from the origin). Dedups so a delete isn't queued twice.
    fn fan_group_deletes(&self, from_pair: &str, group_id: &str, deletes: &[(String, u64, Option<(u64, u64)>)]) {
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
                    for (rel, ts, known) in deletes {
                        if !pd.iter().any(|d| d.rel == *rel && d.ts == *ts) {
                            pd.push(DeleteEvent {
                                rel: rel.clone(),
                                ts: *ts,
                                known: *known,
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

    /// Per landed batch: provenance stamps + the chat timeline's "added" row.
    fn note_received_batch(&self, pair: &Pair, files: &[String]) {
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
        let total: u64 = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
            .sum();
        // Relative names of the files just received, for the chat timeline (#23).
        let synced_names: Vec<String> = files
            .iter()
            .filter_map(|f| crate::iroh_net::folder_rel(Path::new(f), &pair.folder))
            .collect();
        // `from` = who these files came from (your saved label for them, else their
        // broadcast name), so the chat sync row can show provenance (#12). Only on the
        // receive path; the send row already reads "You added". Carry it ONLY when
        // non-empty — an unkeyed link can still have an empty peer_name until the name
        // beacon lands, and a blank "from" would render an empty sender. Null lets the
        // UI's `?? friendName` fallback fire.
        let from_opt = (!from.trim().is_empty()).then(|| from.clone());
        let _ = self.app.emit(
            "folder-synced",
            serde_json::json!({ "pairId": pair.id, "direction": "receive", "files": synced_names, "bytes": total, "from": from_opt }),
        );
    }

    /// Per receive BURST: one History row + one notification.
    fn note_received_burst(&self, pair: &Pair, files: &[String]) {
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
                integrity: vec![],
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
            // Read like a chat notification (the user asked for sync activity to
            // ping the same way a message does): sender as the title, what+where as
            // the body, and an audible sound so it isn't a silent banner.
            let (title, body) = match &from {
                Some(name) => (
                    name.clone(),
                    format!("Added {what} to {}", folder_name(&pair.folder)),
                ),
                None => (
                    "Shared folder".to_string(),
                    format!("{what} arrived in {}", folder_name(&pair.folder)),
                ),
            };
            let _ = self
                .app
                .notification()
                .builder()
                .title(title)
                .body(body)
                .sound("default")
                .show();
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

    /// A folder receive ended WITHOUT landing files (failed, or dropped because the
    /// folder is paused): drop the "Receiving 43%" status back to Idle. Otherwise a
    /// receive-only link (viewer / one-way) — which has no sender loop to overwrite
    /// its status — shows a frozen progress bar until the next transfer.
    pub fn note_folder_receive_ended(&self, pair_id: &str) {
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            if let Ok(mut s) = h.status.lock() {
                if !matches!(s.state, FolderState::Receiving) {
                    return; // a sender state took over meanwhile — leave it be
                }
                s.state = FolderState::Idle;
                s.percent = 0.0;
                s.sending_file = None;
            }
        }
        self.emit_status(pair_id);
    }

    /// Land iroh-received folder files: move them from the private staging dir into
    /// the shared folder using the EXACT same loop-protection / mirror / history
    /// rules as the croc path (shared `inbound` + `self_deleted` guards), then
    /// update the manifest, record history, and reset status to Idle. Returns what
    /// happened to EVERY item — the receipt the sender trusts (D1).
    pub(crate) fn ingest_iroh_folder_files(
        &self,
        pair_id: &str,
        staging: &Path,
        items: &serde_json::Value,
    ) -> IngestReport {
        let (config, inbound, self_deleted, recv_burst) = {
            let handles = self.handles.lock().unwrap();
            let Some(h) = handles.get(pair_id) else {
                return IngestReport::default();
            };
            // Paused → don't land incoming files. The push is normally refused
            // up front; this covers a toggle landing mid-receive. Nothing is
            // reported as landed, so the sender keeps its copy and retries later.
            if h.paused.load(Ordering::Relaxed) {
                drop(handles);
                self.note_folder_receive_ended(pair_id);
                return IngestReport::default();
            }
            (h.config.clone(), h.inbound.clone(), h.self_deleted.clone(), h.recv_burst.clone())
        };
        let (folder, mirror, group) = {
            let p = config.lock().unwrap();
            (p.folder.clone(), p.mirror, p.group_id.is_some())
        };
        // The sender's placement version per item (newer senders), keyed like the
        // ingest outcomes.
        let vers: HashMap<String, u64> = items
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|it| {
                        let raw = it.get("name")?.as_str()?;
                        let ver = it.get("ver")?.as_u64()?;
                        let rel = crate::iroh_net::folder_receive_rel(raw)?;
                        Some((norm_rel(&rel.to_string_lossy()), ver))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cfg = self.config_dir.clone();
        let f2 = folder.clone();
        let ctx = LandCtx {
            vers,
            deleted_at: Some(Box::new(move |sig: &str| deleted_version_at(&cfg, &f2, sig))),
        };
        let report = move_staged_into_folder(staging, &folder, &inbound, mirror, group, &self_deleted, &ctx);
        if !report.moved.is_empty() {
            let snapshot = inbound.lock().unwrap().clone();
            self.persist_manifest(pair_id, &snapshot);
            let pair = config.lock().unwrap().clone();
            self.note_received_batch(&pair, &report.moved);
            // ONE History row + ONE notification per receive BURST (a 3,000-photo
            // sync arrives as ~100 batches — that used to be 100 dings). Flushed
            // once no batch has landed for a few seconds.
            let gen = {
                let mut b = recv_burst.lock().unwrap();
                b.0.extend(report.moved.iter().cloned());
                b.1 = b.1.wrapping_add(1);
                b.1
            };
            if let Some(sm) = self.app.try_state::<Arc<SyncManager>>().map(|s| s.inner().clone()) {
                let burst = recv_burst.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    let files = {
                        let mut b = burst.lock().unwrap();
                        if b.1 != gen {
                            return; // more landed since — the later flush covers it
                        }
                        std::mem::take(&mut b.0)
                    };
                    if !files.is_empty() {
                        sm.note_received_burst(&pair, &files);
                    }
                });
            }
        }
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            if let Ok(mut s) = h.status.lock() {
                s.state = FolderState::Idle;
                s.percent = 0.0;
                s.sending_file = None;
            }
        }
        self.emit_status(pair_id);
        // Right after landing files, kick the control beacon to re-exchange our
        // manifest + tombstones so both sides re-verify they're identical (and any
        // delete/missed-file converges) instead of waiting out the idle cadence.
        self.nudge_reconcile(pair_id);
        report
    }

    /// S3: is `sender_eid` THIS link's peer? A folder message may only come from
    /// the endpoint the link is keyed to. A still-unkeyed invite link is keyed
    /// from the sender first (guarded: never a second slot for an existing
    /// member), exactly as the beacon path does.
    pub fn authorize_folder_peer(self: &Arc<Self>, pair_id: &str, sender_eid: &str) -> bool {
        let Some(cfg) = self.handles.lock().unwrap().get(pair_id).map(|h| h.config.clone()) else {
            return false;
        };
        let eid = cfg.lock().unwrap().endpoint_id.clone();
        // The live config can lag pairs.json (a folder-hello keyed the link a
        // moment ago and the worker refresh hasn't landed): trust the file.
        let on_disk = || {
            pairing::load(&self.config_dir)
                .into_iter()
                .find(|p| p.id == pair_id)
                .and_then(|p| p.endpoint_id)
        };
        match eid {
            Some(e) if e == sender_eid => true,
            Some(_) => {
                if on_disk().as_deref() == Some(sender_eid) {
                    cfg.lock().unwrap().endpoint_id = Some(sender_eid.to_string());
                    return true;
                }
                false
            }
            None => {
                if !pairing::key_unkeyed_group_link(&self.config_dir, pair_id, sender_eid) {
                    if on_disk().as_deref() == Some(sender_eid) {
                        cfg.lock().unwrap().endpoint_id = Some(sender_eid.to_string());
                        return true;
                    }
                    return false;
                }
                cfg.lock().unwrap().endpoint_id = Some(sender_eid.to_string());
                pairing::dedup_group_links(&self.config_dir);
                self.clone().reconcile();
                let _ = self.app.emit("pairs://changed", ());
                true
            }
        }
    }

    /// Why we won't accept a folder push from `sender_eid` right now, if we won't —
    /// decided before reading any of it.
    pub fn folder_push_refusal(self: &Arc<Self>, pair_id: &str, sender_eid: &str) -> Option<&'static str> {
        if !self.has_pair(pair_id) {
            return Some("unknown-pair");
        }
        if !self.authorize_folder_peer(pair_id, sender_eid) {
            log::warn!("refusing folder push for {pair_id} from a non-member ({sender_eid})");
            return Some("not-member");
        }
        let handles = self.handles.lock().unwrap();
        let h = handles.get(pair_id)?;
        let (peer_viewer, folder) = {
            let c = h.config.lock().unwrap();
            (c.peer_is_viewer, c.folder.clone())
        };
        // ROLE ENFORCEMENT (receive side): a read-only VIEWER must never push
        // files into our folder — the authoritative check, not the sender's.
        if peer_viewer {
            return Some("viewer");
        }
        if h.paused.load(Ordering::Relaxed) {
            return Some("paused");
        }
        if !folder_available(&folder) {
            return Some("folder-missing");
        }
        None
    }

    /// Stop and restart one link's workers (e.g. its folder came back after the
    /// drive was reconnected — the watcher must be registered afresh).
    fn restart_pair(self: &Arc<Self>, pair_id: &str) {
        self.stop_pair(pair_id);
        self.reconcile();
    }

    /// Pause/resume from THIS device's UI. The epoch is max(now, stored + 1) so a
    /// toggle always beats the one it replaces even when our clock is behind the
    /// device that set the last one (D16).
    pub fn set_paused_now(self: &Arc<Self>, pair_id: &str, paused: bool) {
        let stored = self
            .handles
            .lock()
            .unwrap()
            .get(pair_id)
            .map(|h| h.config.lock().unwrap().pause_epoch)
            .or_else(|| {
                pairing::load(&self.config_dir)
                    .into_iter()
                    .find(|p| p.id == pair_id)
                    .map(|p| p.pause_epoch)
            })
            .unwrap_or(0);
        self.set_paused(pair_id, paused, next_pause_epoch(now_ms(), stored));
    }

    /// D5: find files we knew that vanished without a watcher event (deleted while
    /// DropBeam was closed, or lost to a watcher overflow), re-verify after a
    /// settle, and propagate them as ordinary deletes stamped `ts` (when we last
    /// saw them) and carrying the exact version we had.
    pub(crate) async fn propagate_missed_deletes(self: &Arc<Self>, pair_id: &str, ts: u64, stopped: &Arc<AtomicBool>) {
        let grab = || {
            let handles = self.handles.lock().ok()?;
            let h = handles.get(pair_id)?;
            let c = h.config.lock().ok()?.clone();
            Some((
                c,
                h.inbound.clone(),
                h.tombstones.clone(),
                h.pending_deletes.clone(),
                h.control_wake.clone(),
                h.stop_notify.clone(),
            ))
        };
        let Some((pair, inbound, tombstones, pending, cw, stop_notify)) = grab() else { return };
        if !pair.mirror || pair.i_am_viewer {
            return;
        }
        let scan = {
            let (f, i, t) = (pair.folder.clone(), inbound.clone(), tombstones.clone());
            move || {
                let inb = i.lock().unwrap().clone();
                let tb = t.lock().unwrap().clone();
                missed_deletes(&f, &inb, &tb)
            }
        };
        let first = tokio::task::spawn_blocking(scan.clone()).await.unwrap_or_default();
        if first.is_empty() {
            return;
        }
        // Settle like the live delete path does: a folder being replaced or a
        // drive mid-remount must not read as a mass delete.
        if !sleep_unless_stopped(&stop_notify, stopped, 20_000).await {
            return;
        }
        let second = tokio::task::spawn_blocking(scan).await.unwrap_or_default();
        let confirmed: Vec<(String, (u64, u64))> = first
            .into_iter()
            .filter(|(r, v)| second.get(r) == Some(v))
            .collect();
        if confirmed.is_empty() || stopped.load(Ordering::SeqCst) {
            return;
        }
        log::warn!(
            "folder {}: {} file(s) were deleted while DropBeam wasn't watching — propagating{}",
            pair.folder,
            confirmed.len(),
            if confirmed.len() <= 12 { format!(": {:?}", confirmed.iter().map(|(r, _)| r).collect::<Vec<_>>()) } else { String::new() }
        );
        let mut tomb_changed = false;
        {
            let mut pd = pending.lock().unwrap();
            for (rel, known) in &confirmed {
                tomb_changed |= note_tombstone(&tombstones, rel, ts);
                note_tomb_version(pair_id, rel, *known, ts);
                if !pd.iter().any(|d| &d.rel == rel) {
                    pd.push(DeleteEvent { rel: rel.clone(), ts, known: Some(*known) });
                }
            }
        }
        if tomb_changed {
            persist_tombstones(&self.config_dir, pair_id, &tombstones);
            persist_tomb_versions(pair_id);
        }
        let sigs: Vec<String> = confirmed.iter().map(|(r, (sz, mt))| format!("{r}|{sz}|{mt}")).collect();
        record_deleted_versions(&self.config_dir, &pair.folder, &sigs, ts);
        {
            let gone: HashSet<&String> = confirmed.iter().map(|(r, _)| r).collect();
            let mut inb = inbound.lock().unwrap();
            inb.retain(|sig| sig_rel(sig).map_or(true, |r| !gone.contains(&norm_rel(&r))));
            let snapshot = inb.clone();
            drop(inb);
            self.persist_manifest(pair_id, &snapshot);
        }
        cw.notify_one();
    }

    /// Kick the control beacon for `pair_id` to re-exchange its manifest + tombstones
    /// NOW rather than waiting out the idle cadence — so the two folders re-check that
    /// they're identical the moment a transfer finishes (the "run a check every time a
    /// file is sent or received" convergence the user asked for). Cheap + idempotent.
    pub fn nudge_reconcile(&self, pair_id: &str) {
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            h.control_wake.notify_one();
        }
    }

    /// Kick the file-SENDER worker for `pair_id` — used the moment the peer comes
    /// back online so a drop that's been parked behind the presence gate starts
    /// sending immediately instead of waiting out the gate's 5s re-check poll.
    pub fn wake_sender(&self, pair_id: &str) {
        if let Some(h) = self.handles.lock().unwrap().get(pair_id) {
            h.wake.notify_one();
        }
    }

    /// Set the shared pause switch for `pair_id`'s folder at `epoch`. Persists +
    /// flips the live flag on EVERY link of the folder (a group pauses as one),
    /// wakes each sender (park on pause / flush on resume), and kicks each control
    /// sender so the new state beacons to peers immediately. Either side may toggle;
    /// the newest epoch wins so both converge.
    pub fn set_paused(self: &Arc<Self>, pair_id: &str, paused: bool, epoch: u64) {
        let affected = pairing::set_pause(&self.config_dir, pair_id, paused, epoch);
        {
            let handles = self.handles.lock().unwrap();
            for pid in &affected {
                if let Some(h) = handles.get(pid) {
                    {
                        let mut p = h.config.lock().unwrap();
                        p.paused = paused;
                        p.pause_epoch = epoch;
                    }
                    h.paused.store(paused, Ordering::Relaxed);
                    h.wake.notify_one();
                    h.control_wake.notify_one();
                }
            }
        }
        for pid in &affected {
            self.emit_status(pid);
        }
        let _ = self.app.emit("pairs://changed", ());
    }

    /// Try to push one folder file directly over iroh. Returns `Some(locality)` on
    /// confirmed delivery, or `None` when the direct path is unavailable or fails
    /// — in which case the caller falls back to the croc relay, so folders keep
    /// working even if the peer is offline or on an older build.
    async fn try_iroh_folder_send(
        self: &Arc<Self>,
        pair: &Pair,
        _settings: &Settings,
        files: &[String],
        status: &Arc<Mutex<StatusSnapshot>>,
        stopped: &Arc<AtomicBool>,
        skip_current: &Arc<AtomicBool>,
    ) -> FolderSendOutcome {
        let Some(eid) = pair.endpoint_id.clone() else { return FolderSendOutcome::Failed };
        let Some(ep) = self.iroh_endpoint() else { return FolderSendOutcome::Failed };
        // Final chokepoint: never send DropBeam's own control paths (the leaked
        // visible `dropbeam-history` etc.), whatever queued them. If that leaves
        // nothing to send, bail cleanly.
        let files: Vec<String> = files
            .iter()
            .filter(|f| {
                crate::iroh_net::folder_rel(Path::new(f), &pair.folder)
                    .map(|rel| !crate::iroh_net::is_control_rel(&rel))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        if files.is_empty() {
            return FolderSendOutcome::Report(crate::iroh_net::FolderSendReport {
                locality: Locality::Unknown,
                items: Vec::new(),
                legacy: false,
            });
        }
        let file = files.first().cloned().unwrap_or_default();
        let file = file.as_str();
        let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
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
            || { last_progress.store(now_ms(), Ordering::Relaxed); },
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
            Ok(report) => {
                if let Ok(mut s) = status.lock() {
                    if !matches!(report.locality, Locality::Unknown) {
                        s.locality = report.locality;
                    }
                }
                FolderSendOutcome::Report(report)
            }
            Err(e) => {
                // An explicit refusal (unshared, viewer, paused, drive missing) is
                // its own outcome so the sender can explain it and back off.
                if let Some(r) = e.downcast_ref::<crate::iroh_net::FolderRefused>() {
                    log::info!("folder send refused by peer: {}", r.0);
                    return FolderSendOutcome::Refused(r.0.clone());
                }
                // EVERYTHING else is "not delivered" — including an empty or
                // unparseable final answer. (That used to be waved through as a
                // soft success, which let "delete after delivery" delete files the
                // peer never kept.) Identical re-deliveries are a cheap no-op on the
                // receiver, so retrying is always safe.
                log::debug!("folder send failed, will retry: {e:#}");
                FolderSendOutcome::Failed
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
            let paused = h.paused.load(Ordering::Relaxed);
            (queued, queued_files, s, eid, paused)
        };
        let (queued, queued_files, mut s, eid, paused) = snapshot;
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
            session_total_files: s.session_total_files,
            session_done_files: s.session_done_files,
            paused,
            conn_detail: None,
            folder_missing: s.folder_missing,
            warning: s.warning.clone(),
        };
        let _ = self.app.emit("folder://status", apply_missing_folder(status));
    }
}

// ---------------------------------------------------------------------------
// croc send / receive runners
// ---------------------------------------------------------------------------

/// How a reconcile tombstone may delete `rel` here (D4): by the version the peer
/// says it deleted, or — legacy peers only — by timestamp. `None` = it may not.
fn reconcile_delete_guard(rec: &Reconcile, rel: &str, tomb_ts: u64) -> Option<DeleteGuard<'static>> {
    match &rec.tomb_versions {
        Some(tv) => {
            let v = tv
                .get(rel)
                .or_else(|| tv.iter().find(|(k, _)| norm_rel(k) == rel).map(|(_, v)| v))?;
            Some(DeleteGuard { ts: tomb_ts, known: Some(*v), listed: None, require_known: true })
        }
        None => Some(DeleteGuard { ts: tomb_ts, known: None, listed: None, require_known: false }),
    }
}

/// A stored pause epoch from a far-future clock (or a hostile beacon accepted by an
/// older build) would win every comparison forever: treat it as 0.
fn sane_epoch(e: u64) -> u64 {
    if e > now_ms().saturating_add(24 * 3600 * 1000) { 0 } else { e }
}

/// D16: a local toggle beats the one it replaces even when our clock is behind.
fn next_pause_epoch(now: u64, stored: u64) -> u64 {
    let stored = if stored > now.saturating_add(24 * 3600 * 1000) { 0 } else { stored };
    now.max(stored.saturating_add(1))
}

/// Viewer → editor: record every file currently on disk as known (loop-guard
/// manifest) and as "peer has its own copy" (so reconcile doesn't push it), until
/// the user changes it. Returns how many files were baselined.
fn baseline_on_promotion(config_dir: &Path, pair: &Pair) -> usize {
    if !folder_available(&pair.folder) {
        return 0;
    }
    let mut manifest = load_manifest(config_dir, &pair.id);
    let mut skipped = load_skipped(config_dir, &pair.id);
    // Deletes made while read-only aren't the editor's to propagate either: forget
    // known files that are gone, so the startup missed-delete scan doesn't read
    // them as "deleted while closed" and push them to everyone.
    let present = live_manifest(&pair.folder);
    let before = manifest.len();
    manifest.retain(|sig| sig_rel(sig).is_some_and(|r| present.contains_key(&norm_rel(&r))));
    let forgotten = before - manifest.len();
    if forgotten > 0 {
        log::info!("folder {}: promoted to editor — not propagating {forgotten} read-only-era deletion(s)", pair.folder);
    }
    let mut n = 0;
    for f in list_files_rec(Path::new(&pair.folder)) {
        let p = f.to_string_lossy().to_string();
        let Some(rel) = rel_path_of(&p, &pair.folder) else { continue };
        if rel.split('/').any(|c| c.starts_with('.')) || crate::iroh_net::is_control_rel(&rel) {
            continue;
        }
        if let Some(sig) = file_sig(&p, &pair.folder) {
            manifest.insert(sig.clone());
            skipped.entry(sig).or_insert_with(|| "promoted".to_string());
            n += 1;
        }
    }
    save_manifest(config_dir, &pair.id, &manifest);
    save_skipped(config_dir, &pair.id, &skipped);
    log::info!("folder {}: promoted to editor — {n} existing file(s) won't be pushed until changed", pair.folder);
    n
}

/// A folder that isn't on disk outranks every other state (D6): nothing can sync
/// until it's back, so the card must say so — an ERROR with the reason — instead of
/// a reassuring "Up to date".
fn apply_missing_folder(mut status: FolderStatus) -> FolderStatus {
    if status.folder_missing {
        status.state = FolderState::Error;
        status.detail = Some(
            "Folder not found — reconnect the drive it's on (or it was moved or renamed). Nothing syncs until it's back."
                .to_string(),
        );
        status.percent = 0.0;
        status.sending_file = None;
    }
    status
}

/// What one folder push means for each queued file (D1 + D2).
#[derive(Default, Debug, Clone, PartialEq)]
struct BatchSettle {
    /// Landed on the peer AND still exactly the version we sent.
    delivered: Vec<String>,
    /// The peer deliberately kept its own copy (reason) — don't resend this version.
    kept: Vec<(String, String)>,
    /// Didn't land, or changed while sending — send again later.
    retry: Vec<String>,
    /// Vanished locally before it was sent.
    gone: Vec<String>,
}

fn settle_batch(
    batch: &[String],
    pre: &HashMap<String, (u64, u64)>,
    report: &crate::iroh_net::FolderSendReport,
) -> BatchSettle {
    use crate::iroh_net::FolderItemOutcome as O;
    let reported: HashMap<String, O> = report
        .items
        .iter()
        .map(|(p, o)| (p.to_string_lossy().to_string(), o.clone()))
        .collect();
    let mut out = BatchSettle::default();
    for f in batch {
        let now_v = std::fs::metadata(f).ok().map(|m| (m.len(), meta_mtime(&m)));
        if now_v.is_none() {
            out.gone.push(f.clone());
            continue;
        }
        let unchanged = now_v == pre.get(f).copied();
        match reported.get(f) {
            Some(O::Landed) if unchanged => out.delivered.push(f.clone()),
            Some(O::Kept(why)) if unchanged => out.kept.push((f.clone(), why.clone())),
            Some(O::Landed) | Some(O::Kept(_)) => {
                log::info!("folder send: {} changed during send — sending the new version", file_name_of(f));
                out.retry.push(f.clone());
            }
            Some(O::Failed) | None => out.retry.push(f.clone()),
        }
    }
    out
}

/// The files "delete after delivery" may remove: exactly the delivered ones, and
/// only when the receiver vouched for each file (never on a legacy bare "ok").
fn auto_delete_targets(settled: &BatchSettle, legacy: bool, auto_delete: bool) -> Vec<String> {
    if !auto_delete || legacy {
        return Vec::new();
    }
    settled.delivered.clone()
}

/// How one folder push ended, as the sender loop sees it.
enum FolderSendOutcome {
    /// The receiver answered; per-item outcomes inside (may be legacy "ok").
    Report(crate::iroh_net::FolderSendReport),
    /// Turned away before the body (reason code from the receiver).
    Refused(String),
    /// Unreachable, stalled, or no trustworthy answer — retry.
    Failed,
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
    // Still changing after ~25 s (a long copy, a live log, a render): NOT ready.
    // Sending now would ship a half-written file. The idle rescan picks it up
    // once it has actually settled (seed_existing checks stability too — D2).
    debounce.lock().unwrap().remove(path);
    false
}

fn file_len(path: &str) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn viewer_warning_due(last: Option<Instant>, now: Instant) -> bool {
    last.map_or(true, |last| now.saturating_duration_since(last) >= Duration::from_secs(60))
}

fn is_sendable_candidate(path: &str, folder: &str, inbound: &Arc<Mutex<HashSet<String>>>) -> bool {
    let p = Path::new(path);
    if !p.is_file() {
        return false;
    }
    // Skip the staging dir and dotfiles / temp files anywhere in the relative
    // path, plus DropBeam's own control paths (incl. the VISIBLE `dropbeam-history`
    // leaked by the old receive de-dot bug — dotless, so the dot check misses it).
    // `folder_rel` also roots aliased event paths (macOS firmlink
    // /System/Volumes/Data/..., /var vs /private/var, a symlinked root) — a bare
    // strip_prefix missed those, so `.DS_Store` & history-archive files got queued.
    // A path that can't be rooted is never sent anyway (the sender skips it).
    let Some(rel) = crate::iroh_net::folder_rel(p, folder) else {
        return false;
    };
    if rel.split('/').any(|c| c.starts_with('.')) || crate::iroh_net::is_control_rel(&rel) {
        return false;
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

/// Returns true if anything new was queued — drives the idle-rescan backoff.
fn seed_existing(
    folder: &str,
    inbound: &Arc<Mutex<HashSet<String>>>,
    queue: &Arc<Mutex<SendQueue>>,
    wake: &Arc<Notify>,
) -> bool {
    seed_existing_with(folder, inbound, queue, wake, Duration::from_millis(1500))
}

/// `settle` = how long candidates must keep the same size + mtime before they're
/// queued (one pause for the whole scan, not per file) — so a file still being
/// copied in is never sent half-written (D2).
fn seed_existing_with(
    folder: &str,
    inbound: &Arc<Mutex<HashSet<String>>>,
    queue: &Arc<Mutex<SendQueue>>,
    wake: &Arc<Notify>,
    settle: Duration,
) -> bool {
    let files = list_files_rec(Path::new(folder));
    // Run the per-file candidate checks (they stat every file) WITHOUT the
    // queue lock — holding it across a thousands-file scan would block the
    // watcher's event handler and the sender for the whole walk. The lock is
    // taken once at the end, just for the dedupe + push.
    let mut candidates: Vec<(String, Option<(u64, u64)>)> = Vec::new();
    for f in files {
        let p = f.to_string_lossy().to_string();
        // Sweep a stale "<name>.dropbeam-incoming" placeholder left by a receive
        // that was interrupted (e.g. crash mid-transfer). It's never synced —
        // just visible litter. A LIVE receive's placeholder is left alone.
        if p.ends_with(".dropbeam-incoming") {
            sweep_stale_placeholder(&f);
            continue;
        }
        if is_sendable_candidate(&p, folder, inbound) {
            let v = std::fs::metadata(&f).ok().map(|m| (m.len(), meta_mtime(&m)));
            candidates.push((p, v));
        }
    }
    if candidates.is_empty() {
        return false;
    }
    if !settle.is_zero() {
        std::thread::sleep(settle);
        candidates.retain(|(p, v)| {
            let now = std::fs::metadata(p).ok().map(|m| (m.len(), meta_mtime(&m)));
            now.is_some() && now == *v
        });
    }
    let mut any = false;
    {
        let mut q = queue.lock().unwrap();
        for (p, _) in candidates {
            any |= q.push_back(p);
        }
    }
    if any {
        wake.notify_one();
    }
    any
}

fn list_files_rec(dir: &Path) -> Vec<PathBuf> {
    let _walk = crate::fs_walk::Watch::new("list_files_rec", dir);
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = crate::fs_walk::read_dir(&d) else {
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

/// What a folder receive did with each staged item (keyed by NFC rel) — the
/// receiver's half of the per-item receipt (D1).
#[derive(Default, Debug)]
pub(crate) struct IngestReport {
    /// Absolute paths now in the folder that were newly written.
    pub moved: Vec<String>,
    pub outcomes: HashMap<String, crate::iroh_net::FolderItemOutcome>,
}

/// Per-receive facts the landing decision needs beyond the staged bytes.
#[derive(Default)]
struct LandCtx {
    /// rel → the SENDER's placement version (ms) of its copy, when it said.
    vers: HashMap<String, u64>,
    /// When this exact version (rel|size|mtime) was deleted here, if ever.
    deleted_at: Option<Box<dyn Fn(&str) -> Option<u64> + Send + Sync>>,
}

/// Move everything received in `staging` into `folder`, collision-safe.
/// Returns what happened to each item and records landed ones as inbound (loop guard).
fn move_staged_into_folder(
    staging: &Path,
    folder: &str,
    inbound: &Arc<Mutex<HashSet<String>>>,
    mirror: bool,
    group: bool,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
    ctx: &LandCtx,
) -> IngestReport {
    use crate::iroh_net::FolderItemOutcome as O;
    let folder_str = folder;
    let folder_path = Path::new(folder);
    let mut report = IngestReport::default();
    let staged = list_files_rec(staging);
    // The folder itself must be there (D6): never recreate a missing root (an
    // unplugged drive) by landing into it.
    if !folder_available(folder) {
        for src in staged {
            if let Ok(rel) = src.strip_prefix(staging) {
                report.outcomes.insert(norm_rel(&rel.to_string_lossy()), O::Failed);
            }
        }
        return report;
    }
    for src in staged {
        let Ok(rel) = src.strip_prefix(staging) else {
            continue;
        };
        let rel_str = norm_rel(&rel.to_string_lossy());
        let dest_path = folder_path.join(rel);
        let sender_ver = ctx.vers.get(&rel_str).copied();
        // REVERSE TYPE SWAP: an ancestor of this incoming FILE exists locally as a
        // FILE (a peer kept a folder where we replaced it with a same-named file).
        // create_dir_all(parent) would fail (AlreadyExists) and the file could never
        // land → reconcile re-queues it forever. Archive the blocking ancestor file
        // to History (nothing lost), loop-guard its removal so our watcher doesn't
        // echo a spurious delete, then remove it so the dir chain can be created.
        // If archiving fails, the ancestor stays and this item reports Failed.
        if mirror {
            let mut anc = rel.parent();
            while let Some(a) = anc {
                if a.as_os_str().is_empty() {
                    break;
                }
                let anc_abs = folder_path.join(a);
                if anc_abs.is_file() {
                    let arel = norm_rel(&a.to_string_lossy());
                    self_deleted.lock().unwrap().insert(arel.clone(), Instant::now());
                    crate::folder_history::archive(
                        folder_str,
                        &anc_abs.to_string_lossy(),
                        &arel,
                        "replaced",
                    );
                }
                anc = a.parent();
            }
        }
        if let Some(parent) = dest_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("move_staged_into_folder: create_dir_all {parent:?} failed: {e}");
            }
        }

        // The incoming (staged) version's identity. mtime was already stamped to
        // the origin's value by the receive path, so it's comparable across members.
        let in_size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        let in_mtime = std::fs::metadata(&src).ok().map(|m| meta_mtime(&m)).unwrap_or(0);

        // A copy of a version WE DELETED, placed before we deleted it: a device
        // that was offline past the tombstone TTL coming back with old files. Don't
        // resurrect it (the sender keeps its copy — nothing is lost).
        if std::fs::symlink_metadata(&dest_path).is_err() {
            if let (Some(ver), Some(deleted_at)) = (sender_ver, ctx.deleted_at.as_ref()) {
                let sig = format!("{rel_str}|{in_size}|{in_mtime}");
                if let Some(del) = deleted_at(&sig) {
                    if ver < del {
                        log::warn!(
                            "folder receive: not restoring {rel_str:?} — this exact version was deleted here \
                             after the sender's copy was made (stale copy from a long-offline device)"
                        );
                        let _ = std::fs::remove_file(&src);
                        report.outcomes.insert(rel_str, O::Kept("deleted".into()));
                        continue;
                    }
                }
            }
        }

        // D7: the name exists locally in DIFFERENT letter case (case-insensitive
        // volume — both spellings open the same file). Decide explicitly instead of
        // letting either spelling overwrite/archive the other in a loop.
        // Mirror folders only: a plain drop folder keeps its "name (1)" behaviour.
        if let (true, Some(ver), Some(actual)) =
            (mirror, sender_ver, if mirror { case_variant_on_disk(&dest_path) } else { None })
        {
            let actual_path = dest_path.with_file_name(&actual);
            let local_ver = std::fs::metadata(&actual_path)
                .map(|m| path_version_ms(&actual_path, &m))
                .unwrap_or(u64::MAX);
            let same = std::fs::metadata(&actual_path).map(|m| m.len()).ok() == Some(in_size)
                && content_hash(&src) == content_hash(&actual_path);
            let actual_rel = rel_path_of(&actual_path.to_string_lossy(), folder_str);
            if ver <= local_ver {
                // Ours is the newer spelling/version: keep it; tell the sender.
                let _ = std::fs::remove_file(&src);
                report.outcomes.insert(rel_str, O::Kept(if same { "case-same".into() } else { "case".into() }));
                continue;
            }
            // The sender's is newer (a case-only rename, or a renamed+edited file):
            // adopt it. Same bytes → just respell ours; different → archive ours.
            if let Some(ar) = &actual_rel {
                self_deleted.lock().unwrap().insert(ar.clone(), Instant::now());
            }
            if same {
                let _ = std::fs::remove_file(&src);
                if std::fs::rename(&actual_path, &dest_path).is_ok() {
                    if let Some(sig) = file_sig(&dest_path.to_string_lossy(), folder_str) {
                        inbound.lock().unwrap().insert(sig);
                    }
                    report.outcomes.insert(rel_str, O::Landed);
                } else {
                    report.outcomes.insert(rel_str, O::Failed);
                }
                continue;
            }
            let archived = actual_rel.as_deref().is_some_and(|ar| {
                crate::folder_history::archive(folder_str, &actual_path.to_string_lossy(), ar, "replaced")
            });
            if !archived {
                report.outcomes.insert(rel_str, O::Failed);
                continue;
            }
            // fall through: dest is free now; land normally below.
        }

        // RE-DELIVERY is a no-op in EVERY mode. If the sender never saw our "ok"
        // (ack lost to a path drop, or its stall watchdog fired) it re-sends a
        // file we already landed — without this check a plain folder grows a
        // visible "name (1)" duplicate that a two-way pair then syncs BACK, and
        // a 1:1 mirror pointlessly archives another multi-GB copy to history on
        // every redelivery. Identity is SIZE + content HASH — NOT mtime: a
        // re-received identical file often reads back with a 1-2s-skewed mtime
        // across filesystems, and gating on mtime here made every redelivery fall
        // through to the conflict branch and archive-then-re-land the file each
        // cycle (the user's "it never stays in my folder"). Same bytes = the file
        // we already have, whatever its mtime; record it and drop the staged copy.
        if dest_path.is_file() {
            let loc_size = std::fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0);
            if in_size == loc_size {
                let h = content_hash(&src);
                if !h.is_empty() && h == content_hash(&dest_path) {
                    let _ = std::fs::remove_file(&src);
                    if let Some(sig) = file_sig(&dest_path.to_string_lossy(), folder_str) {
                        inbound.lock().unwrap().insert(sig);
                    }
                    report.outcomes.insert(rel_str, O::Landed);
                    continue;
                }
            }
        }

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
                report.outcomes.insert(rel_str, O::Landed);
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
                report.outcomes.insert(rel_str, O::Kept("conflict".into()));
                continue;
            }
            // Incoming wins → archive the local copy, then replace it below. If the
            // archive fails, NOTHING is replaced (the local copy must never be
            // removed without a recoverable copy) and the item reports Failed.
            self_deleted
                .lock()
                .unwrap()
                .insert(rel_str.clone(), Instant::now());
            if !crate::folder_history::archive(
                folder_str,
                &dest_path.to_string_lossy(),
                &rel_str,
                "replaced",
            ) && dest_path.exists()
            {
                report.outcomes.insert(rel_str, O::Failed);
                continue;
            }
        } else if mirror && dest_path.is_file() {
            // Classic 1:1 mirror — UNCHANGED: the incoming file always replaces the
            // local one (arrival-order wins), old copy archived to History. No
            // mtime/clock dependence, so no skew regression for shipped folders.
            self_deleted
                .lock()
                .unwrap()
                .insert(rel_str.clone(), Instant::now());
            if !crate::folder_history::archive(
                folder_str,
                &dest_path.to_string_lossy(),
                &rel_str,
                "replaced",
            ) && dest_path.exists()
            {
                report.outcomes.insert(rel_str, O::Failed);
                continue;
            }
        }

        // TYPE SWAP: a DIRECTORY occupies the exact path where this incoming FILE
        // must land (a peer replaced a folder with a same-named file). `rename`/
        // `copy` can't overwrite a directory, so without this the file is silently
        // dropped and reconcile re-queues it forever (permanent divergence). Archive
        // the directory's contents to History (nothing lost), loop-guard each removal
        // so our watcher doesn't echo it back, then clear it so the file can land.
        // Only an EMPTIED tree is removed — anything that couldn't be archived stays.
        if mirror && dest_path.is_dir() {
            for child in list_files_rec(&dest_path) {
                if let Some(crel) = rel_path_of(&child.to_string_lossy(), folder_str) {
                    self_deleted.lock().unwrap().insert(crel.clone(), Instant::now());
                    crate::folder_history::archive(
                        folder_str,
                        &child.to_string_lossy(),
                        &crel,
                        "replaced",
                    );
                }
            }
            self_deleted.lock().unwrap().insert(rel_str.clone(), Instant::now());
            prune_empty_tree(&dest_path);
            if dest_path.exists() {
                report.outcomes.insert(rel_str, O::Failed);
                continue;
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
            report.moved.push(dest_str);
            report.outcomes.insert(rel_str, O::Landed);
        } else {
            report.outcomes.insert(rel_str, O::Failed);
        }
    }
    report
}

/// A file's modified-time as whole seconds since the epoch (0 if unavailable).
fn meta_mtime(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A file's "version" timestamp in epoch MILLIS: the most recent of its content
/// modification (mtime), its inode-change time (ctime — bumped when the file is
/// MOVED or COPIED into a folder), and its birth time. THE data-loss fix: a file
/// freshly dropped into a synced folder keeps its old content mtime (a video shot
/// weeks ago), but its ctime/birthtime is "now". Comparing a delete tombstone
/// against this max lets a just-added file defend itself against a stale "deleted"
/// tombstone instead of being silently wiped the instant it lands.
fn meta_version_ms(meta: &std::fs::Metadata) -> u64 {
    let to_ms = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64)
    };
    let mtime = meta.modified().ok().and_then(to_ms).unwrap_or(0);
    let birth = meta.created().ok().and_then(to_ms).unwrap_or(0);
    #[cfg(unix)]
    let ctime = {
        use std::os::unix::fs::MetadataExt;
        let s = meta.ctime();
        if s > 0 {
            (s as u64).saturating_mul(1000) + (meta.ctime_nsec().max(0) as u64 / 1_000_000)
        } else {
            0
        }
    };
    #[cfg(not(unix))]
    let ctime = 0u64;
    mtime.max(birth).max(ctime)
}

/// How long after a file is created/moved into a synced folder it stays immune
/// from a sync-driven delete. A delete targeting a file this fresh is almost
/// always a race with the user dropping it in — a bogus tombstone arriving at the
/// same instant (the "dropped a folder and it deleted right away" bug). We refuse
/// it. A GENUINE delete (e.g. the user drags a folder in and right back out) still
/// lands once the file ages past this window — the tombstone persists and the
/// reconcile retries. The race this guards against is sub-second to a few seconds,
/// so a 2-minute window covers it with wide margin while keeping delete-convergence
/// snappy. (Per-file write-completion is handled separately by `wait_until_stable`
/// BEFORE a file is ever queued, so a slow multi-GB copy doesn't need a long window
/// here.) Earlier this was 10 minutes, which made a deliberate drag-in/drag-out take
/// up to 10 minutes to disappear on the peer — too slow to feel reliable.
const DELETE_GRACE_MS: u64 = 2 * 60 * 1000;

/// True when `v` (epoch ms) is within `grace_ms` of `now` in EITHER direction.
/// Bounded on the future side on purpose: a wildly future-stamped file (clock
/// skew, or copied from a fast-clock machine) must NOT be protected forever — past
/// the window it ages out and obeys deletes again.
fn within_grace(v: u64, now: u64, grace_ms: u64) -> bool {
    if v == 0 || grace_ms == 0 {
        return false;
    }
    if v >= now {
        v - now < grace_ms
    } else {
        now - v < grace_ms
    }
}

/// True if `path` exists and was placed/modified within `grace_ms` — i.e. it's a
/// freshly-added file that must NOT be wiped by an incoming delete. `grace_ms` is
/// a parameter (not the constant directly) so tests can disable the protection.
fn file_is_fresh(path: &Path, grace_ms: u64) -> bool {
    if grace_ms == 0 {
        return false;
    }
    if let Ok(meta) = std::fs::metadata(path) {
        return within_grace(path_version_ms(path, &meta), now_ms(), grace_ms);
    }
    false
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
    // Stream in bounded chunks — this runs on multi-GB files during conflict
    // resolution, and reading the whole file into RAM (the old way) could spike
    // gigabytes of memory on the receiving machine mid-sync.
    let Ok(f) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut reader = std::io::BufReader::with_capacity(1 << 20, f);
    let mut h = Sha256::new();
    if std::io::copy(&mut reader, &mut h).is_err() {
        return String::new();
    }
    hex::encode(h.finalize())
}

/// Compute a file's path relative to the folder (forward-slashed, so the same
/// key is used on both peers). Works even when the file is already gone.
/// Canonical form of a folder-relative path used as a sync KEY. Two peers must
/// produce the SAME string for the same file, or reconcile thinks each is
/// "missing" it forever. Two normalizations matter:
///   • forward slashes (Windows back-slashes → `/`), and
///   • Unicode NFC. macOS stores filenames decomposed (NFD: `e` + ´) while
///     Windows/Linux use composed (NFC: `é`). Without this, a subfolder named
///     `Résumé` keys differently on each side → the file re-sends forever. This
///     is the classic Syncthing normalization bug; NFC-on-compare fixes it.
pub(crate) fn norm_rel(rel: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    rel.replace('\\', "/").nfc().collect()
}

/// True when a peer-supplied (already `norm_rel`'d) path names something INSIDE
/// the shared folder: non-empty, relative, no `..`, and no Windows drive/UNC
/// prefix (`Path::join("C:/x")` or even `join("C:")` REPLACES the folder base on
/// Windows, so the old `starts_with('/')` check alone let a peer escape there).
fn rel_stays_inside(rel_norm: &str) -> bool {
    use std::path::Component;
    !rel_norm.is_empty()
        && !rel_norm.starts_with('/')
        && !rel_norm.split('/').any(|c| c == "..")
        && Path::new(rel_norm)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn rel_path_of(abs: &str, folder: &str) -> Option<String> {
    let p = Path::new(abs);
    let norm = |r: &Path| norm_rel(&r.to_string_lossy());
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

/// What a delete is allowed to remove (D4). `ts` = when the peer deleted it;
/// `known` = the exact version (size, mtime) the deleter last had, when it said;
/// `listed` = for a DIRECTORY delete, the rels the deleter itself enumerated —
/// `Some` means "remove the directory only once it's empty; never a child the
/// deleter didn't list" (its listed children arrive as their own entries).
pub(crate) struct DeleteGuard<'a> {
    pub ts: u64,
    pub known: Option<(u64, u64)>,
    pub listed: Option<&'a HashSet<String>>,
    /// The deleter speaks the versioned-delete protocol: it names the version of
    /// every FILE it deleted. An entry without one is a directory (or a name it
    /// never had a synced version of) and must never remove a file here.
    pub require_known: bool,
}

impl DeleteGuard<'static> {
    /// Unconditional (tests / explicit user actions only).
    #[cfg(test)]
    fn any() -> Self {
        DeleteGuard { ts: u64::MAX, known: None, listed: None, require_known: false }
    }
}

/// Apply a delete the peer propagated. `rel` may name a FILE or a DIRECTORY (when
/// the peer removed a whole folder). Every file removed is archived to history
/// first (so nothing is unrecoverable) and recorded in `self_deleted` (loop
/// guard) and `applied` (the rels we actually removed, for tombstoning). Returns
/// true if anything was removed. Refuses (keeps the file) when it was just
/// dropped in, when it's a different/newer version than the one deleted, or when
/// the name only matches a local file in different letter case.
fn apply_remote_delete(
    folder: &str,
    rel: &str,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
    applied: &mut Vec<String>,
    grace_ms: u64,
    guard: &DeleteGuard,
) -> bool {
    let rel_norm = norm_rel(rel);
    // Never let a peer reach outside the folder.
    if !rel_stays_inside(&rel_norm) {
        return false;
    }
    // Never act on a folder that isn't there (unplugged drive).
    if !folder_available(folder) {
        return false;
    }
    let dest = Path::new(folder).join(&rel_norm);
    // D7: on a case-insensitive volume "photo.jpg" also opens "Photo.jpg". A
    // delete of one name must never remove a file that exists under ANOTHER
    // spelling — that's a rename, not this delete.
    if case_variant_on_disk(&dest).is_some() {
        log::warn!("apply_remote_delete: {rel_norm:?} only matches a differently-capitalized local file — keeping it");
        return false;
    }
    let mut removed = false;
    if dest.is_file() {
        // DATA-LOSS GUARD: never wipe a file the user just dropped in. A delete
        // landing on a brand-new file is almost always a stale tombstone racing the
        // drop — refuse it. The tombstone persists, so a GENUINE delete still lands
        // once the file ages past the grace window.
        if file_is_fresh(&dest, grace_ms) {
            log::warn!(
                "apply_remote_delete: REFUSED delete of freshly-added file {rel_norm:?} \
                 (placed within {}m) — protecting just-dropped data",
                grace_ms / 60_000
            );
            return false;
        }
        // D4: only the version the peer deleted goes — a newer local edit wins.
        if guard.require_known && guard.known.is_none() {
            log::warn!("apply_remote_delete: kept {rel_norm:?} — the peer never had a version of this file");
            return false;
        }
        if !delete_allowed(&dest, guard.ts, guard.known) {
            log::warn!("apply_remote_delete: kept {rel_norm:?} — the local copy is a different/newer version than the one deleted");
            return false;
        }
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
        if guard.listed.is_some() {
            // The deleter enumerated the children it knew; each arrives as its own
            // (version-checked) entry. All that's left for the directory itself is
            // to go once it's EMPTY — anything still inside is something the
            // deleter never saw (or a newer edit), and stays.
            prune_empty_tree(&dest);
            if !dest.exists() {
                self_deleted.lock().unwrap().insert(rel_norm.clone(), Instant::now());
                applied.push(rel_norm.clone());
                removed = true;
            }
        } else {
            // A whole folder was removed on the peer (legacy entry with no child
            // list). Archive + remove every file inside that's OLDER than the
            // delete (recording each child rel), then prune the now-empty tree.
            // Skip any freshly-dropped or newer child — so re-adding a folder that
            // shares a name with a deleted one can't nuke the new contents.
            let mut kept = 0usize;
            for child_abs in list_files_rec(&dest) {
                if file_is_fresh(&child_abs, grace_ms) || !delete_allowed(&child_abs, guard.ts, None) {
                    kept += 1;
                    continue;
                }
                let Some(child_rel) = rel_path_of(&child_abs.to_string_lossy(), folder) else {
                    kept += 1;
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
                } else {
                    kept += 1;
                }
            }
            if kept > 0 {
                log::warn!(
                    "apply_remote_delete: kept {kept} file(s) under {rel_norm:?} \
                     instead of deleting — new or newer than the delete"
                );
            }
            // Only remove the directory once NOTHING survives inside it — and only
            // ever an empty tree (remove_dir, never remove_dir_all: a file that
            // appeared a moment ago must not be swept away unarchived).
            prune_empty_tree(&dest);
            if !dest.exists() {
                self_deleted
                    .lock()
                    .unwrap()
                    .insert(rel_norm.clone(), Instant::now());
                removed = true;
            }
        }
    }
    // Tidy up: drop any now-empty parent directories up to (but not including)
    // the shared-folder root, so a removed subtree doesn't leave hollow folders.
    prune_empty_dirs(folder, &rel_norm);
    removed
}

/// Remove every EMPTY directory under (and including) `dir`, deepest first. Never
/// removes a user file or a non-empty directory. OS litter (`.DS_Store`,
/// `Thumbs.db`, `._x`…) doesn't count as content — Finder drops a `.DS_Store` into
/// almost every folder it shows, and it must not keep a deleted folder alive.
fn prune_empty_tree(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut junk: Vec<PathBuf> = Vec::new();
    let mut other = false;
    for e in entries.flatten() {
        let ft = e.file_type();
        if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
            prune_empty_tree(&e.path());
            if e.path().exists() {
                other = true;
            }
        } else if ft.map(|t| t.is_file()).unwrap_or(false)
            && crate::iroh_net::os_junk_name(&e.file_name().to_string_lossy())
        {
            junk.push(e.path());
        } else {
            other = true;
        }
    }
    if other {
        return;
    }
    for j in junk {
        let _ = std::fs::remove_file(j);
    }
    let _ = std::fs::remove_dir(dir); // only succeeds when empty
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
            // iOS has no user-visible Trash (and the `trash` crate has no mobile
            // backend), so there the "move to trash" mode degrades to a plain
            // remove. Desktop behaviour is unchanged.
            #[cfg(desktop)]
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

/// A stable signature (relative path + size + mtime) used to remember files we've
/// already sent or received, so restarts don't re-send delivered files and a
/// two-way folder never beams a received file back.
fn file_sig(path: &str, folder: &str) -> Option<String> {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).ok()?;
    // Fast path: paths from our own walks (seed_existing, move_staged) are literally
    // rooted at the configured folder string, so the rel falls out of a plain strip —
    // no syscalls. Only when that fails (watcher events can arrive under a firmlink/
    // symlink alias like /System/Volumes/Data/… or /private/tmp/…) canonicalize both
    // sides so the alias and the configured folder path resolve to a matching
    // relative path. canonicalize is two full realpath walks (on Windows it opens the
    // file, waking the AV filter) — which the idle rescan used to pay for EVERY file
    // EVERY tick.
    let rel_path = match p.strip_prefix(folder) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let canon_p = std::fs::canonicalize(p).ok()?;
            let canon_folder = std::fs::canonicalize(folder).ok()?;
            canon_p.strip_prefix(&canon_folder).ok()?.to_path_buf()
        }
    };
    // NFC-normalize the rel like every other sync key, so the signatures stored
    // in `inbound` and the tombstone keys derived from them match the (NFC)
    // reconcile manifest — otherwise a delete inside a non-ASCII subfolder
    // (NFD on macOS) never propagates via the reconcile backstop.
    let rel = norm_rel(&rel_path.to_string_lossy());
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
        // write_atomic, NOT bare fs::write: this manifest is the inbound loop-guard —
        // a crash mid-write would truncate it, load_manifest would fall back to empty,
        // and seed_existing would re-queue the WHOLE folder for a pointless re-upload.
        let _ = crate::settings::write_atomic(&manifest_path(config_dir, pair_id), txt.as_bytes());
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
        // write_atomic, NOT bare fs::write: tombstones are the only guard stopping the
        // peer's reconcile from RESURRECTING deliberately deleted files. A crash
        // mid-write would truncate the file, load_tombstones falls back to empty, and
        // every prior delete comes back — the exact failure class this app has fought
        // hardest.
        let _ =
            crate::settings::write_atomic(&tombstones_path(config_dir, pair_id), txt.as_bytes());
    }
}

/// Snapshot the in-memory tombstone map and persist it ONCE. Callers batch: record a
/// whole burst via `note_tombstone` (memory-only), then persist once per scope. A
/// bulk delete / first-sync adoption used to rewrite the entire (MB-scale on real
/// machines) file once PER tombstone — O(N²) write amplification, gigabytes written.
fn persist_tombstones(config_dir: &Path, pair_id: &str, tomb: &Arc<Mutex<HashMap<String, u64>>>) {
    let snapshot = tomb.lock().unwrap().clone();
    save_tombstones(config_dir, pair_id, &snapshot);
}

/// Record `rel` as deleted at `ts` (keeping the NEWEST timestamp), in MEMORY only.
/// Returns whether anything changed; the caller persists once per batch via
/// `persist_tombstones` — never re-add a per-call save here (see above).
fn note_tombstone(tomb: &Arc<Mutex<HashMap<String, u64>>>, rel: &str, ts: u64) -> bool {
    // Clamp an absurd FUTURE timestamp down to ~now. A peer with a wildly-ahead
    // clock (or a malicious one) could otherwise stamp a tombstone "newer than
    // everything forever" and have reconcile delete files the user still wants.
    // Clamping DOWN is the safe direction — at worst a legit delete from a very
    // fast clock won't reconcile-propagate (the live delete path still does), it
    // never causes an extra deletion.
    let ts = ts.min(now_ms().saturating_add(24 * 3600 * 1000));
    let mut t = tomb.lock().unwrap();
    let e = t.entry(rel.to_string()).or_insert(0);
    if ts > *e {
        *e = ts;
        true
    } else {
        false
    }
}

// ── Moves / renames ─────────────────────────────────────────────────────────
// When a user MOVES or RENAMES a file that's already synced (e.g. drags it into a
// subfolder of the shared folder), DropBeam recognizes it's the SAME bytes (same
// inode) and records a move `from_rel → to_rel`. The move op rides the SAME control
// beacon as deletes and is applied BEFORE them: the peer renames its copy in place
// (no re-download), then the (still-sent) delete of `from` is a harmless no-op.
//
// SAFETY: the move is a pure OPTIMIZATION layered on the existing, reliable
// delete+reconcile machinery — we do NOT suppress the deletion of `from`. So if the
// move op is lost, mismatched, or the peer is on an older build that ignores it, the
// peer simply removes `from` and reconcile re-pushes `to` (correct, just a re-upload).
// The apply is content-VERIFIED (size+mtime) so a stale or mis-detected move can
// never relocate/remove the wrong file. Moves expire fast (the delete backstops
// reliability), which also means a week-old stale op can't resurface.
const MOVE_TTL_MS: u64 = 15 * 60 * 1000;

fn moves_path(config_dir: &Path, pair_id: &str) -> PathBuf {
    config_dir.join(format!("moves-{pair_id}.json"))
}

/// `from_rel → (to_rel, size, mtime, ts_ms)`. size+mtime identify the moved bytes
/// so the receiver only relocates if its `from` really is that file.
type MoveRec = (String, u64, u64, u64);

fn load_moves(config_dir: &Path, pair_id: &str) -> HashMap<String, MoveRec> {
    std::fs::read_to_string(moves_path(config_dir, pair_id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_moves(config_dir: &Path, pair_id: &str, map: &HashMap<String, MoveRec>) {
    let cutoff = now_ms().saturating_sub(MOVE_TTL_MS);
    let pruned: HashMap<&String, &MoveRec> =
        map.iter().filter(|(_, (_, _, _, ts))| *ts >= cutoff).collect();
    if let Ok(txt) = serde_json::to_string(&pruned) {
        // write_atomic for crash-consistency, matching manifest/tombstones.
        let _ = crate::settings::write_atomic(&moves_path(config_dir, pair_id), txt.as_bytes());
    }
}

/// Record that `from` was moved/renamed to `to` (carrying the moved file's
/// size+mtime so the peer can verify), in memory + on disk.
#[allow(clippy::too_many_arguments)]
fn note_move(
    config_dir: &Path,
    pair_id: &str,
    moves: &Arc<Mutex<HashMap<String, MoveRec>>>,
    from: &str,
    to: &str,
    size: u64,
    mtime: u64,
    ts: u64,
) {
    {
        let mut m = moves.lock().unwrap();
        // Collapse a chain A→from then from→to into A→to so the peer does one hop.
        let chained: Vec<String> = m
            .iter()
            .filter(|(_, (t, _, _, _))| t == &norm_rel(from))
            .map(|(k, _)| k.clone())
            .collect();
        for k in chained {
            m.insert(k, (norm_rel(to), size, mtime, ts));
        }
        m.insert(norm_rel(from), (norm_rel(to), size, mtime, ts));
    }
    let snapshot = moves.lock().unwrap().clone();
    save_moves(config_dir, pair_id, &snapshot);
}

/// Compare the prior inode index (inode → old rel) against the live manifest (inode →
/// new rel) and return the moves that happened between snapshots: an inode now sitting
/// at a DIFFERENT rel, same size, whose old rel is gone from the manifest, is a
/// same-volume move/rename. This is the de-raced backstop for [`handle_move_candidate`]
/// — the live add-branch can miss a move when the periodic index rebuild fires in its
/// window, but the reconcile round catches it here before clobbering the index. Inode 0
/// (e.g. Windows today) yields nothing; that direction needs a real file id.
fn detect_moves_from_index(
    idx: &HashMap<u64, (String, u64, u64)>,
    prev_lm: &HashMap<String, FileEntry>,
    lm: &HashMap<String, FileEntry>,
) -> Vec<(String, String, u64, u64)> {
    let mut out: Vec<(String, String, u64, u64)> = Vec::new();
    // (1) FILE-ID (inode) path — reliable for same-volume moves where the platform gives
    // a stable id (Unix). Require size AND mtime to match (a rename preserves both) — as
    // strict as the apply gate, so inode RECYCLING into a same-size file can't be mistaken
    // for a move. Windows has no file id here (inode 0) and falls through to (2).
    for (rel, e) in lm {
        if e.inode == 0 {
            continue;
        }
        if let Some((old_rel, old_size, old_mtime)) = idx.get(&e.inode) {
            if old_rel != rel
                && *old_size == e.size
                && *old_mtime == e.mtime
                && !lm.contains_key(old_rel)
            {
                out.push((old_rel.clone(), rel.clone(), e.size, e.mtime));
            }
        }
    }
    // (2) OS-AGNOSTIC fallback for files with NO file id (e.g. WINDOWS, inode 0): a path
    // that VANISHED since the prior manifest and a NEW path with IDENTICAL size+mtime is a
    // move/rename — so a Windows user reorganizing also relocates instead of re-uploading.
    // Fire ONLY when that (size,mtime) is UNIQUE among both the vanished and appeared sets:
    // an ambiguous batch (several same-size files) is left to the normal re-upload, never
    // mis-paired. apply_remote_move re-verifies + archives to History, so even a rare
    // coincidence is recoverable, never lost.
    let appeared: Vec<(String, u64, u64)> = lm
        .iter()
        .filter(|(rel, e)| e.inode == 0 && !prev_lm.contains_key(*rel))
        .map(|(rel, e)| (rel.clone(), e.size, e.mtime))
        .collect();
    let gone: Vec<(String, u64, u64)> = prev_lm
        .iter()
        .filter(|(rel, e)| e.inode == 0 && !lm.contains_key(*rel))
        .map(|(rel, e)| (rel.clone(), e.size, e.mtime))
        .collect();
    for (arel, asz, amt) in &appeared {
        let app_count = appeared.iter().filter(|(_, s, m)| s == asz && m == amt).count();
        let matches: Vec<&(String, u64, u64)> =
            gone.iter().filter(|(_, s, m)| s == asz && m == amt).collect();
        if app_count == 1 && matches.len() == 1 {
            let grel = &matches[0].0;
            if !out.iter().any(|(f, t, _, _)| f == grel || t == arel) {
                out.push((grel.clone(), arel.clone(), *asz, *amt));
            }
        }
    }
    out
}

/// Apply a peer's MOVE: they renamed `from_rel`→`to_rel` inside the shared folder,
/// so relocate OUR copy instead of re-receiving the bytes. CONTENT-VERIFIED: only
/// acts if our `from` actually matches the moved file's `exp_size`+`exp_mtime`, so a
/// stale or mis-detected move can never touch the wrong file. Freshness-guarded;
/// archives anything removed to History (recoverable). Applied BEFORE the peer's
/// delete of `from`, so the common case never re-downloads.
///   • from matches, to missing → rename from→to (clean move; never clobbers).
///   • from matches, to matches → `to` already has the bytes (reconcile raced
///     ahead) → archive + remove the duplicate `from`.
///   • anything else            → no-op (the delete + reconcile backstop converge).
/// Returns true if it changed the filesystem.
#[allow(clippy::too_many_arguments)]
fn apply_remote_move(
    folder: &str,
    from_rel: &str,
    to_rel: &str,
    exp_size: u64,
    exp_mtime: u64,
    self_deleted: &Arc<Mutex<HashMap<String, Instant>>>,
    inbound: &Arc<Mutex<HashSet<String>>>,
    grace_ms: u64,
) -> bool {
    let from = norm_rel(from_rel);
    let to = norm_rel(to_rel);
    if !rel_stays_inside(&from) || !rel_stays_inside(&to) || from == to {
        return false;
    }
    let from_abs = Path::new(folder).join(&from);
    let to_abs = Path::new(folder).join(&to);
    // D7: a CASE-ONLY rename ("photo.jpg" → "Photo.jpg"). On a case-insensitive
    // volume `from` and `to` are the SAME file, so the "to already has the bytes →
    // archive the duplicate from" branch below would delete the only copy. Rename
    // in place instead (when our copy really is that file under the old spelling).
    if from.to_lowercase() == to.to_lowercase() {
        let Ok(fmeta) = std::fs::metadata(&from_abs) else { return false };
        if !fmeta.is_file() || fmeta.len() != exp_size || meta_mtime(&fmeta) != exp_mtime {
            return false;
        }
        if case_variant_on_disk(&from_abs).is_some() {
            return false; // our copy isn't spelled like the peer's old name
        }
        // `to` exactly spelled already exists → it's a separate file (a
        // case-sensitive volume) or the rename already happened: never overwrite.
        if std::fs::symlink_metadata(&to_abs).is_ok() && case_variant_on_disk(&to_abs).is_none() {
            return false;
        }
        self_deleted.lock().unwrap().insert(from.clone(), Instant::now());
        if std::fs::rename(&from_abs, &to_abs).is_err() {
            return false;
        }
        if let Some(sig) = file_sig(&to_abs.to_string_lossy(), folder) {
            inbound.lock().unwrap().insert(sig);
        }
        return true;
    }
    // Our `from` must BE the moved file (same size + mtime). A different file that
    // merely shares the old path (e.g. a coincidental new file, or an inode-recycle
    // mis-detection on the sender) won't match → we leave it untouched and let the
    // normal delete + reconcile handle convergence.
    let Ok(fmeta) = std::fs::metadata(&from_abs) else {
        return false; // from gone → already moved / never had it
    };
    if !fmeta.is_file() || fmeta.len() != exp_size || meta_mtime(&fmeta) != exp_mtime {
        return false;
    }
    // Never disturb a file the user may be actively editing right now.
    if file_is_fresh(&from_abs, grace_ms) {
        return false;
    }
    if let Ok(tmeta) = std::fs::metadata(&to_abs) {
        // `to` already exists. Only treat `from` as a removable duplicate if `to`
        // is genuinely the SAME bytes; otherwise leave both alone (don't clobber a
        // real, different file that happens to sit at `to`).
        if !tmeta.is_file() || tmeta.len() != exp_size || meta_mtime(&tmeta) != exp_mtime {
            return false;
        }
        // …and is a DIFFERENT file than `from` (never archive the only copy
        // because two spellings resolve to one inode).
        if meta_inode(&tmeta) != 0 && meta_inode(&tmeta) == meta_inode(&fmeta) {
            return false;
        }
        if file_is_fresh(&to_abs, grace_ms) {
            return false;
        }
        self_deleted.lock().unwrap().insert(from.clone(), Instant::now());
        let removed =
            crate::folder_history::archive(folder, &from_abs.to_string_lossy(), &from, "moved");
        if removed {
            prune_empty_dirs(folder, &from);
        }
        return removed;
    }
    // Clean rename. Loop-guard BEFORE touching the fs so OUR OWN watcher (which
    // will see a Remove of `from` + a Create of `to`) doesn't echo the move back:
    //  • mark `from` self-deleted (our delete branch ignores it),
    //  • pre-register `to`'s post-rename signature in `inbound` (add branch skips it).
    if let Some(parent) = to_abs.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    self_deleted.lock().unwrap().insert(from.clone(), Instant::now());
    if std::fs::rename(&from_abs, &to_abs).is_err() {
        return false;
    }
    if let Some(sig) = file_sig(&to_abs.to_string_lossy(), folder) {
        inbound.lock().unwrap().insert(sig);
    }
    prune_empty_dirs(folder, &from);
    true
}

/// Decide whether a file that just APPEARED at `p` is actually a MOVE/RENAME of a
/// file we already had (its inode previously lived at a different path that's now
/// gone). On a hit: record the move (with the file's size+mtime so the peer can
/// verify), keep the inode index current, kick the control sender, and return true
/// (the caller skips the byte-send — the move op + the existing delete/reconcile
/// convey the relocation). Inode + size match = same file, so no false positives
/// for a same-volume move; a genuinely-new file or a copy never matches. We do NOT
/// suppress the deletion of the old path — it stays as the reliable backstop.
fn handle_move_candidate(
    p: &str,
    folder: &str,
    config_dir: &Path,
    pair_id: &str,
    moves: &Arc<Mutex<HashMap<String, MoveRec>>>,
    ino_index: &Arc<Mutex<HashMap<u64, (String, u64, u64)>>>,
    control_wake: &Arc<Notify>,
) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    let inode = meta_inode(&meta);
    let size = meta.len();
    let mtime = meta_mtime(&meta);
    let Some(new_rel) = rel_path_of(p, folder) else {
        return false;
    };
    // The reconcile round may have ALREADY recorded a move TO this path (it catches
    // relocations the live index raced past). If THIS file is that moved file (its
    // size+mtime match the recorded move), don't also queue a byte-send — the peer is
    // already being told to relocate its copy, so re-uploading would waste bandwidth
    // and risk a duplicate. TWO guards make this safe: (a) size+mtime must match, so a
    // genuinely new DIFFERENT file at the same path is still sent; (b) we ONLY trust the
    // skip when we have a reliable file id (inode != 0) — i.e. the move was inode-detected.
    // We must NEVER suppress a byte-send on the strength of the weaker size+mtime-only
    // (no-file-id / Windows) move heuristic: a rare coincidence there would otherwise
    // leave the peer with stale content that never re-syncs (same size+mtime ⇒ reconcile
    // sees them as identical). On a no-id platform the bytes always flow and guarantee
    // correctness; the move op is then just a harmless placement hint.
    if inode != 0
        && !new_rel.is_empty()
        && moves
            .lock()
            .unwrap()
            .values()
            .any(|(to, sz, mt, _)| *to == new_rel && *sz == size && *mt == mtime)
    {
        return true;
    }
    if inode == 0 || new_rel.is_empty() {
        return false;
    }
    let prev = ino_index.lock().unwrap().get(&inode).cloned();
    if let Some((old_rel, old_size, old_mtime)) = prev {
        // Same inode now at a DIFFERENT rel, same size AND mtime (a rename preserves
        // both), old path gone = a same-volume move. The mtime check matches the
        // apply-side gate so inode recycling into a same-size file can't false-trigger.
        if old_rel != new_rel
            && old_size == size
            && old_mtime == mtime
            && !Path::new(folder).join(&old_rel).exists()
        {
            note_move(config_dir, pair_id, moves, &old_rel, &new_rel, size, mtime, now_ms());
            ino_index.lock().unwrap().insert(inode, (new_rel.clone(), size, mtime));
            control_wake.notify_one();
            log::info!("folder move detected: {old_rel:?} → {new_rel:?} (relocating, no re-upload)");
            return true;
        }
    }
    // A genuine add/change — keep the inode index warm for future move detection.
    ino_index.lock().unwrap().insert(inode, (new_rel, size, mtime));
    false
}

/// Every relative file path currently present under `folder`, mapped to its
/// signature (`size|mtime`). The authoritative "what I actually have on disk".
async fn live_manifest_async(folder: String) -> HashMap<String, FileEntry> {
    tokio::task::spawn_blocking(move || live_manifest(&folder))
        .await.expect("manifest scan panicked")
}

fn live_manifest(folder: &str) -> HashMap<String, FileEntry> {
    let _walk = crate::fs_walk::Watch::new("live_manifest", Path::new(folder));
    let mut out = HashMap::new();
    for abs in list_files_rec(Path::new(folder)) {
        let p = abs.to_string_lossy().to_string();
        if p.ends_with(".dropbeam-incoming") {
            continue;
        }
        let Some(rel) = rel_path_of(&p, folder) else {
            continue;
        };
        // Skip dot-paths AND DropBeam's own control paths — including the VISIBLE
        // `dropbeam-history` leaked by the old receive de-dot bug, which the dot
        // check alone misses, letting it self-sync forever.
        if rel.is_empty()
            || rel.split('/').any(|c| c.starts_with('.'))
            || crate::iroh_net::is_control_rel(&rel)
        {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&abs) {
            out.insert(
                rel,
                FileEntry {
                    size: meta.len(),
                    mtime: meta_mtime(&meta),
                    version_ms: path_version_ms(&abs, &meta),
                    inode: meta_inode(&meta),
                },
            );
        }
    }
    out
}

#[derive(Clone, Copy, Default)]
struct FileEntry {
    size: u64,
    /// Content modification time, epoch SECONDS (the conflict tie-breaker).
    mtime: u64,
    /// Placement "version", epoch MILLIS: max(mtime, ctime, birthtime). What a
    /// delete tombstone is compared against, so a freshly-dropped file (old mtime,
    /// brand-new ctime) isn't wiped on arrival. See [`meta_version_ms`].
    version_ms: u64,
    /// Filesystem inode (Unix) / file index (Windows). 0 when unavailable. A `mv`
    /// within the SAME volume preserves this, so it's how move detection recognizes
    /// that a file which "appeared" at a new path is the SAME bytes that "vanished"
    /// from an old one — letting us send a rename instruction instead of re-uploading.
    inode: u64,
}

/// A file's inode (Unix) / file index (Windows) — a stable identity within one
/// volume that survives a move/rename. 0 when the platform can't provide it (then
/// move detection simply doesn't trigger and we fall back to re-send + delete).
fn meta_inode(meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    }
    // Windows: the stable std API has no file index (`file_index()` is behind the
    // unstable `windows_by_handle` feature), so we return 0 → move DETECTION is a
    // no-op on Windows and it falls back to the normal re-send + delete. (Windows
    // can still RECEIVE a move from another OS — apply matches by size+mtime, not
    // inode.) Getting it would mean GetFileInformationByHandle; not worth it here.
    #[cfg(not(unix))]
    {
        let _ = meta;
        0
    }
}

/// Relative paths of directories under `folder` that contain NO files anywhere
/// beneath them (a dir whose whole subtree is file-less). Skips dot-dirs and
/// symlinks. Used so empty organizing-folders still sync.
fn live_empty_dirs(folder: &str) -> Vec<String> {
    let _walk = crate::fs_walk::Watch::new("live_empty_dirs", Path::new(folder));
    let mut dirs: Vec<String> = Vec::new();
    // For every file, every ANCESTOR directory rel is "has files".
    let mut has_files: HashSet<String> = HashSet::new();
    let mut stack = vec![Path::new(folder).to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = crate::fs_walk::read_dir(&d) else {
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

/// The receive half of the [`live_empty_dirs`] round trip: create every file-less
/// directory the peer reported that we don't have yet, NESTED ones included (the
/// peer sends `a` and `a/b` as separate rels, and `create_dir_all` handles either
/// order). Purely additive — it never removes anything.
///
/// Skips a rel tombstoned on either side, checking every ANCESTOR too: creating
/// `a/b` would otherwise silently resurrect a deleted `a`.
fn apply_empty_dirs(
    folder: &str,
    empty_dirs: &[String],
    my_tomb: &HashMap<String, u64>,
    peer_tomb: &HashMap<String, u64>,
) {
    for rel in empty_dirs {
        let norm = norm_rel(rel);
        if norm.is_empty()
            || norm.starts_with('/')
            || norm.split('/').any(|c| c == ".." || c.starts_with('.'))
        {
            continue;
        }
        let mut prefix = String::new();
        let deleted = norm.split('/').any(|c| {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(c);
            my_tomb.contains_key(&prefix) || peer_tomb.contains_key(&prefix)
        });
        if deleted {
            continue; // deleted somewhere — don't recreate
        }
        let abs = Path::new(folder).join(&norm);
        if !abs.exists() {
            let _ = std::fs::create_dir_all(&abs);
        }
    }
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
    now: u64,
) -> ReconcilePlan {
    reconcile_plan_ci(mine, peer_files, peer_tombs, my_tombs, now, false)
}

/// `reconcile_plan` for a folder on a case-INSENSITIVE volume (`ci`): a peer file
/// whose name differs from ours only in letter case IS our file (both spellings
/// open the same bytes here) — never "missing", so a case-only rename can't
/// ping-pong between two Macs forever (D7). Deletes stay exact-name only.
fn reconcile_plan_ci(
    mine: &HashMap<String, FileEntry>,
    peer_files: &HashMap<String, (u64, u64)>,
    peer_tombs: &HashMap<String, u64>,
    my_tombs: &HashMap<String, u64>,
    now: u64,
    ci: bool,
) -> ReconcilePlan {
    // Compare on NFC-normalized keys so a subfolder name that macOS stored
    // decomposed (NFD) and the peer stored composed (NFC) is recognized as the
    // SAME file instead of "missing" → re-sent forever.
    let peer_files: HashMap<String, (u64, u64)> =
        peer_files.iter().map(|(k, v)| (norm_rel(k), *v)).collect();
    // Case-folded index of the peer's files: lower → (count, entry).
    let peer_folded: HashMap<String, (usize, (u64, u64))> = if ci {
        let mut m: HashMap<String, (usize, (u64, u64))> = HashMap::new();
        for (k, v) in &peer_files {
            let e = m.entry(k.to_lowercase()).or_insert((0, *v));
            e.0 += 1;
            e.1 = *v;
        }
        m
    } else {
        HashMap::new()
    };
    let peer_tombs: HashMap<String, u64> =
        peer_tombs.iter().map(|(k, v)| (norm_rel(k), *v)).collect();
    let my_tombs: HashMap<String, u64> =
        my_tombs.iter().map(|(k, v)| (norm_rel(k), *v)).collect();

    let mut plan = ReconcilePlan::default();
    for (rel0, entry) in mine {
        let rel = norm_rel(rel0);
        // Compare a tombstone against the file's PLACEMENT version (max of mtime,
        // ctime, birthtime), not just content mtime — so a re-dropped file with an
        // old mtime but a brand-new ctime defends itself. Fall back to mtime when
        // version_ms is unset (e.g. unit tests).
        let file_ms = entry
            .version_ms
            .max(entry.mtime.saturating_mul(1000));
        let peer_t = peer_tombs.get(&rel).copied().unwrap_or(0);
        let mine_t = my_tombs.get(&rel).copied().unwrap_or(0);
        // A file placed/edited within the grace window is OFF-LIMITS: it's almost
        // certainly something the user is dropping in right now, racing a stale
        // "deleted" tombstone. SKIP it entirely — neither delete (don't wipe it) nor
        // push (the live watcher already sends genuine drops; re-pushing here would
        // resurrect a file legitimately deleted right after a sync). Once it ages
        // past the window, normal rules resume and it converges.
        let fresh = within_grace(file_ms, now, DELETE_GRACE_MS);
        if fresh {
            continue;
        }
        // A tombstone newer than our file means it should be gone.
        if peer_t > file_ms {
            plan.delete.push(rel);
            continue;
        }
        if mine_t > file_ms {
            // We already know it's deleted locally-pending; don't push it.
            continue;
        }
        // Convergence rule: push ONLY when the peer is missing the file, or has a
        // genuinely different version (different SIZE) that's older than ours.
        // NEVER re-push on an mtime-only difference — two machines round file
        // mtimes differently (APFS nanoseconds vs FAT/NTFS 2-second buckets, plus
        // clock skew), so an identical file reads back a second or two apart and
        // would re-send forever. mtime stays a tie-breaker for genuine conflicts
        // (in move_staged_into_folder), never the in-sync test. This is what makes
        // the sync actually converge instead of ping-ponging the same file.
        let peer_entry = peer_files.get(&rel).copied().or_else(|| {
            peer_folded
                .get(&rel.to_lowercase())
                .filter(|(n, _)| *n == 1)
                .map(|(_, v)| *v)
        });
        let need = match peer_entry {
            None => true,
            Some((psize, pmtime)) => psize != entry.size && entry.mtime > pmtime,
        };
        if need {
            plan.push.push(rel);
        }
    }
    plan.delete.sort();
    plan.push.sort();
    plan.delete.dedup();
    plan.push.dedup();
    plan
}

/// Compute the honest "are these two folders identical?" answer that drives the
/// Verify button, from OUR live manifest, the PEER's snapshot (`rec`), and our own
/// tombstones. Pure (no I/O) so it can be exhaustively unit-tested. It reuses
/// `reconcile_plan` for both directions — exactly the size-only signature rule the
/// background self-heal already converges — so the difference count it reports IS
/// what is being fixed:
///   • `missing_on_peer` = `reconcile_plan(ours → peer).push` (files we'll send).
///   • `missing_locally` = `reconcile_plan(peer → ours).push` (files we'll receive),
///     computed by running the SAME planner with the sides swapped.
///   • `pending_deletes`  = local files the peer tombstoned newer than ours (we'll
///     delete) + peer files WE tombstoned newer than theirs (they'll delete).
///   • `matched`          = paths present on both with the same byte size.
/// `identical` is true iff every one of those difference buckets is empty.
fn compute_verify(
    mine: &HashMap<String, FileEntry>,
    rec: &Reconcile,
    my_tombs: &HashMap<String, u64>,
    now: u64,
) -> VerifyResult {
    // Direction 1: what WE would push/delete given the peer's snapshot.
    let forward = reconcile_plan(mine, &rec.files, &rec.tombstones, my_tombs, now);

    // Direction 2: what the PEER would push given OUR snapshot — i.e. files the peer
    // has that we're missing or have a stale-size copy of. Model our side as a
    // FileEntry map (size + mtime; ctime/inode irrelevant for this comparison) and
    // run the very same planner with the roles swapped. We pass empty tombstones
    // here because `forward` already accounts for every delete in BOTH directions
    // (peer→us via `forward.delete`, us→peer via the my_tombs scan below); folding
    // them in again would double-count.
    let mine_as_peer: HashMap<String, (u64, u64)> =
        mine.iter().map(|(k, e)| (k.clone(), (e.size, e.mtime))).collect();
    // Pre-normalize our tombstones so a peer file WE deleted (tombstone newer than
    // the peer's copy) is excluded from "to receive" — it's a pending DELETE we'll
    // push, counted separately below, not a file we'll pull back down.
    let my_tombs_norm: HashMap<String, u64> =
        my_tombs.iter().map(|(k, v)| (norm_rel(k), *v)).collect();
    let peer_as_mine: HashMap<String, FileEntry> = rec
        .files
        .iter()
        .filter(|(k, &(_, mtime))| {
            my_tombs_norm.get(&norm_rel(k)).copied().unwrap_or(0) <= mtime.saturating_mul(1000)
        })
        .map(|(k, &(size, mtime))| {
            (
                k.clone(),
                FileEntry {
                    size,
                    mtime,
                    version_ms: mtime.saturating_mul(1000),
                    inode: 0,
                },
            )
        })
        .collect();
    let no_tombs: HashMap<String, u64> = HashMap::new();
    let reverse = reconcile_plan(&peer_as_mine, &mine_as_peer, &no_tombs, &no_tombs, now);

    let missing_on_peer = forward.push.len() as u32;
    let missing_locally = reverse.push.len() as u32;

    // Deletes the peer told us about that still apply to a local file (forward.delete),
    // plus deletes WE hold for a file the peer still has (we'll push our tombstone).
    let mut pending_deletes = forward.delete.len() as u32;
    let peer_norm: HashMap<String, (u64, u64)> =
        rec.files.iter().map(|(k, v)| (norm_rel(k), *v)).collect();
    let peer_tomb_norm: HashMap<String, u64> =
        rec.tombstones.iter().map(|(k, v)| (norm_rel(k), *v)).collect();
    for (rel0, &ts) in my_tombs {
        let rel = norm_rel(rel0);
        if let Some(&(_psize, pmtime)) = peer_norm.get(&rel) {
            // We deleted it; the peer still has a copy whose placement predates our
            // tombstone, and the peer hasn't already tombstoned it itself.
            let peer_already = peer_tomb_norm.get(&rel).copied().unwrap_or(0) >= ts;
            if ts > pmtime.saturating_mul(1000) && !peer_already {
                pending_deletes += 1;
            }
        }
    }

    // Matched = same path on both sides with the same byte size (mtime ignored, per
    // the reconcile's own in-sync rule). NFC-normalize both sides so a name macOS
    // stored decomposed and the peer stored composed counts as the same file.
    let mine_norm: HashMap<String, u64> =
        mine.iter().map(|(k, e)| (norm_rel(k), e.size)).collect();
    let mut matched = 0u32;
    for (rel, size) in &mine_norm {
        if peer_norm.get(rel).map(|&(psize, _)| psize == *size).unwrap_or(false) {
            matched += 1;
        }
    }

    // A path on BOTH sides whose byte SIZE differs is a real difference (different
    // content) — but the mtime-based planner misses it when the two copies happen
    // to share an mtime (equal mtime → no push in either direction), which would
    // make `identical` wrongly true. Count those explicitly, excluding any path the
    // planners already flagged as a push (avoid double-counting). This guarantees
    // "identical" can never be reported while a same-path size mismatch exists.
    let fwd_push: std::collections::HashSet<String> =
        forward.push.iter().map(|p| norm_rel(p)).collect();
    let rev_push: std::collections::HashSet<String> =
        reverse.push.iter().map(|p| norm_rel(p)).collect();
    let mut size_mismatch = 0u32;
    for (rel, size) in &mine_norm {
        if let Some(&(psize, _)) = peer_norm.get(rel) {
            if psize != *size && !fwd_push.contains(rel) && !rev_push.contains(rel) {
                size_mismatch += 1;
            }
        }
    }

    let differences = missing_on_peer + missing_locally + pending_deletes + size_mismatch;
    VerifyResult {
        peer_online: true,
        compared: true,
        identical: differences == 0,
        matched,
        differences,
        missing_on_peer,
        missing_locally,
        pending_deletes,
        local_files: mine.len() as u32,
        peer_files: rec.files.len() as u32,
    }
}

fn friend_sig(f: &Friend) -> String {
    // auto_accept is included so flipping it restarts the listener in the new mode.
    format!("{}|{:?}|{}|{}", f.id, f.role, f.secret, f.auto_accept)
}

/// Folders whose filesystem-watch registration has already been logged as failing.
/// `start_pair` re-runs whenever a pair's structural signature changes, and a folder
/// that is missing fails the same way every time — so without this the identical
/// "watch failed for …" line piles up in the log and the diagnostics digest.
fn watch_failures() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Returns true the FIRST time this folder's watch fails (i.e. "log it"), false for
/// every repeat until [`clear_watch_failure`] forgets it.
fn note_watch_failure(folder: &str) -> bool {
    watch_failures()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(folder.to_string())
}

/// The watch succeeded — forget any earlier failure so a folder that goes missing
/// again later is logged again rather than silently swallowed.
fn clear_watch_failure(folder: &str) {
    watch_failures()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(folder);
}

fn structural_sig(p: &Pair) -> String {
    // Roles are part of the structural signature so a role change restarts the
    // pair's workers — the new direction takes effect AND the control sender
    // re-beacons the roster immediately (instead of waiting out its idle cadence).
    format!(
        "{}|{:?}|{}|{}|{}|{}|{}",
        p.folder, p.role, p.secret, p.two_way, p.mirror, p.i_am_viewer, p.peer_is_viewer
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
) -> (String, Vec<(String, String, bool)>, Option<String>, u64) {
    let Some(gid) = pair.group_id.clone() else {
        return (String::new(), Vec::new(), None, 0);
    };
    let members = pairing::members_of_group(config_dir, &gid);
    // My own role in the group (all my links share it). The roster carries each
    // member's `is_viewer` so the whole mesh converges on the owner's assignment.
    let am_i_viewer = members.iter().any(|p| p.i_am_viewer);
    let mut roster: Vec<(String, String, bool)> =
        vec![(ep.id().to_string(), my_name.to_string(), am_i_viewer)];
    for p in &members {
        if let Some(eid) = &p.endpoint_id {
            let n = if p.peer_name.trim().is_empty() {
                "Member".to_string()
            } else {
                p.peer_name.clone()
            };
            roster.push((eid.clone(), n, p.peer_is_viewer));
        }
    }
    // The owner + role epoch: who's authoritative for roles and which version we're
    // on. We relay the owner's value verbatim so an offline owner's last assignment
    // still reaches the whole mesh.
    let (owner_eid, role_epoch) = pairing::group_role_authority(config_dir, &gid);
    (gid, roster, owner_eid, role_epoch)
}

// ── Folder availability (D6) ─────────────────────────────────────────────────
/// Is the synced folder really there? Not just "a directory exists at that path":
/// on Linux an unmounted drive leaves its EMPTY mount-point directory behind, which
/// would read as "every file deleted". We remember folders that are their own
/// mount (different device from their parent) and treat them as missing whenever
/// they're back on the parent's device.
pub(crate) fn folder_available(folder: &str) -> bool {
    let root = Path::new(folder);
    if !root.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        static MOUNTS: OnceLock<Mutex<Option<HashSet<String>>>> = OnceLock::new();
        let file = sync_config_dir().get().map(|d| d.join("folder-mounts.json"));
        let (Ok(meta), Some(parent)) = (std::fs::metadata(root), root.parent()) else { return true };
        let Ok(pmeta) = std::fs::metadata(parent) else { return true };
        let mut guard = MOUNTS.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
        let set = guard.get_or_insert_with(|| {
            file.as_ref()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        });
        if meta.dev() != pmeta.dev() {
            if set.insert(folder.to_string()) {
                if let (Some(f), Ok(txt)) = (file, serde_json::to_string(&*set)) {
                    let _ = crate::settings::write_atomic(&f, txt.as_bytes());
                }
            }
        } else if set.contains(folder) {
            return false; // was its own mount, now the bare mount point
        }
    }
    true
}

// ── Send queue (T14) ─────────────────────────────────────────────────────────
/// The per-folder send queue: FIFO order + a set for O(1) "already queued?" checks.
/// A 3,000-photo drop used to pay an O(n) `iter().any` scan per enqueue under a
/// std Mutex — O(n²) with the async runtime blocked meanwhile.
#[derive(Default)]
pub(crate) struct SendQueue {
    q: VecDeque<String>,
    set: HashSet<String>,
    /// Consecutive per-item failures the PEER reported (or changed-while-sending).
    fails: HashMap<String, u32>,
    /// Items parked after repeated failures (a file the peer can't land — e.g. a
    /// folder in the way holding hidden files, or an unreadable History index on
    /// their side): not re-queued by the rescan/reconcile until this time.
    cooldown: HashMap<String, Instant>,
}

impl SendQueue {
    /// Append unless already queued (or parked). Returns whether it was added.
    fn push_back(&mut self, p: String) -> bool {
        if let Some(until) = self.cooldown.get(&p) {
            if Instant::now() < *until {
                return false;
            }
            self.cooldown.remove(&p);
        }
        if self.set.insert(p.clone()) {
            self.q.push_back(p);
            true
        } else {
            false
        }
    }
    fn front(&self) -> Option<&String> {
        self.q.front()
    }
    fn len(&self) -> usize {
        self.q.len()
    }
    fn iter(&self) -> std::collections::vec_deque::Iter<'_, String> {
        self.q.iter()
    }
    fn remove(&mut self, p: &str) -> bool {
        if !self.set.remove(p) {
            return false;
        }
        if self.q.front().map(|f| f == p).unwrap_or(false) {
            self.q.pop_front();
        } else if let Some(pos) = self.q.iter().position(|x| x == p) {
            self.q.remove(pos);
        }
        true
    }
    /// The peer couldn't land this item. After 3 in a row it's parked with an
    /// exponential cool-down (2 min doubling, max 1 h) instead of re-sending its
    /// bytes every round forever. Returns whether it was parked.
    fn note_failed(&mut self, p: &str) -> bool {
        let n = {
            let e = self.fails.entry(p.to_string()).or_insert(0);
            *e += 1;
            *e
        };
        if n < 3 {
            return false;
        }
        let mins = (2u64 << (n - 3).min(5)).min(60);
        self.cooldown.insert(p.to_string(), Instant::now() + Duration::from_secs(mins * 60));
        self.remove(p);
        log::warn!("folder send: peer keeps failing to save {} — retrying in {mins} min", file_name_of(p));
        true
    }
    fn note_ok(&mut self, p: &str) {
        self.fails.remove(p);
        self.cooldown.remove(p);
    }
    /// Move an item to the back (a file that keeps failing must not block the rest).
    fn rotate_to_back(&mut self, p: &str) {
        if let Some(pos) = self.q.iter().position(|x| x == p) {
            if let Some(item) = self.q.remove(pos) {
                self.q.push_back(item);
            }
        }
    }
}

// ── Active receive placeholders ──────────────────────────────────────────────
/// "<name>.dropbeam-incoming" placeholders owned by a receive that is still
/// running. The litter sweeps skip these — they used to delete a live receive's
/// placeholder on every idle rescan.
fn active_placeholders() -> &'static Mutex<HashSet<PathBuf>> {
    static SET: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn placeholder_active(path: &Path, on: bool) {
    let mut set = active_placeholders().lock().unwrap_or_else(|e| e.into_inner());
    if on {
        set.insert(path.to_path_buf());
    } else {
        set.remove(path);
    }
}

fn is_placeholder_active(path: &Path) -> bool {
    active_placeholders()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(path)
}

/// Remove a crash-orphaned placeholder only when no live receive owns it and it's
/// stale (> 10 min since it was created).
fn sweep_stale_placeholder(f: &Path) {
    if is_placeholder_active(f) {
        return;
    }
    let stale = std::fs::metadata(f)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs() > 600)
        .unwrap_or(true);
    if stale {
        let _ = std::fs::remove_file(f);
    }
}

// ── Placement version (D15) ──────────────────────────────────────────────────
/// When a path genuinely APPEARED in a synced folder (non-Unix only). Windows has
/// no ctime in std and a same-volume MOVE keeps a file's old creation time, so an
/// old file moved INTO a synced folder read as "old" and a stale tombstone deleted
/// it. Unix gets this from ctime. Elsewhere the watcher records the moment it saw
/// a path appear (never a startup scan — that would make every file look "placed
/// at launch" and block real deletes), persisted so it survives a restart.
const FIRST_SEEN_TTL_MS: u64 = 45 * 24 * 3600 * 1000;
const FIRST_SEEN_CAP: usize = 100_000;

fn sync_config_dir() -> &'static OnceLock<PathBuf> {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    &DIR
}

fn first_seen_map() -> &'static Mutex<Option<HashMap<String, (u64, u64, u64)>>> {
    static MAP: OnceLock<Mutex<Option<HashMap<String, (u64, u64, u64)>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(None))
}

fn first_seen_file() -> Option<PathBuf> {
    sync_config_dir().get().map(|d| d.join("first-seen.json"))
}

fn with_first_seen<R>(f: impl FnOnce(&mut HashMap<String, (u64, u64, u64)>) -> R) -> R {
    let mut guard = first_seen_map().lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| {
        first_seen_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// Record that `path` (size, mtime) just appeared. Persisted.
fn note_appeared(path: &Path, size: u64, mtime: u64, now: u64) {
    let key = path.to_string_lossy().to_string();
    let snapshot = with_first_seen(|m| {
        if m.get(&key).is_some_and(|e| e.1 == size && e.2 == mtime) {
            return None; // already known at this version
        }
        m.insert(key, (now, size, mtime));
        let cutoff = now.saturating_sub(FIRST_SEEN_TTL_MS);
        m.retain(|_, e| e.0 >= cutoff);
        if m.len() > FIRST_SEEN_CAP {
            let mut v: Vec<_> = m.iter().map(|(k, e)| (k.clone(), *e)).collect();
            v.sort_by_key(|(_, e)| std::cmp::Reverse(e.0));
            v.truncate(FIRST_SEEN_CAP);
            *m = v.into_iter().collect();
        }
        Some(m.clone())
    });
    if let (Some(snap), Some(file)) = (snapshot, first_seen_file()) {
        if let Ok(txt) = serde_json::to_string(&snap) {
            let _ = crate::settings::write_atomic(&file, txt.as_bytes());
        }
    }
}

/// When `path` appeared at THIS (size, mtime), if the watcher saw it appear.
fn appeared_at(path: &Path, size: u64, mtime: u64) -> Option<u64> {
    let key = path.to_string_lossy().to_string();
    with_first_seen(|m| m.get(&key).filter(|e| e.1 == size && e.2 == mtime).map(|e| e.0))
}

/// The watcher saw `path` appear/change: note it where the platform lacks ctime.
fn note_appeared_now(path: &Path) {
    if cfg!(unix) {
        return;
    }
    if let Ok(m) = std::fs::metadata(path) {
        note_appeared(path, m.len(), meta_mtime(&m), now_ms());
    }
}

/// A file's placement version (ms): `meta_version_ms` plus, where the platform
/// has no ctime, when the watcher saw it appear. The value every delete decision
/// uses. A pure lookup — scanning a folder never makes files look new.
pub(crate) fn path_version_ms(path: &Path, meta: &std::fs::Metadata) -> u64 {
    let v = meta_version_ms(meta);
    if cfg!(unix) {
        return v;
    }
    v.max(appeared_at(path, meta.len(), meta_mtime(meta)).unwrap_or(0))
}

// ── Case-insensitive volumes (D7) ────────────────────────────────────────────
/// Whether `folder` lives on a case-INSENSITIVE filesystem (APFS/HFS+ default,
/// NTFS, FAT/exFAT). Probed once per folder with a dot-file (never synced).
fn folder_case_insensitive(folder: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(folder) {
        return *v;
    }
    let root = Path::new(folder);
    if !root.is_dir() {
        return cfg!(any(target_os = "macos", target_os = "ios", windows));
    }
    let probe = root.join(format!(".dropbeam-case-{}", uuid::Uuid::new_v4().simple()));
    let v = if std::fs::write(&probe, b"").is_ok() {
        let upper = root.join(probe.file_name().unwrap().to_string_lossy().to_uppercase());
        let ci = upper.exists();
        let _ = std::fs::remove_file(&probe);
        ci
    } else {
        cfg!(any(target_os = "macos", target_os = "ios", windows))
    };
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(folder.to_string(), v);
    v
}

/// The name this path's LAST component actually has on disk, when it exists under
/// a different letter case (only possible on a case-insensitive volume). `None`
/// if it doesn't exist or the on-disk name matches exactly.
fn case_variant_on_disk(path: &Path) -> Option<std::ffi::OsString> {
    let name = path.file_name()?;
    if std::fs::symlink_metadata(path).is_err() {
        return None;
    }
    let parent = path.parent()?;
    let entries = std::fs::read_dir(parent).ok()?;
    let mut variant = None;
    for e in entries.flatten() {
        let n = e.file_name();
        if n == name {
            return None; // exact match exists
        }
        if n.to_string_lossy().to_lowercase() == name.to_string_lossy().to_lowercase() {
            variant = Some(n);
        }
    }
    variant
}

// ── Per-link "peer won't take this version" memory (D7/D8) ──────────────────
/// Signatures (rel|size|mtime) the PEER answered "kept" for — a name its disk
/// can't hold, a case clash, a version it deleted… Reconcile doesn't re-push these
/// until the file itself changes (new signature), so nothing loops.
fn skipped_path(config_dir: &Path, pair_id: &str) -> PathBuf {
    config_dir.join(format!("peer-skipped-{pair_id}.json"))
}

fn load_skipped(config_dir: &Path, pair_id: &str) -> HashMap<String, String> {
    std::fs::read_to_string(skipped_path(config_dir, pair_id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_skipped(config_dir: &Path, pair_id: &str, map: &HashMap<String, String>) {
    if let Ok(txt) = serde_json::to_string(map) {
        let _ = crate::settings::write_atomic(&skipped_path(config_dir, pair_id), txt.as_bytes());
    }
}

/// One status line for what the peer refused to take, if anything user-actionable.
fn skipped_warning(map: &HashMap<String, String>, peer: &str) -> Option<String> {
    let mut names = 0usize;
    let mut cases = 0usize;
    let mut example: Option<String> = None;
    for (sig, why) in map {
        let hit = match why.as_str() {
            "name" => {
                names += 1;
                true
            }
            "case" => {
                cases += 1;
                true
            }
            _ => false,
        };
        if hit && example.is_none() {
            example = sig_rel(sig).map(|r| file_name_of(&r));
        }
    }
    let ex = example.map(|e| format!(" (e.g. “{e}”)")).unwrap_or_default();
    match (names, cases) {
        (0, 0) => None,
        (n, 0) => Some(format!(
            "{n} file{} can't be copied to {peer} — the name isn't allowed on their computer{ex}. Rename {} to sync.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "it" } else { "them" }
        )),
        (0, c) => Some(format!(
            "{c} file{} can't be copied to {peer} — another file there has the same name in different capitals{ex}.",
            if c == 1 { "" } else { "s" }
        )),
        (n, c) => Some(format!(
            "{} files can't be copied to {peer} because of their names{ex}.",
            n + c
        )),
    }
}

// ── Deleted-version memory (tombstone TTL backstop) ──────────────────────────
/// Every version (rel|size|mtime) this folder deleted, with when — kept FAR longer
/// than tombstones (which expire after 45 days). A device that was offline longer
/// than that comes back still holding files everyone else deleted; with the
/// tombstones gone, its reconcile would resurrect them. The receiver refuses an
/// incoming copy whose exact version it deleted AFTER that copy was placed.
const DELETED_SIG_TTL_MS: u64 = 400 * 24 * 3600 * 1000;
const DELETED_SIG_CAP: usize = 50_000;

fn deleted_sigs_path(config_dir: &Path, folder: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let h = hex::encode(&Sha256::digest(folder.as_bytes())[..8]);
    config_dir.join(format!("deleted-versions-{h}.json"))
}

fn deleted_sigs_store() -> &'static Mutex<HashMap<PathBuf, HashMap<String, u64>>> {
    static S: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, u64>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_deleted_sigs<R>(config_dir: &Path, folder: &str, f: impl FnOnce(&mut HashMap<String, u64>) -> R) -> R {
    let path = deleted_sigs_path(config_dir, folder);
    let mut store = deleted_sigs_store().lock().unwrap_or_else(|e| e.into_inner());
    let map = store.entry(path.clone()).or_insert_with(|| {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// Remember that these exact versions were deleted at `ts`. One write per call.
fn record_deleted_versions(config_dir: &Path, folder: &str, sigs: &[String], ts: u64) {
    if sigs.is_empty() {
        return;
    }
    let path = deleted_sigs_path(config_dir, folder);
    let snapshot = with_deleted_sigs(config_dir, folder, |m| {
        for s in sigs {
            let e = m.entry(s.clone()).or_insert(0);
            *e = (*e).max(ts);
        }
        let cutoff = now_ms().saturating_sub(DELETED_SIG_TTL_MS);
        m.retain(|_, t| *t >= cutoff);
        if m.len() > DELETED_SIG_CAP {
            let mut v: Vec<(String, u64)> = m.iter().map(|(k, t)| (k.clone(), *t)).collect();
            v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
            v.truncate(DELETED_SIG_CAP);
            *m = v.into_iter().collect();
        }
        m.clone()
    });
    if let Ok(txt) = serde_json::to_string(&snapshot) {
        let _ = crate::settings::write_atomic(&path, txt.as_bytes());
    }
}

/// When this exact version was deleted here, if ever.
fn deleted_version_at(config_dir: &Path, folder: &str, sig: &str) -> Option<u64> {
    with_deleted_sigs(config_dir, folder, |m| m.get(sig).copied())
}

// ── Tombstone versions (D4, reconcile path) ─────────────────────────────────
/// rel → (size, mtime, ts): the exact version each tombstone deleted, when known.
/// Rides the reconcile snapshot as `tombVersions`, so a peer's self-heal deletes
/// only THAT version — never a same-named file the deleter never had.
fn tomb_vers_store() -> &'static Mutex<HashMap<String, HashMap<String, (u64, u64, u64)>>> {
    static S: OnceLock<Mutex<HashMap<String, HashMap<String, (u64, u64, u64)>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn tomb_vers_file(pair_id: &str) -> Option<PathBuf> {
    sync_config_dir().get().map(|d| d.join(format!("tombstone-versions-{pair_id}.json")))
}

fn with_tomb_vers<R>(pair_id: &str, f: impl FnOnce(&mut HashMap<String, (u64, u64, u64)>) -> R) -> R {
    let mut store = tomb_vers_store().lock().unwrap_or_else(|e| e.into_inner());
    let map = store.entry(pair_id.to_string()).or_insert_with(|| {
        tomb_vers_file(pair_id)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(map)
}

fn note_tomb_version(pair_id: &str, rel: &str, version: (u64, u64), ts: u64) {
    with_tomb_vers(pair_id, |m| {
        let e = m.entry(norm_rel(rel)).or_insert((version.0, version.1, ts));
        if ts >= e.2 {
            *e = (version.0, version.1, ts);
        }
    });
}

fn persist_tomb_versions(pair_id: &str) {
    let cutoff = now_ms().saturating_sub(TOMBSTONE_TTL_MS);
    let snap = with_tomb_vers(pair_id, |m| {
        m.retain(|_, e| e.2 >= cutoff);
        m.clone()
    });
    if let (Some(file), Ok(txt)) = (tomb_vers_file(pair_id), serde_json::to_string(&snap)) {
        let _ = crate::settings::write_atomic(&file, txt.as_bytes());
    }
}

fn tomb_versions_of(pair_id: &str) -> HashMap<String, (u64, u64)> {
    with_tomb_vers(pair_id, |m| m.iter().map(|(k, e)| (k.clone(), (e.0, e.1))).collect())
}

// ── Last full observation (D5) ───────────────────────────────────────────────
fn seen_path(config_dir: &Path, pair_id: &str) -> PathBuf {
    config_dir.join(format!("folder-seen-{pair_id}.json"))
}

/// When we last WALKED this folder and saw every file in it (ms).
fn load_last_seen(config_dir: &Path, pair_id: &str) -> Option<u64> {
    std::fs::read_to_string(seen_path(config_dir, pair_id))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .or_else(|| {
            // Upgrading from a build that didn't record it: the loop-guard manifest
            // was last written when we last synced something — a time we had it all.
            std::fs::metadata(manifest_path(config_dir, pair_id))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
        })
}

fn save_last_seen(config_dir: &Path, pair_id: &str, ts: u64) {
    let _ = crate::settings::write_atomic(&seen_path(config_dir, pair_id), ts.to_string().as_bytes());
}

/// D5: files we KNEW (we sent or received them — they're in the loop-guard
/// manifest) that are gone from disk with no tombstone: deleted while DropBeam
/// was closed, or while the watcher overflowed. Returns rel → the last version we
/// had. EMPTY when the folder isn't safely observable: the root is missing, or
/// EVERY known file vanished at once (an unmounted drive / swapped mount point
/// looks exactly like that — resurrection is the safe failure, never deletion).
fn missed_deletes(
    folder: &str,
    inbound: &HashSet<String>,
    tombstones: &HashMap<String, u64>,
) -> HashMap<String, (u64, u64)> {
    let root = Path::new(folder);
    if !folder_available(folder) {
        return HashMap::new();
    }
    // Latest known version per rel.
    let mut known: HashMap<String, (u64, u64)> = HashMap::new();
    for sig in inbound {
        let mut it = sig.rsplitn(3, '|');
        let (Some(mt), Some(sz), Some(rel)) = (it.next(), it.next(), it.next()) else { continue };
        let (Ok(mt), Ok(sz)) = (mt.parse::<u64>(), sz.parse::<u64>()) else { continue };
        if rel.is_empty()
            || rel.split('/').any(|c| c.starts_with('.'))
            || crate::iroh_net::is_control_rel(rel)
            || !rel_stays_inside(&norm_rel(rel))
        {
            continue;
        }
        let e = known.entry(norm_rel(rel)).or_insert((sz, mt));
        if mt > e.1 {
            *e = (sz, mt);
        }
    }
    if known.is_empty() {
        return HashMap::new();
    }
    // Compare against the folder's own (NFC-keyed) manifest, not by joining paths:
    // on a normalization-SENSITIVE disk an NFD-stored name isn't found under its
    // NFC key, and a case-only respelling isn't found on a case-sensitive one.
    let present = live_manifest(folder);
    let ci = folder_case_insensitive(folder);
    let present_folded: HashSet<String> =
        if ci { present.keys().map(|k| k.to_lowercase()).collect() } else { HashSet::new() };
    let missing: HashMap<String, (u64, u64)> = known
        .iter()
        .filter(|(rel, _)| !tombstones.contains_key(*rel))
        .filter(|(rel, _)| !present.contains_key(*rel) && !(ci && present_folded.contains(&rel.to_lowercase())))
        // Truly gone = NotFound. Any other error (permission, I/O on a flaky NAS)
        // proves nothing.
        .filter(|(rel, _)| {
            matches!(std::fs::symlink_metadata(root.join(rel)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        })
        // Under a symlinked directory (its target may be offline) — the walker
        // never lists those, so absence there proves nothing either.
        .filter(|(rel, _)| {
            let mut anc = Path::new(rel.as_str()).parent();
            while let Some(a) = anc {
                if a.as_os_str().is_empty() {
                    return true;
                }
                if std::fs::symlink_metadata(root.join(a)).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    return false;
                }
                anc = a.parent();
            }
            true
        })
        .map(|(r, v)| (r.clone(), *v))
        .collect();
    // Subtree guard: a directory that still EXISTS but is now empty while we knew
    // several files in it is an unmounted sub-volume / offline share far more
    // often than a deliberate delete — skip that whole subtree.
    let mut by_dir: HashMap<String, (usize, usize)> = HashMap::new();
    for rel in known.keys() {
        if let Some(parent) = Path::new(rel).parent().filter(|p| !p.as_os_str().is_empty()) {
            let e = by_dir.entry(parent.to_string_lossy().to_string()).or_insert((0, 0));
            e.0 += 1;
            if missing.contains_key(rel) {
                e.1 += 1;
            }
        }
    }
    let hollow: Vec<String> = by_dir
        .into_iter()
        .filter(|(dir, (known_n, missing_n))| {
            *known_n >= 2
                && known_n == missing_n
                && std::fs::read_dir(root.join(dir)).map(|mut it| it.next().is_none()).unwrap_or(false)
        })
        .map(|(d, _)| d)
        .collect();
    let missing: HashMap<String, (u64, u64)> = missing
        .into_iter()
        .filter(|(rel, _)| !hollow.iter().any(|d| rel.starts_with(&format!("{d}/"))))
        .collect();
    // "Suddenly empty": every known file gone at once, or the folder has no files
    // at all, is a drive/mount problem far more often than a deliberate delete.
    let all_gone = missing.len() == known.len() && known.len() > 3;
    if all_gone || (!missing.is_empty() && present.is_empty()) {
        log::warn!(
            "missed-delete scan of {folder}: {} of {} known files missing — looks like a \
             missing/empty drive, NOT propagating any deletes",
            missing.len(),
            known.len()
        );
        return HashMap::new();
    }
    missing
}

/// Wait until `ms` elapses or the pair stops. Returns false if stopped.
async fn sleep_unless_stopped(stop_notify: &Arc<Notify>, stopped: &Arc<AtomicBool>, ms: u64) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
        _ = stop_notify.notified() => {}
    }
    !stopped.load(Ordering::SeqCst)
}

/// Whether a delete may remove the local file at `abs`:
///   • the deleter told us WHICH version it deleted (size, mtime) → only that
///     exact version goes; anything else is a newer/different edit and stays,
///   • otherwise (older peers, reconcile tombstones) only a file whose placement
///     version is OLDER than the delete goes — a late delete never beats an edit.
fn delete_allowed(abs: &Path, ts: u64, known: Option<(u64, u64)>) -> bool {
    let Ok(meta) = std::fs::metadata(abs) else { return false };
    match known {
        // The exact version — unless this copy was clearly PLACED well after the
        // delete (restored from History, re-added identical bytes): a late or
        // re-sent delete must not undo that. 10 min of slack absorbs clock skew.
        Some((size, mtime)) => {
            meta.len() == size
                && meta_mtime(&meta) == mtime
                && path_version_ms(abs, &meta) <= ts.saturating_add(10 * 60 * 1000)
        }
        None => ts > path_version_ms(abs, &meta),
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

    /// A "now" far in the future for reconcile_plan tests, so the small epoch-second
    /// mtimes used below read as ancient (non-fresh) and exercise the normal rules.
    const TEST_NOW: u64 = 10_000_000_000_000;

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
    fn rel_stays_inside_rejects_root_escape_and_absolute() {
        assert!(rel_stays_inside("a.txt"));
        assert!(rel_stays_inside("sub/dir/a.txt"));
        assert!(rel_stays_inside("dots..in..name.txt"));
        assert!(!rel_stays_inside("")); // the folder root itself
        assert!(!rel_stays_inside("../Other"));
        assert!(!rel_stays_inside("sub/../../x"));
        assert!(!rel_stays_inside("/Users/me/Desktop/Empty"));
        #[cfg(windows)]
        {
            assert!(!rel_stays_inside("C:/Users/x/Documents"));
            assert!(!rel_stays_inside("C:"));
        }
    }

    /// A peer tombstone of "" / "../x" must not remove the (empty) shared folder
    /// root or an empty dir beside it via the reconcile empty-dir sweep.
    #[test]
    fn remote_delete_never_touches_root_or_outside() {
        let base = temp_dir("inside");
        let folder = base.join("share");
        let outside = base.join("Other");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut applied = Vec::new();
        let f = folder.to_str().unwrap();
        assert!(!apply_remote_delete(f, "", &sd, &mut applied, 0, &DeleteGuard::any()));
        assert!(!apply_remote_delete(f, "../Other", &sd, &mut applied, 0, &DeleteGuard::any()));
        assert!(folder.is_dir() && outside.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The watcher can report an aliased path (/private/var vs /var, firmlinks,
    /// a symlinked root). Dotfiles and DropBeam's own history archive must still be
    /// recognized as unsendable instead of slipping past a bare strip_prefix.
    #[cfg(unix)]
    #[test]
    fn sendable_candidate_filters_dotfiles_on_aliased_paths() {
        let base = temp_dir("alias");
        let real = base.join("real");
        std::fs::create_dir_all(real.join(".dropbeam-history/data")).unwrap();
        std::fs::write(real.join(".DS_Store"), b"x").unwrap();
        std::fs::write(real.join(".dropbeam-history/data/abc"), b"x").unwrap();
        std::fs::write(real.join("photo.jpg"), b"x").unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let folder = link.to_str().unwrap(); // configured via the alias…
        let ev = |n: &str| real.join(n).to_string_lossy().to_string(); // …events via the real path
        assert!(!is_sendable_candidate(&ev(".DS_Store"), folder, &inbound));
        assert!(!is_sendable_candidate(&ev(".dropbeam-history/data/abc"), folder, &inbound));
        assert!(is_sendable_candidate(&ev("photo.jpg"), folder, &inbound));
        let _ = std::fs::remove_dir_all(&base);
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
            &LandCtx::default(),
        )
        .moved
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
    fn type_swap_file_replaces_existing_directory() {
        // A peer replaced a folder named "notes" with a FILE named "notes". Without
        // the type-swap guard the incoming file can't overwrite the directory, so it
        // was silently dropped and re-queued forever. It must now land as a file, and
        // the old directory's contents must be preserved in History.
        let folder = temp_dir("swap-f");
        let staging = temp_dir("swap-s");
        std::fs::create_dir_all(folder.join("notes")).unwrap();
        write_with_mtime(&folder.join("notes/inner.txt"), b"kept", 1000);
        write_with_mtime(&staging.join("notes"), b"i am a file now", 2000);
        let moved = apply(&staging, &folder);
        assert!(folder.join("notes").is_file(), "incoming file must land");
        assert_eq!(std::fs::read(folder.join("notes")).unwrap(), b"i am a file now");
        assert_eq!(moved.len(), 1, "exactly the landed file is reported");
        // The replaced directory's child survives in History (nothing lost).
        let hist = folder.join(".dropbeam-history");
        let recoverable = list_files_rec(&hist)
            .iter()
            .any(|p| std::fs::read(p).map(|b| b == b"kept").unwrap_or(false));
        assert!(recoverable, "old directory contents archived to History");
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
            &LandCtx::default(),
        )
        .moved;
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
        let archived = apply_remote_delete(&folder_s, "sub/x.txt", &sd, &mut applied, 0, &DeleteGuard::any());
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
        assert!(!apply_remote_delete(&folder_s, "../escape.txt", &sd, &mut ap, 0, &DeleteGuard::any()));
        assert!(!apply_remote_delete(&folder_s, "/etc/passwd", &sd, &mut ap, 0, &DeleteGuard::any()));
        assert!(!apply_remote_delete(&folder_s, "a/../../b.txt", &sd, &mut ap, 0, &DeleteGuard::any()));
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
        let removed = apply_remote_delete(&folder_s, "clips", &sd, &mut applied, 0, &DeleteGuard::any());
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
        mine.insert("have.txt".into(), FileEntry { size: 1, mtime: 100, version_ms: 0, inode: 0 });
        mine.insert("alsohave.txt".into(), FileEntry { size: 2, mtime: 100, version_ms: 0, inode: 0 });
        // Peer has neither → both must be pushed; NOTHING deleted (no tombstones).
        let plan = reconcile_plan(&mine, &HashMap::new(), &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert_eq!(plan.push, vec!["alsohave.txt".to_string(), "have.txt".to_string()]);
        assert!(plan.delete.is_empty(), "absence alone must NEVER cause a delete");
    }

    #[test]
    fn viewer_warning_is_rate_limited() {
        let now = Instant::now();
        assert!(viewer_warning_due(None, now));
        assert!(!viewer_warning_due(Some(now), now + Duration::from_secs(59)));
        assert!(viewer_warning_due(Some(now), now + Duration::from_secs(60)));
    }

    // ---- compute_verify (the Verify-button answer) ----

    fn fe(size: u64, mtime: u64) -> FileEntry {
        FileEntry { size, mtime, version_ms: mtime.saturating_mul(1000), inode: 0 }
    }

    #[test]
    fn verify_identical_folders_report_match() {
        let mut mine = HashMap::new();
        mine.insert("a.txt".into(), fe(10, 100));
        mine.insert("sub/b.bin".into(), fe(20, 100));
        let mut rec = Reconcile::default();
        rec.files.insert("a.txt".into(), (10, 100));
        rec.files.insert("sub/b.bin".into(), (20, 100));
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(r.identical, "same paths + sizes must read as identical");
        assert_eq!(r.matched, 2);
        assert_eq!(r.differences, 0);
        assert_eq!(r.local_files, 2);
        assert_eq!(r.peer_files, 2);
    }

    #[test]
    fn verify_identical_when_only_mtime_differs() {
        // Two machines round mtimes differently — an mtime-only delta is NOT a
        // difference (same rule the reconcile uses to avoid re-sending forever).
        let mut mine = HashMap::new();
        mine.insert("a.txt".into(), fe(10, 100));
        let mut rec = Reconcile::default();
        rec.files.insert("a.txt".into(), (10, 102)); // same size, 2s skew
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(r.identical, "mtime-only skew is not a real difference");
        assert_eq!(r.matched, 1);
    }

    #[test]
    fn verify_same_path_different_size_same_mtime_is_not_identical() {
        // The trust-breaking edge: same path + SAME mtime but DIFFERENT byte size.
        // The mtime-based planner pushes nothing (equal mtime → no winner), so this
        // must be caught explicitly or "identical" would be wrongly reported true.
        let mut mine = HashMap::new();
        mine.insert("x.txt".into(), fe(10, 100));
        let mut rec = Reconcile::default();
        rec.files.insert("x.txt".into(), (20, 100)); // different size, identical mtime
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(!r.identical, "a same-path size mismatch is never 'identical'");
        assert_eq!(r.matched, 0, "different size → not matched");
        assert!(r.differences >= 1);
    }

    #[test]
    fn verify_counts_files_to_send_and_receive() {
        let mut mine = HashMap::new();
        mine.insert("shared.txt".into(), fe(5, 100));
        mine.insert("only_local.txt".into(), fe(7, 100)); // peer lacks → we send
        let mut rec = Reconcile::default();
        rec.files.insert("shared.txt".into(), (5, 100));
        rec.files.insert("only_peer.txt".into(), (9, 100)); // we lack → we receive
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(!r.identical);
        assert_eq!(r.matched, 1, "only shared.txt matches");
        assert_eq!(r.missing_on_peer, 1);
        assert_eq!(r.missing_locally, 1);
        assert_eq!(r.pending_deletes, 0);
        assert_eq!(r.differences, 2);
    }

    #[test]
    fn verify_counts_a_peer_tombstone_as_a_pending_delete() {
        // Peer deleted a file we still have (tombstone newer than our copy) — that's
        // a difference still converging.
        let mut mine = HashMap::new();
        mine.insert("gone.txt".into(), fe(3, 100)); // mtime 100s → 100_000ms
        let mut rec = Reconcile::default();
        rec.tombstones.insert("gone.txt".into(), 200_000); // deleted at 200s
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(!r.identical);
        assert_eq!(r.pending_deletes, 1);
        assert_eq!(r.missing_on_peer, 0, "a tombstoned file is never 'to send'");
        assert_eq!(r.differences, 1);
    }

    #[test]
    fn verify_counts_our_tombstone_for_a_file_the_peer_still_has() {
        // WE deleted a file; the peer still has its older copy → we'll push our
        // tombstone. One pending delete, not a "to receive".
        let mine: HashMap<String, FileEntry> = HashMap::new();
        let mut rec = Reconcile::default();
        rec.files.insert("doc.txt".into(), (4, 100)); // peer's copy, mtime 100s
        let mut my_tombs = HashMap::new();
        my_tombs.insert("doc.txt".to_string(), 200_000u64); // we deleted at 200s
        let r = compute_verify(&mine, &rec, &my_tombs, TEST_NOW);
        assert!(!r.identical);
        assert_eq!(r.pending_deletes, 1);
        assert_eq!(r.missing_locally, 0, "we deleted it; it's not 'to receive'");
        assert_eq!(r.differences, 1);
    }

    #[test]
    fn verify_empty_folders_are_identical() {
        let mine: HashMap<String, FileEntry> = HashMap::new();
        let rec = Reconcile::default();
        let r = compute_verify(&mine, &rec, &HashMap::new(), TEST_NOW);
        assert!(r.identical);
        assert_eq!(r.matched, 0);
        assert_eq!(r.differences, 0);
    }

    #[test]
    fn reconcile_plan_applies_a_newer_tombstone_as_a_delete() {
        let mut mine = HashMap::new();
        mine.insert("old.mov".into(), FileEntry { size: 9, mtime: 100, version_ms: 0, inode: 0 }); // mtime 100s
        let mut peer_tombs = HashMap::new();
        peer_tombs.insert("old.mov".to_string(), 200_000u64); // deleted at 200s (ms)
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new(), TEST_NOW);
        assert_eq!(plan.delete, vec!["old.mov".to_string()]);
        assert!(plan.push.is_empty(), "a tombstoned file is never pushed");
    }

    #[test]
    fn reconcile_plan_local_edit_newer_than_tombstone_wins() {
        // We edited the file AFTER the peer's delete → keep + push (edit beats delete).
        let mut mine = HashMap::new();
        mine.insert("doc.txt".into(), FileEntry { size: 9, mtime: 300, version_ms: 0, inode: 0 }); // edited at 300s
        let mut peer_tombs = HashMap::new();
        peer_tombs.insert("doc.txt".to_string(), 200_000u64); // delete at 200s
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new(), TEST_NOW);
        assert!(plan.delete.is_empty(), "newer local edit must survive the delete");
        assert_eq!(plan.push, vec!["doc.txt".to_string()]);
    }

    #[test]
    fn reconcile_plan_freshly_dropped_file_with_old_mtime_is_not_deleted() {
        // THE data-loss bug: a folder of videos dropped into a synced folder keeps
        // its OLD content mtime, but a bogus "deleted" tombstone arrives stamped
        // ~now. The file's placement VERSION (ctime/birthtime ≈ now) must defend it:
        // it's within the grace window, so it is KEPT and re-pushed, never wiped.
        let now = 1_900_000_000_000u64; // fixed "now" in ms
        let mut mine = HashMap::new();
        // mtime is months old (epoch seconds), but version_ms is "just now".
        mine.insert(
            "Smartness Deluxe/clip.mp4".into(),
            FileEntry {
                size: 246_806_641,
                mtime: 1_700_000_000, // old content time (seconds)
                version_ms: now - 3_000, // dropped 3s ago
                inode: 0,
            },
        );
        let mut peer_tombs = HashMap::new();
        // Peer tombstoned it 1s ago — newer than the old mtime, but the file is fresh.
        peer_tombs.insert("Smartness Deluxe/clip.mp4".to_string(), now - 1_000);
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new(), now);
        assert!(
            plan.delete.is_empty(),
            "a freshly-dropped file must NOT be deleted by a same-instant tombstone"
        );
        // It is also NOT force-pushed from reconcile (that would resurrect a file
        // legitimately deleted right after a sync). The live watcher sends genuine
        // user drops; reconcile just protects the fresh file from deletion.
        assert!(
            plan.push.is_empty(),
            "a fresh file is skipped by reconcile (neither deleted nor pushed)"
        );
    }

    #[test]
    fn reconcile_plan_settled_old_file_still_obeys_a_tombstone() {
        // The guard must NOT block legitimate deletes: a file that has sat in the
        // folder since well before the grace window still deletes on a peer tombstone.
        let now = 1_900_000_000_000u64;
        let mut mine = HashMap::new();
        mine.insert(
            "old/settled.mov".into(),
            FileEntry {
                size: 9,
                mtime: 1_700_000_000,
                version_ms: now - 60 * 60 * 1000, // placed an hour ago (past grace)
                inode: 0,
            },
        );
        let mut peer_tombs = HashMap::new();
        peer_tombs.insert("old/settled.mov".to_string(), now - 30 * 60 * 1000); // deleted 30m ago
        let plan = reconcile_plan(&mine, &HashMap::new(), &peer_tombs, &HashMap::new(), now);
        assert_eq!(plan.delete, vec!["old/settled.mov".to_string()]);
        assert!(plan.push.is_empty());
    }

    #[test]
    fn within_grace_is_bounded_on_both_sides() {
        let now = 1_000_000_000u64;
        let g = 10 * 60 * 1000; // 10 min
        assert!(within_grace(now - 1000, now, g), "1s ago = fresh");
        assert!(within_grace(now + 1000, now, g), "1s future (skew) = fresh");
        assert!(!within_grace(now - g - 1, now, g), "past the window = not fresh");
        assert!(
            !within_grace(now + 100 * g, now, g),
            "a wildly future-stamped file must NOT be protected forever"
        );
        assert!(!within_grace(0, now, g), "no timestamp = not fresh");
        assert!(!within_grace(now, now, 0), "zero grace disables protection");
    }

    #[test]
    fn apply_remote_delete_refuses_a_freshly_created_file() {
        // The receive-side net: even if a bogus tombstone slips through, a file the
        // user just created on disk is refused (it's fresh) and survives.
        let folder = temp_dir("fresh-del");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("just-dropped.mp4"), b"brand new").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut applied = Vec::new();
        let removed = apply_remote_delete(&folder_s, "just-dropped.mp4", &sd, &mut applied, DELETE_GRACE_MS, &DeleteGuard::any());
        assert!(!removed, "a just-created file must not be deleted");
        assert!(applied.is_empty());
        assert!(
            folder.join("just-dropped.mp4").exists(),
            "the fresh file must still be on disk"
        );
        assert!(
            crate::folder_history::load(&folder_s).is_empty(),
            "and nothing should have been archived"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    // size+mtime of a file just written, for the content-verified move apply.
    fn sig_of(p: &std::path::Path) -> (u64, u64) {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), meta_mtime(&m))
    }

    #[test]
    fn apply_remote_move_renames_when_to_missing() {
        // The clean case: peer renamed a/x.txt → a/sub/x.txt. We relocate our copy
        // (no re-download) and arm the loop-guards so our own watcher won't echo it.
        let folder = temp_dir("mv-rename");
        let fs = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("a")).unwrap();
        std::fs::write(folder.join("a/x.txt"), b"payload").unwrap();
        let (sz, mt) = sig_of(&folder.join("a/x.txt"));
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        let did = apply_remote_move(&fs, "a/x.txt", "a/sub/x.txt", sz, mt, &sd, &inb, 0);
        assert!(did);
        assert!(!folder.join("a/x.txt").exists(), "source is moved away");
        assert_eq!(std::fs::read(folder.join("a/sub/x.txt")).unwrap(), b"payload");
        assert!(sd.lock().unwrap().contains_key("a/x.txt"), "source marked self-deleted");
        assert!(!inb.lock().unwrap().is_empty(), "dest signature pre-registered (no echo)");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn apply_remote_move_refuses_when_source_content_differs() {
        // CONTENT VERIFICATION: a move op whose size/mtime DON'T match our local
        // `from` (a coincidental new file, or a mis-detected/stale move) must NOT
        // be relocated — we leave it untouched and let delete+reconcile converge.
        let folder = temp_dir("mv-mismatch");
        let fs = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("report.txt"), b"the user's NEW unrelated file").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        // Op claims report.txt (size 5, mtime 123) moved → archive/report.txt.
        let did = apply_remote_move(&fs, "report.txt", "archive/report.txt", 5, 123, &sd, &inb, 0);
        assert!(!did, "a non-matching source is never moved");
        assert!(folder.join("report.txt").exists(), "the user's file is untouched");
        assert!(!folder.join("archive/report.txt").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn apply_remote_move_removes_orphan_only_when_to_matches() {
        // A reconcile push raced ahead and `to` already has the SAME bytes → `from`
        // is a stale duplicate → archive+remove it. (If `to` differed, we'd leave both.)
        let folder = temp_dir("mv-dedup");
        let fs = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("old.txt"), b"same").unwrap();
        let (sz, mt) = sig_of(&folder.join("old.txt"));
        std::fs::write(folder.join("new.txt"), b"same").unwrap();
        // Make `new.txt` share old's mtime (a real synced copy would).
        stamp_mtime(&folder.join("new.txt"), mt);
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        let did = apply_remote_move(&fs, "old.txt", "new.txt", sz, mt, &sd, &inb, 0);
        assert!(did, "the orphaned duplicate `from` is removed");
        assert!(!folder.join("old.txt").exists(), "duplicate gone");
        assert_eq!(std::fs::read(folder.join("new.txt")).unwrap(), b"same", "content kept at `to`");
        assert!(
            !crate::folder_history::load(&fs).is_empty(),
            "the removed copy is recoverable from history"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn detect_moves_from_index_finds_raced_relocation() {
        // Prior index had inode 7 at "a.jpg"; the live manifest now has inode 7 at
        // "sub/a.jpg" and "a.jpg" is gone → a same-volume move the live add-branch may
        // have raced past. The reconcile detector must surface it (so it becomes a
        // rename op, not a re-upload + orphan + duplicate).
        let mut idx: HashMap<u64, (String, u64, u64)> = HashMap::new();
        // A rename preserves mtime, so the prior index mtime equals the new file's.
        idx.insert(7, ("a.jpg".into(), 100, 111));
        idx.insert(9, ("keep.txt".into(), 50, 5));
        let mut lm: HashMap<String, FileEntry> = HashMap::new();
        lm.insert("sub/a.jpg".into(), FileEntry { size: 100, mtime: 111, version_ms: 111_000, inode: 7 });
        lm.insert("keep.txt".into(), FileEntry { size: 50, mtime: 5, version_ms: 5_000, inode: 9 });
        let prev: HashMap<String, FileEntry> = HashMap::new();
        let got = detect_moves_from_index(&idx, &prev, &lm);
        assert_eq!(got, vec![("a.jpg".to_string(), "sub/a.jpg".to_string(), 100, 111)]);
    }

    #[test]
    fn detect_moves_from_index_ignores_nonmoves_and_inode_zero() {
        let mut idx: HashMap<u64, (String, u64, u64)> = HashMap::new();
        idx.insert(7, ("a.jpg".into(), 100, 1));
        let mut lm: HashMap<String, FileEntry> = HashMap::new();
        // Same inode, same rel → not a move.
        lm.insert("a.jpg".into(), FileEntry { size: 100, mtime: 1, version_ms: 1_000, inode: 7 });
        // A COPY (old still present, new inode) → not a move.
        lm.insert("copy.jpg".into(), FileEntry { size: 100, mtime: 2, version_ms: 2_000, inode: 42 });
        // Inode 0 with no prior manifest entry → never matched here.
        lm.insert("win.bin".into(), FileEntry { size: 100, mtime: 3, version_ms: 3_000, inode: 0 });
        let prev: HashMap<String, FileEntry> = HashMap::new();
        assert!(detect_moves_from_index(&idx, &prev, &lm).is_empty());
    }

    #[test]
    fn detect_moves_from_index_windows_fallback_matches_by_size_mtime() {
        // Windows has no file id (inode 0). "a.jpg" (100, mtime 7) vanished since the prior
        // manifest and "sub/a.jpg" (100, mtime 7) appeared → the unique size+mtime match
        // identifies the move, so a Windows reorg relocates instead of re-uploading.
        let idx: HashMap<u64, (String, u64, u64)> = HashMap::new();
        let mut prev: HashMap<String, FileEntry> = HashMap::new();
        prev.insert("a.jpg".into(), FileEntry { size: 100, mtime: 7, version_ms: 7_000, inode: 0 });
        let mut lm: HashMap<String, FileEntry> = HashMap::new();
        lm.insert("sub/a.jpg".into(), FileEntry { size: 100, mtime: 7, version_ms: 7_000, inode: 0 });
        let got = detect_moves_from_index(&idx, &prev, &lm);
        assert_eq!(got, vec![("a.jpg".to_string(), "sub/a.jpg".to_string(), 100, 7)]);
    }

    #[test]
    fn detect_moves_from_index_windows_fallback_skips_ambiguous() {
        // Two vanished files share size+mtime → ambiguous → NO move (safe re-upload).
        let idx: HashMap<u64, (String, u64, u64)> = HashMap::new();
        let mut prev: HashMap<String, FileEntry> = HashMap::new();
        prev.insert("a.txt".into(), FileEntry { size: 10, mtime: 1, version_ms: 1_000, inode: 0 });
        prev.insert("b.txt".into(), FileEntry { size: 10, mtime: 1, version_ms: 1_000, inode: 0 });
        let mut lm: HashMap<String, FileEntry> = HashMap::new();
        lm.insert("sub/x.txt".into(), FileEntry { size: 10, mtime: 1, version_ms: 1_000, inode: 0 });
        assert!(
            detect_moves_from_index(&idx, &prev, &lm).is_empty(),
            "ambiguous same-signature set ⇒ no move"
        );
    }

    #[test]
    fn detect_moves_from_index_rejects_inode_recycle_same_size_diff_mtime() {
        // inode 7 was "a.jpg" (size 100, mtime 1). a.jpg is GONE; inode 7 now belongs to
        // a DIFFERENT file "recycled.bin" of the same SIZE but a different mtime (inode
        // reuse). Without the mtime guard this would synthesize a false move that
        // wrongly relocates the peer's a.jpg. The mtime guard must reject it.
        let mut idx: HashMap<u64, (String, u64, u64)> = HashMap::new();
        idx.insert(7, ("a.jpg".into(), 100, 1));
        let mut lm: HashMap<String, FileEntry> = HashMap::new();
        lm.insert("recycled.bin".into(), FileEntry { size: 100, mtime: 999, version_ms: 999_000, inode: 7 });
        let prev: HashMap<String, FileEntry> = HashMap::new();
        assert!(
            detect_moves_from_index(&idx, &prev, &lm).is_empty(),
            "inode recycle into a same-size, different-mtime file is NOT a move"
        );
    }

    #[test]
    fn handle_move_candidate_skips_send_when_move_already_recorded() {
        // The reconcile round already recorded a move TO sub/a.jpg. The live add-branch
        // must then report "handled" (true) so it does NOT also re-upload the bytes —
        // re-uploading is what orphaned the old path and produced the duplicate.
        let folder = temp_dir("mv-skip");
        let fs = folder.to_string_lossy().to_string();
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        let p = folder.join("sub/a.jpg");
        std::fs::write(&p, b"payload").unwrap();
        let (sz, mt) = sig_of(&p);
        let moves: Arc<Mutex<HashMap<String, MoveRec>>> = Arc::new(Mutex::new(HashMap::new()));
        // A move A→sub/a.jpg recorded with THIS file's signature ⇒ it's the moved file.
        moves.lock().unwrap().insert("a.jpg".into(), ("sub/a.jpg".into(), sz, mt, now_ms()));
        let ino_index = Arc::new(Mutex::new(HashMap::new()));
        let cw = Arc::new(Notify::new());
        let did = handle_move_candidate(&p.to_string_lossy(), &fs, &folder, "pid", &moves, &ino_index, &cw);
        assert!(did, "an already-recorded move of THIS file ⇒ skip the byte-send");

        // Critical: a genuinely DIFFERENT new file dropped at the same path (a move
        // op for a different-sized file lingers) must NOT be skipped — it must still
        // sync, or it would be silently lost.
        let moves2: Arc<Mutex<HashMap<String, MoveRec>>> = Arc::new(Mutex::new(HashMap::new()));
        moves2.lock().unwrap().insert("a.jpg".into(), ("sub/a.jpg".into(), sz + 999, mt, now_ms()));
        let did2 = handle_move_candidate(&p.to_string_lossy(), &fs, &folder, "pid", &moves2, &ino_index, &cw);
        assert!(!did2, "a different file at the same path must NOT be skipped");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn apply_remote_move_noop_when_from_missing() {
        // `from` already gone (move applied earlier, or we never had it) → no-op;
        // never touches an unrelated file at `to`.
        let folder = temp_dir("mv-noop");
        let fs = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("here.txt"), b"x").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        assert!(!apply_remote_move(&fs, "gone.txt", "here.txt", 1, 1, &sd, &inb, 0));
        assert!(folder.join("here.txt").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn apply_remote_move_refuses_fresh_source() {
        // A just-written source (within grace) must NOT be yanked — the user may be
        // editing it; this is the same data-loss guard the delete path enforces.
        let folder = temp_dir("mv-fresh");
        let fs = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("editing.txt"), b"live").unwrap();
        let (sz, mt) = sig_of(&folder.join("editing.txt"));
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        let did = apply_remote_move(&fs, "editing.txt", "moved.txt", sz, mt, &sd, &inb, DELETE_GRACE_MS);
        assert!(!did);
        assert!(folder.join("editing.txt").exists());
        assert!(!folder.join("moved.txt").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn apply_remote_move_rejects_path_escape() {
        // A peer can never make a move reach outside the shared folder.
        let folder = temp_dir("mv-escape");
        let fs = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("ok.txt"), b"x").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inb = Arc::new(Mutex::new(HashSet::new()));
        assert!(!apply_remote_move(&fs, "ok.txt", "../escape.txt", 1, 1, &sd, &inb, 0));
        assert!(!apply_remote_move(&fs, "../../etc/x", "ok2.txt", 1, 1, &sd, &inb, 0));
        assert!(folder.join("ok.txt").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn note_move_persists_and_collapses_chains() {
        let dir = temp_dir("mv-persist");
        let m = Arc::new(Mutex::new(HashMap::new()));
        note_move(&dir, "p", &m, "a.txt", "b.txt", 10, 100, now_ms());
        // a→b then b→c collapses to a→c, so the peer does one relocation, not two.
        note_move(&dir, "p", &m, "b.txt", "c.txt", 10, 100, now_ms());
        let reloaded = load_moves(&dir, "p");
        assert_eq!(reloaded.get("a.txt").map(|(t, _, _, _)| t.as_str()), Some("c.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tombstone_future_timestamp_is_clamped() {
        let dir = temp_dir("tomb");
        let tomb = Arc::new(Mutex::new(HashMap::new()));
        // A peer claims a delete a century in the future → must be clamped to ~now,
        // so it can't sit "newer than everything forever" and delete wanted files.
        let absurd = now_ms() + 100 * 365 * 24 * 3600 * 1000;
        assert!(note_tombstone(&tomb, "x.mov", absurd), "first record changes the map");
        let stored = *tomb.lock().unwrap().get("x.mov").unwrap();
        assert!(stored <= now_ms() + 24 * 3600 * 1000 + 1000, "clamped to ~now+1day");
        // A normal recent timestamp is kept as-is.
        let normal = now_ms().saturating_sub(5000);
        note_tombstone(&tomb, "y.mov", normal);
        assert_eq!(*tomb.lock().unwrap().get("y.mov").unwrap(), normal);
        // An equal-or-older re-record is a no-op (returns false → no batch save).
        assert!(!note_tombstone(&tomb, "y.mov", normal));
        // Batched persistence round-trips through disk.
        persist_tombstones(&dir, "p", &tomb);
        let loaded = load_tombstones(&dir, "p");
        assert_eq!(loaded.get("y.mov").copied(), Some(normal));
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
    fn nested_empty_dir_round_trips_to_the_peer() {
        // GitHub #22 — "a shared folder containing a nested EMPTY subfolder failed to
        // send". Exercises the real send path (live_empty_dirs), the real wire shape
        // (the `emptyDirs` JSON array the control beacon carries), and the real
        // receive path (apply_empty_dirs), end to end.
        let src = temp_dir("emptyrt-src");
        let dst = temp_dir("emptyrt-dst");
        std::fs::create_dir_all(src.join("a/b")).unwrap(); // `a` holds only `b`; `b` is empty
        std::fs::create_dir_all(src.join("docs/drafts")).unwrap(); // empty, under a populated dir
        std::fs::write(src.join("docs/read.txt"), b"x").unwrap();

        let mut dirs = live_empty_dirs(&src.to_string_lossy());
        dirs.sort();
        assert_eq!(
            dirs,
            vec!["a".to_string(), "a/b".to_string(), "docs/drafts".to_string()],
            "the nested empty dir must be discovered, not just its parent"
        );

        // Exactly what rides the beacon (iroh_net encodes `emptyDirs` as a string
        // array and decodes it back), so serialization is part of the round trip.
        let wire = serde_json::to_string(&dirs).unwrap();
        let received: Vec<String> = serde_json::from_str(&wire).unwrap();

        apply_empty_dirs(&dst.to_string_lossy(), &received, &HashMap::new(), &HashMap::new());
        assert!(dst.join("a").is_dir(), "the parent empty dir must land");
        assert!(dst.join("a/b").is_dir(), "the NESTED empty dir must land");
        assert!(dst.join("docs/drafts").is_dir(), "empty dir under a populated one must land");

        // A tombstoned ancestor must NOT be resurrected by its nested child.
        let dst2 = temp_dir("emptyrt-tomb");
        let mut tomb = HashMap::new();
        tomb.insert("a".to_string(), 1u64);
        apply_empty_dirs(&dst2.to_string_lossy(), &received, &tomb, &HashMap::new());
        assert!(!dst2.join("a").exists(), "a deleted parent must not come back via a/b");
        assert!(dst2.join("docs/drafts").is_dir(), "unrelated empty dirs still land");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
        let _ = std::fs::remove_dir_all(&dst2);
    }

    #[test]
    fn watch_failure_is_logged_once_until_the_watch_succeeds() {
        // The "watch failed for <path>: No path was found" x74 spam: start_pair is
        // re-entered on every structural change, so the same missing folder must
        // only be reported once — and again if it goes missing a second time.
        let folder = "/dropbeam-test/never-exists-watch-dedup";
        clear_watch_failure(folder);
        assert!(note_watch_failure(folder), "first failure logs");
        assert!(!note_watch_failure(folder), "repeats are suppressed");
        assert!(!note_watch_failure(folder));
        clear_watch_failure(folder); // the watch came back
        assert!(note_watch_failure(folder), "a later disappearance logs again");
        clear_watch_failure(folder);
    }

    #[test]
    fn reconcile_plan_identical_files_do_nothing() {
        let mut mine = HashMap::new();
        mine.insert("same.txt".into(), FileEntry { size: 5, mtime: 100, version_ms: 0, inode: 0 });
        let mut peer = HashMap::new();
        peer.insert("same.txt".to_string(), (5u64, 100u64));
        let plan = reconcile_plan(&mine, &peer, &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert!(plan.push.is_empty() && plan.delete.is_empty(), "no-op when in sync");
    }

    #[test]
    fn reconcile_same_size_mtime_skew_does_not_loop() {
        // The infinite-re-send bug: same file, mtimes 1s apart (cross-filesystem
        // rounding). Must NOT push — and must be symmetric so neither side loops.
        let mut mine = HashMap::new();
        mine.insert("vid.mp4".into(), FileEntry { size: 1000, mtime: 12346, version_ms: 0, inode: 0 });
        let mut peer = HashMap::new();
        peer.insert("vid.mp4".to_string(), (1000u64, 12345u64));
        let plan = reconcile_plan(&mine, &peer, &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert!(plan.push.is_empty(), "same-size file with skewed mtime must not re-push");
        // The peer side (roles flipped) must also not push → converged.
        let mut peer_mine = HashMap::new();
        peer_mine.insert("vid.mp4".into(), FileEntry { size: 1000, mtime: 12345, version_ms: 0, inode: 0 });
        let mut my_snap = HashMap::new();
        my_snap.insert("vid.mp4".to_string(), (1000u64, 12346u64));
        let plan2 = reconcile_plan(&peer_mine, &my_snap, &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert!(plan2.push.is_empty(), "convergence: the other side must not push either");
    }

    #[test]
    fn reconcile_subfolder_nfd_nfc_is_the_same_file() {
        // macOS stores "Résumé/x" decomposed (NFD); the peer composed (NFC). They
        // must be recognized as ONE file, not re-sent forever.
        let nfd = "Re\u{301}sume\u{301}/x.bin"; // e + combining acute
        let nfc = "R\u{e9}sum\u{e9}/x.bin"; // é precomposed
        let mut mine = HashMap::new();
        mine.insert(nfd.to_string(), FileEntry { size: 10, mtime: 5, version_ms: 0, inode: 0 });
        let mut peer = HashMap::new();
        peer.insert(nfc.to_string(), (10u64, 5u64));
        let plan = reconcile_plan(&mine, &peer, &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert!(plan.push.is_empty(), "NFD and NFC forms of the same name must match");
    }

    #[test]
    fn reconcile_still_pushes_missing_and_changed() {
        let mut mine = HashMap::new();
        mine.insert("new.txt".into(), FileEntry { size: 7, mtime: 50, version_ms: 0, inode: 0 });
        mine.insert("edited.txt".into(), FileEntry { size: 200, mtime: 99, version_ms: 0, inode: 0 });
        let mut peer = HashMap::new();
        peer.insert("edited.txt".to_string(), (100u64, 40u64)); // peer has older, smaller
        let plan = reconcile_plan(&mine, &peer, &HashMap::new(), &HashMap::new(), TEST_NOW);
        assert!(plan.push.contains(&"new.txt".to_string()), "missing file is pushed");
        assert!(
            plan.push.contains(&"edited.txt".to_string()),
            "different-size newer file is pushed"
        );
    }

    #[test]
    fn history_is_a_dotdir_so_it_never_syncs() {
        // The history dir must start with '.' so is_sendable_candidate skips it.
        let folder = temp_dir("hist");
        let folder_s = folder.to_string_lossy().to_string();
        std::fs::write(folder.join("a.txt"), b"x").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        apply_remote_delete(&folder_s, "a.txt", &sd, &mut ap, 0, &DeleteGuard::any());
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

    // ── Regression tests for the 2026-10-04 folder-sync audit ──────────────────

    fn report(items: Vec<(&str, crate::iroh_net::FolderItemOutcome)>, legacy: bool) -> crate::iroh_net::FolderSendReport {
        crate::iroh_net::FolderSendReport {
            locality: Locality::Unknown,
            items: items.into_iter().map(|(p, o)| (PathBuf::from(p), o)).collect(),
            legacy,
        }
    }

    fn version_of(p: &Path) -> (u64, u64) {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), meta_mtime(&m))
    }

    #[test]
    fn send_queue_dedupes_in_constant_time_and_keeps_order() {
        let mut q = SendQueue::default();
        assert!(q.push_back("a".into()));
        assert!(q.push_back("b".into()));
        assert!(!q.push_back("a".into()), "already queued");
        assert!(q.push_back("c".into()));
        q.rotate_to_back("a");
        assert_eq!(q.iter().cloned().collect::<Vec<_>>(), vec!["b", "c", "a"]);
        assert!(q.remove("c"));
        assert!(!q.remove("c"));
        assert_eq!(q.front().map(String::as_str), Some("b"));
        assert_eq!(q.len(), 2);
        assert!(q.push_back("c".into()), "re-queue after removal");
        // P3-d: an item the peer keeps failing to land is parked, not resent forever.
        assert!(!q.note_failed("b"));
        assert!(!q.note_failed("b"));
        assert!(q.note_failed("b"));
        assert!(!q.iter().any(|x| x == "b"));
        assert!(!q.push_back("b".into()), "parked: rescan/reconcile can't re-queue it");
        q.note_ok("b");
        assert!(q.push_back("b".into()));
    }

    /// D1: only items the receiver confirmed LANDED may be auto-deleted — never a
    /// kept/failed/unreported item, and nothing at all on a legacy bare "ok".
    #[test]
    fn auto_delete_only_ever_targets_confirmed_landed_items() {
        use crate::iroh_net::FolderItemOutcome as O;
        let dir = temp_dir("d1-settle");
        let mk = |n: &str| {
            let p = dir.join(n);
            std::fs::write(&p, n.as_bytes()).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b, c, d) = (mk("a"), mk("b"), mk("c"), mk("d"));
        let batch = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let pre: HashMap<String, (u64, u64)> =
            batch.iter().map(|f| (f.clone(), version_of(Path::new(f)))).collect();
        let r = report(
            vec![(&a, O::Landed), (&b, O::Kept("conflict".into())), (&c, O::Failed)],
            false,
        );
        let st = settle_batch(&batch, &pre, &r);
        assert_eq!(st.delivered, vec![a.clone()]);
        assert_eq!(st.kept, vec![(b.clone(), "conflict".to_string())]);
        assert_eq!(st.retry, vec![c.clone(), d.clone()], "failed + unreported are retried");
        assert_eq!(auto_delete_targets(&st, false, true), vec![a.clone()]);
        assert!(auto_delete_targets(&st, false, false).is_empty(), "auto-delete off");
        // A legacy receiver's bare "ok" can't vouch per file → delete nothing.
        let legacy = report(batch.iter().map(|f| (f.as_str(), O::Landed)).collect(), true);
        let st = settle_batch(&batch, &pre, &legacy);
        assert_eq!(st.delivered.len(), 4);
        assert!(auto_delete_targets(&st, true, true).is_empty());
        // An EMPTY report (nothing confirmed) delivers nothing.
        let st = settle_batch(&batch, &pre, &report(vec![], false));
        assert!(st.delivered.is_empty() && auto_delete_targets(&st, false, true).is_empty());
    }

    /// D2: a file that changed while it was being sent is NOT delivered (the peer
    /// has an older version) — it's re-sent, never recorded or auto-deleted.
    #[test]
    fn file_changed_during_send_is_resent_not_deleted() {
        use crate::iroh_net::FolderItemOutcome as O;
        let dir = temp_dir("d2-changed");
        let f = dir.join("growing.mov");
        std::fs::write(&f, b"first half").unwrap();
        let fs_ = f.to_string_lossy().to_string();
        let pre: HashMap<String, (u64, u64)> = [(fs_.clone(), version_of(&f))].into();
        std::fs::write(&f, b"first half + second half").unwrap(); // still being written
        let st = settle_batch(&[fs_.clone()], &pre, &report(vec![(&fs_, O::Landed)], false));
        assert!(st.delivered.is_empty());
        assert_eq!(st.retry, vec![fs_.clone()]);
        assert!(auto_delete_targets(&st, false, true).is_empty());
        // Vanished before sending → just dropped from the queue.
        std::fs::remove_file(&f).unwrap();
        let st = settle_batch(&[fs_.clone()], &pre, &report(vec![], false));
        assert_eq!(st.gone, vec![fs_]);
    }

    /// D2: the idle rescan never queues a file that's still being written.
    #[test]
    fn seed_existing_skips_a_file_still_being_written() {
        let dir = temp_dir("d2-seed");
        let folder = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("done.txt"), b"complete").unwrap();
        let growing = dir.join("copying.bin");
        std::fs::write(&growing, b"x").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (g, st) = (growing.clone(), stop.clone());
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            while !st.load(Ordering::SeqCst) {
                let mut f = std::fs::OpenOptions::new().append(true).open(&g).unwrap();
                f.write_all(&[0u8; 4096]).unwrap();
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let queue = Arc::new(Mutex::new(SendQueue::default()));
        let wake = Arc::new(Notify::new());
        seed_existing_with(&folder, &inbound, &queue, &wake, Duration::from_millis(400));
        stop.store(true, Ordering::SeqCst);
        writer.join().unwrap();
        let queued: Vec<String> = queue.lock().unwrap().iter().cloned().collect();
        assert!(queued.iter().any(|p| p.ends_with("done.txt")));
        assert!(!queued.iter().any(|p| p.ends_with("copying.bin")), "half-written file not queued");
    }

    /// D4: a delete only removes the VERSION the deleter had; a newer local edit
    /// (different size/mtime) survives it.
    #[test]
    fn delete_with_known_version_spares_a_newer_local_edit() {
        let dir = temp_dir("d4-known");
        let folder = dir.to_string_lossy().to_string();
        let f = dir.join("notes.txt");
        write_with_mtime(&f, b"v1", 1_000_000);
        let v1 = version_of(&f);
        let sd = Arc::new(Mutex::new(HashMap::new()));
        // We edited it after the peer deleted the old version.
        write_with_mtime(&f, b"v2 - edited here", 2_000_000);
        let mut ap = Vec::new();
        let g = DeleteGuard { ts: now_ms() + 60_000, known: Some(v1), listed: None, require_known: false };
        assert!(!apply_remote_delete(&folder, "notes.txt", &sd, &mut ap, 0, &g));
        assert!(f.is_file(), "newer edit kept");
        // The exact version the deleter had DOES go (archived, recoverable).
        let g = DeleteGuard { ts: now_ms(), known: Some(version_of(&f)), listed: None, require_known: false };
        assert!(apply_remote_delete(&folder, "notes.txt", &sd, &mut ap, 0, &g));
        assert!(!f.exists());
        assert_eq!(crate::folder_history::load(&folder).len(), 1);
    }

    /// A late/re-sent delete never undoes a restore of the identical version.
    #[test]
    fn late_known_delete_spares_a_restored_identical_file() {
        let dir = temp_dir("d4-restored");
        let folder = dir.to_string_lossy().to_string();
        let f = dir.join("restored.txt");
        write_with_mtime(&f, b"same bytes", 1_000); // placed (ctime) now
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        let g = DeleteGuard { ts: now_ms() - 3_600_000, known: Some(version_of(&f)), listed: None, require_known: true };
        assert!(!apply_remote_delete(&folder, "restored.txt", &sd, &mut ap, 0, &g));
        assert!(f.is_file());
    }

    /// D4: without a known version (older peers / reconcile tombstones), a delete
    /// older than the local copy's placement version is refused.
    #[test]
    fn late_delete_never_beats_a_newer_local_version() {
        let dir = temp_dir("d4-late");
        let folder = dir.to_string_lossy().to_string();
        let f = dir.join("late.txt");
        std::fs::write(&f, b"edited just now").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        let stale = DeleteGuard { ts: now_ms() - 3_600_000, known: None, listed: None, require_known: false };
        assert!(!apply_remote_delete(&folder, "late.txt", &sd, &mut ap, 0, &stale));
        assert!(f.is_file());
        let newer = DeleteGuard { ts: now_ms() + 60_000, known: None, listed: None, require_known: false };
        assert!(apply_remote_delete(&folder, "late.txt", &sd, &mut ap, 0, &newer));
    }

    /// D4: a versioned-delete peer that names no version for an entry can only
    /// remove an empty directory — never a file it never had.
    #[test]
    fn versioned_delete_without_a_version_never_removes_a_file() {
        let dir = temp_dir("d4-require-known");
        let folder = dir.to_string_lossy().to_string();
        write_with_mtime(&dir.join("theirs-never-saw.txt"), b"mine", 1_000);
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        let g = DeleteGuard { ts: now_ms() + 60_000, known: None, listed: None, require_known: true };
        assert!(!apply_remote_delete(&folder, "theirs-never-saw.txt", &sd, &mut ap, 0, &g));
        assert!(dir.join("theirs-never-saw.txt").is_file());
        std::fs::create_dir_all(dir.join("empty-dir")).unwrap();
        let listed: HashSet<String> = ["empty-dir".to_string()].into();
        let g = DeleteGuard { ts: now_ms() + 60_000, known: None, listed: Some(&listed), require_known: true };
        assert!(apply_remote_delete(&folder, "empty-dir", &sd, &mut ap, 0, &g));
        assert!(!dir.join("empty-dir").exists());
    }

    /// D4: a directory delete removes only what the deleter listed; a file it
    /// never saw keeps the directory (and itself) alive.
    #[test]
    fn directory_delete_removes_only_listed_children() {
        let dir = temp_dir("d4-dir");
        let folder = dir.to_string_lossy().to_string();
        write_with_mtime(&dir.join("trip/a.jpg"), b"a", 1_000);
        write_with_mtime(&dir.join("trip/mine-only.jpg"), b"never synced", 1_000);
        std::fs::write(dir.join("trip/.DS_Store"), b"junk").unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let listed: HashSet<String> = ["trip/a.jpg".to_string(), "trip".to_string()].into();
        let mut ap = Vec::new();
        let ga = DeleteGuard { ts: now_ms(), known: Some(version_of(&dir.join("trip/a.jpg"))), listed: Some(&listed), require_known: false };
        assert!(apply_remote_delete(&folder, "trip/a.jpg", &sd, &mut ap, 0, &ga));
        let gd = DeleteGuard { ts: now_ms() + 60_000, known: None, listed: Some(&listed), require_known: false };
        assert!(!apply_remote_delete(&folder, "trip", &sd, &mut ap, 0, &gd));
        assert!(dir.join("trip/mine-only.jpg").is_file(), "unlisted file survives");
        // Once only OS litter is left, the listed directory goes.
        std::fs::remove_file(dir.join("trip/mine-only.jpg")).unwrap();
        assert!(apply_remote_delete(&folder, "trip", &sd, &mut ap, 0, &gd));
        assert!(!dir.join("trip").exists(), ".DS_Store doesn't keep a deleted folder alive");
    }

    /// D5: files we knew that vanished while DropBeam was closed are found — but
    /// never when the whole folder vanished/emptied (unplugged drive), and never
    /// for paths already tombstoned.
    #[test]
    fn missed_deletes_found_but_never_for_a_missing_or_emptied_folder() {
        let dir = temp_dir("d5");
        let folder = dir.to_string_lossy().to_string();
        for n in ["a.txt", "b.txt", "c.txt", "d.txt", "sub/e.txt"] {
            write_with_mtime(&dir.join(n), n.as_bytes(), 1_000);
        }
        let inbound: HashSet<String> = ["a.txt", "b.txt", "c.txt", "d.txt", "sub/e.txt"]
            .iter()
            .map(|n| file_sig(&dir.join(n).to_string_lossy(), &folder).unwrap())
            .collect();
        std::fs::remove_file(dir.join("b.txt")).unwrap();
        std::fs::remove_file(dir.join("sub/e.txt")).unwrap();
        let mut tombs = HashMap::new();
        let found = missed_deletes(&folder, &inbound, &tombs);
        let mut keys: Vec<_> = found.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["b.txt", "sub/e.txt"]);
        assert_eq!(found["b.txt"], (5, 1_000), "carries the version we had");
        tombs.insert("b.txt".to_string(), 5);
        assert_eq!(missed_deletes(&folder, &inbound, &tombs).len(), 1, "already tombstoned");
        // A sub-folder that's still there but EMPTY (an unmounted sub-volume)
        // while we knew several files in it: not a delete.
        for n in ["mnt/x.txt", "mnt/y.txt"] {
            write_with_mtime(&dir.join(n), n.as_bytes(), 1_000);
        }
        let mut inbound2 = inbound.clone();
        for n in ["mnt/x.txt", "mnt/y.txt"] {
            inbound2.insert(file_sig(&dir.join(n).to_string_lossy(), &folder).unwrap());
        }
        std::fs::remove_file(dir.join("mnt/x.txt")).unwrap();
        std::fs::remove_file(dir.join("mnt/y.txt")).unwrap();
        assert!(!missed_deletes(&folder, &inbound2, &tombs).contains_key("mnt/x.txt"));
        // Everything gone at once = drive problem, not a delete.
        for n in ["a.txt", "c.txt", "d.txt"] {
            std::fs::remove_file(dir.join(n)).unwrap();
        }
        assert!(missed_deletes(&folder, &inbound, &HashMap::new()).is_empty());
        // Root missing entirely.
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(missed_deletes(&folder, &inbound, &HashMap::new()).is_empty());
    }

    /// D6: a receive into a folder whose drive is unplugged lands nothing, never
    /// recreates the path, and reports every item Failed (so the sender retries).
    #[test]
    fn receive_into_missing_folder_fails_and_never_recreates_it() {
        let dir = temp_dir("d6");
        let staging = dir.join("staging");
        write_with_mtime(&staging.join("x/photo.jpg"), b"img", 1_000);
        let folder = dir.join("Volumes/Unplugged/Shared");
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let r = move_staged_into_folder(&staging, &folder.to_string_lossy(), &inbound, true, false, &sd, &LandCtx::default());
        assert!(r.moved.is_empty());
        assert_eq!(r.outcomes.get("x/photo.jpg"), Some(&crate::iroh_net::FolderItemOutcome::Failed));
        assert!(!folder.exists() && !dir.join("Volumes").exists(), "missing root not recreated");
        let mut ap = Vec::new();
        assert!(!apply_remote_delete(&folder.to_string_lossy(), "x/photo.jpg", &sd, &mut ap, 0, &DeleteGuard::any()));
    }

    /// D1 receiver side: a replace whose archive fails keeps the local file and
    /// reports Failed — never removes the only copy.
    #[cfg(unix)]
    #[test]
    fn replace_never_removes_local_copy_when_archive_fails() {
        let dir = temp_dir("d1-archive-fail");
        let folder = dir.join("f");
        write_with_mtime(&folder.join("doc.txt"), b"local only copy", 1_000);
        // Unreadable history index → archive refuses.
        std::fs::create_dir_all(folder.join(".dropbeam-history/index.json")).unwrap();
        let staging = dir.join("staging");
        write_with_mtime(&staging.join("doc.txt"), b"incoming", 2_000);
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let r = move_staged_into_folder(&staging, &folder.to_string_lossy(), &inbound, true, false, &sd, &LandCtx::default());
        assert_eq!(r.outcomes.get("doc.txt"), Some(&crate::iroh_net::FolderItemOutcome::Failed));
        assert_eq!(std::fs::read(folder.join("doc.txt")).unwrap(), b"local only copy");
    }

    /// Tombstone-TTL backstop: a copy of a version we deleted, made BEFORE we
    /// deleted it (a device offline > 45 days), isn't resurrected; a fresh re-add
    /// of identical bytes is.
    #[test]
    fn stale_copy_of_a_deleted_version_is_not_resurrected() {
        let dir = temp_dir("ttl");
        let folder = dir.join("f");
        std::fs::create_dir_all(&folder).unwrap();
        let staging = dir.join("staging");
        write_with_mtime(&staging.join("old.txt"), b"old", 1_000);
        let sig = "old.txt|3|1000".to_string();
        let deleted_at = 5_000_000u64;
        let ctx = |ver: u64| LandCtx {
            vers: [("old.txt".to_string(), ver)].into(),
            deleted_at: Some(Box::new({
                let sig = sig.clone();
                move |s: &str| (s == sig).then_some(deleted_at)
            })),
        };
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let r = move_staged_into_folder(&staging, &folder.to_string_lossy(), &inbound, true, true, &sd, &ctx(1_000_000));
        assert_eq!(r.outcomes.get("old.txt"), Some(&crate::iroh_net::FolderItemOutcome::Kept("deleted".into())));
        assert!(!folder.join("old.txt").exists());
        write_with_mtime(&staging.join("old.txt"), b"old", 1_000);
        let r = move_staged_into_folder(&staging, &folder.to_string_lossy(), &inbound, true, true, &sd, &ctx(9_000_000));
        assert_eq!(r.outcomes.get("old.txt"), Some(&crate::iroh_net::FolderItemOutcome::Landed));
        assert!(folder.join("old.txt").is_file(), "a re-add after the delete lands");
    }

    /// D7: on a case-insensitive volume a peer file that differs only in letter
    /// case is the SAME file — never "missing", so a case-only rename can't loop.
    #[test]
    fn case_only_difference_is_not_missing_on_case_insensitive_volume() {
        let mine: HashMap<String, FileEntry> = [("Photo.jpg".to_string(), fe(10, 5))].into();
        let peer: HashMap<String, (u64, u64)> = [("photo.jpg".to_string(), (10, 5))].into();
        let none = HashMap::new();
        assert_eq!(reconcile_plan_ci(&mine, &peer, &none, &none, TEST_NOW, false).push, vec!["Photo.jpg"]);
        assert!(reconcile_plan_ci(&mine, &peer, &none, &none, TEST_NOW, true).push.is_empty());
        // Two peer files folding to one name (a case-sensitive peer) → ambiguous:
        // fall back to exact-name behaviour.
        let peer2: HashMap<String, (u64, u64)> =
            [("photo.jpg".to_string(), (10, 5)), ("PHOTO.jpg".to_string(), (11, 5))].into();
        assert_eq!(reconcile_plan_ci(&mine, &peer2, &none, &none, TEST_NOW, true).push, vec!["Photo.jpg"]);
    }

    /// D7: on a case-insensitive volume, a case-only rename from the peer renames
    /// in place (never archives the only copy), a delete of the OLD spelling never
    /// removes the renamed file, and a newer incoming spelling is adopted.
    #[test]
    fn case_only_rename_is_a_rename_never_a_delete() {
        let dir = temp_dir("d7");
        let folder = dir.to_string_lossy().to_string();
        if !folder_case_insensitive(&folder) {
            return; // only meaningful on APFS/NTFS-style volumes
        }
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let inbound = Arc::new(Mutex::new(HashSet::new()));
        write_with_mtime(&dir.join("photo.jpg"), b"bytes", 1_000);
        // Peer renamed photo.jpg → Photo.jpg.
        assert!(apply_remote_move(&folder, "photo.jpg", "Photo.jpg", 5, 1_000, &sd, &inbound, 0));
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"Photo.jpg".to_string()), "{names:?}");
        assert!(crate::folder_history::load(&folder).is_empty(), "nothing archived");
        // The backstop delete of the old spelling must not touch the renamed file.
        let mut ap = Vec::new();
        let g = DeleteGuard { ts: now_ms() + 60_000, known: Some((5, 1_000)), listed: None, require_known: false };
        assert!(!apply_remote_delete(&folder, "photo.jpg", &sd, &mut ap, 0, &g));
        assert!(dir.join("Photo.jpg").is_file());
        // Receiving the old spelling from an older-version sender keeps ours…
        let staging = dir.join("..").join(format!("stg-{}", std::process::id()));
        write_with_mtime(&staging.join("photo.jpg"), b"bytes", 1_000);
        let ctx = LandCtx { vers: [("photo.jpg".to_string(), 1)].into(), deleted_at: None };
        let r = move_staged_into_folder(&staging, &folder, &inbound, true, false, &sd, &ctx);
        assert!(matches!(r.outcomes.get("photo.jpg"), Some(crate::iroh_net::FolderItemOutcome::Kept(_))));
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"Photo.jpg".to_string()));
        // …but a NEWER spelling is adopted (renamed in place, same bytes).
        write_with_mtime(&staging.join("PHOTO.jpg"), b"bytes", 1_000);
        let ctx = LandCtx { vers: [("PHOTO.jpg".to_string(), now_ms() + 60_000)].into(), deleted_at: None };
        let r = move_staged_into_folder(&staging, &folder, &inbound, true, false, &sd, &ctx);
        assert_eq!(r.outcomes.get("PHOTO.jpg"), Some(&crate::iroh_net::FolderItemOutcome::Landed));
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('.')).collect();
        assert_eq!(names, vec!["PHOTO.jpg".to_string()]);
        let _ = std::fs::remove_dir_all(staging);
    }

    /// P3-e: a versioned peer's tombstone without a version never removes a file;
    /// with one, only that version.
    #[test]
    fn reconcile_tombstone_deletes_only_a_named_version() {
        let dir = temp_dir("p3e");
        let folder = dir.to_string_lossy().to_string();
        write_with_mtime(&dir.join("mine.txt"), b"independent", 1_000);
        let mut rec = Reconcile { tomb_versions: Some(HashMap::new()), ..Default::default() };
        assert!(reconcile_delete_guard(&rec, "mine.txt", now_ms() + 60_000).is_none());
        rec.tomb_versions = Some([("mine.txt".to_string(), (999, 1))].into());
        let g = reconcile_delete_guard(&rec, "mine.txt", now_ms() + 60_000).unwrap();
        let sd = Arc::new(Mutex::new(HashMap::new()));
        let mut ap = Vec::new();
        assert!(!apply_remote_delete(&folder, "mine.txt", &sd, &mut ap, 0, &g), "different version kept");
        rec.tomb_versions = None;
        assert!(reconcile_delete_guard(&rec, "mine.txt", 5).is_some_and(|g| !g.require_known), "legacy peers: ts rule");
        assert!(folder_available(&folder));
        assert!(!folder_available(&dir.join("nope").to_string_lossy()));
    }

    #[test]
    fn pause_epoch_beats_the_last_toggle_and_ignores_poisoned_values() {
        assert_eq!(next_pause_epoch(1_000, 0), 1_000);
        assert_eq!(next_pause_epoch(1_000, 5_000), 5_001, "clock behind the last toggle");
        assert_eq!(next_pause_epoch(1_000, u64::MAX), 1_000, "poisoned epoch reset");
        assert_eq!(sane_epoch(u64::MAX), 0);
    }

    #[test]
    fn peer_kept_warning_names_the_problem() {
        let mut m = HashMap::new();
        assert!(skipped_warning(&m, "Ethan").is_none());
        m.insert("sub/a:b.txt|3|9".to_string(), "name".to_string());
        m.insert("x.txt|1|1".to_string(), "conflict".to_string());
        let w = skipped_warning(&m, "Ethan").unwrap();
        assert!(w.contains("1 file") && w.contains("Ethan") && w.contains("a:b.txt"), "{w}");
    }

    /// P3: a viewer promoted to editor doesn't push what it changed while
    /// read-only — those files are baselined until edited again.
    #[test]
    fn promotion_baselines_existing_files() {
        let dir = temp_dir("promote");
        let cfg = temp_dir("promote-cfg");
        std::fs::write(dir.join("viewer-edit.txt"), b"local change").unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();
        // A file it knew and deleted while read-only.
        save_manifest(&cfg, "p1", &["gone.txt|3|1000".to_string()].into());
        let pair = Pair {
            id: "p1".into(), role: crate::models::PairRole::B, peer_name: String::new(), secret: "s".into(),
            folder: dir.to_string_lossy().to_string(), two_way: true, mirror: true, auto_delete: false,
            delete_mode: DeleteMode::Trash, created_at: 0, endpoint_id: None, group_id: None,
            i_am_viewer: false, peer_is_viewer: false, owner_eid: None, role_epoch: 0, paused: false, pause_epoch: 0,
        };
        assert_eq!(baseline_on_promotion(&cfg, &pair), 1);
        let inbound = Arc::new(Mutex::new(load_manifest(&cfg, "p1")));
        assert!(!is_sendable_candidate(&dir.join("viewer-edit.txt").to_string_lossy(), &pair.folder, &inbound));
        let sig = file_sig(&dir.join("viewer-edit.txt").to_string_lossy(), &pair.folder).unwrap();
        assert!(load_skipped(&cfg, "p1").contains_key(&sig), "reconcile won't push it either");
        assert!(!load_manifest(&cfg, "p1").contains("gone.txt|3|1000"), "read-only-era delete not propagated");
        let tombs = HashMap::new();
        assert!(missed_deletes(&pair.folder, &load_manifest(&cfg, "p1"), &tombs).is_empty());
    }

    #[test]
    fn live_receive_placeholder_is_never_swept() {
        let dir = temp_dir("placeholder");
        let ph = dir.join("big.mov.dropbeam-incoming");
        std::fs::write(&ph, b"").unwrap();
        // Make it look old (a long receive).
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::OpenOptions::new().write(true).open(&ph).unwrap().set_modified(old).unwrap();
        placeholder_active(&ph, true);
        sweep_stale_placeholder(&ph);
        assert!(ph.exists(), "active receive keeps its placeholder");
        placeholder_active(&ph, false);
        sweep_stale_placeholder(&ph);
        assert!(!ph.exists(), "orphan swept once the receive is gone");
    }

    /// D15: where there's no ctime, a file first seen NOW reads as new even if its
    /// own dates are old (a moved-in file), and a rewrite resets it.
    #[test]
    fn first_seen_marks_a_moved_in_old_file_as_new() {
        let p = PathBuf::from("/nonexistent/first-seen-test.bin");
        // A scan/lookup never records anything (P1-a: no "everything is new at launch").
        assert_eq!(appeared_at(&p, 10, 100), None);
        note_appeared(&p, 10, 100, 5_000);
        assert_eq!(appeared_at(&p, 10, 100), Some(5_000));
        note_appeared(&p, 10, 100, 9_000);
        assert_eq!(appeared_at(&p, 10, 100), Some(5_000), "stable while unchanged");
        assert_eq!(appeared_at(&p, 11, 100), None, "a different version there isn't covered");
        note_appeared(&p, 11, 100, 9_000);
        assert_eq!(appeared_at(&p, 11, 100), Some(9_000));
    }
}
