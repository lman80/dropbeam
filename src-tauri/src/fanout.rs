//! Files to a friend who has several devices (a Mac AND an iPhone) go to EVERY
//! one of them — not just whichever answers first.
//!
//! One send = one card (`Record`), with where the files are on each device
//! (`Delivery`). Each online device gets them directly, all at once, through
//! the normal resumable friend send (one "leg" per device, see
//! `iroh_net::send_to_friend_opts`). Devices that are offline get ONE
//! Transfer Server copy sealed for all of them together (the server keeps it
//! until each has taken it); with no server they wait here — durably, across
//! restarts — and get it directly the moment they show up.
//!
//! The engine is independent of the app window (`Env`), so the loopback tests
//! drive the real scheduling, the real server and real transfers without Tauri.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::iroh_net::{CancelReason, IrohState};
use crate::models::{Direction, TransferState, TransferUpdate};

/// A leg's "this device didn't answer" — not a failure (it gets held or queued).
pub const DEVICE_OFFLINE: &str = "dropbeam:device-offline";

pub const SENDING: &str = "sending";
pub const UPLOADING: &str = "uploading";
/// Didn't answer the first dial; about to be held or queued.
pub const OFFLINE: &str = "offline";
pub const DELIVERED: &str = "delivered";
pub const HELD: &str = "held";
pub const WAITING: &str = "waiting";
pub const FAILED: &str = "failed";
pub const DECLINED: &str = "declined";
pub const CANCELED: &str = "canceled";
pub const PAUSED: &str = "paused";

/// Give up on a device after this many real failures (offline never counts).
const MAX_ATTEMPTS: u32 = 3;
/// A device still waiting after this long is given up on.
const WAIT_MAX_MS: u64 = 30 * 24 * 3600 * 1000;
/// Finished sends are forgotten after this long.
const KEEP_MS: u64 = 14 * 24 * 3600 * 1000;

/// One device's leg of a fan-out send (passed to the friend send engine).
#[derive(Debug, Clone)]
pub struct ChildCtx {
    /// The leg's own transfer id (its updates are routed to the send's card).
    pub id: String,
    /// How long the first dial may take before the device counts as offline.
    pub first_dial: Duration,
}

/// Where a send is on one of the friend's devices.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub eid: String,
    /// What the device is ("Mac", "iPhone", "Mac 2").
    pub label: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    /// sending | uploading | offline | delivered | held | waiting | failed | declined | canceled | paused
    pub state: String,
    /// The Transfer Server holding (or receiving) it for this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// A short reason ("Linux Box is full", "files_gone", …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_try_ms: u64,
}

impl Delivery {
    fn busy(&self) -> bool {
        matches!(self.state.as_str(), SENDING | UPLOADING | OFFLINE)
    }
    fn settled(&self) -> bool {
        !self.busy() && self.state != WAITING && self.state != HELD
    }
}

/// One send to a person with several devices (persisted in `fanout.json`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// The send's card id.
    pub id: String,
    /// The chat card's transfer link id (what the recipients link it by).
    pub chat_id: String,
    /// Our thread id for the person.
    pub peer_id: String,
    pub friend_name: String,
    pub paths: Vec<String>,
    pub names: Vec<String>,
    pub total: u64,
    pub created_ms: u64,
    /// Chat link generation base; `+ gen` on every visible change.
    pub attempt: u64,
    #[serde(default)]
    pub gen: u64,
    /// The one History row for the send was written.
    #[serde(default)]
    pub logged: bool,
    #[serde(default)]
    pub updated_ms: u64,
    pub devices: Vec<Delivery>,
}

fn now() -> u64 {
    crate::chat::now_ms()
}

// ── store ───────────────────────────────────────────────────────────────────

static LOCK: Mutex<()> = Mutex::new(());
/// Devices with something waiting for them (so presence can wake the queue).
static WAITING_EIDS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn store_path(config: &Path) -> PathBuf {
    config.join("fanout.json")
}

fn load(config: &Path) -> BTreeMap<String, Record> {
    match crate::settings::read_json_store(&store_path(config)) {
        crate::settings::StoreRead::Loaded(v) => v,
        _ => BTreeMap::new(),
    }
}

fn save(config: &Path, map: &BTreeMap<String, Record>) {
    match serde_json::to_vec(map) {
        Ok(bytes) => {
            if let Err(e) = crate::settings::write_atomic(&store_path(config), &bytes) {
                log::warn!("fanout: cannot save: {e}");
            }
        }
        Err(e) => log::warn!("fanout: cannot serialize: {e}"),
    }
    let waiting: HashSet<String> = map.values().flat_map(|r| r.devices.iter())
        .filter(|d| d.state == WAITING).map(|d| d.eid.clone()).collect();
    *WAITING_EIDS.lock().unwrap_or_else(|p| p.into_inner()) = Some(waiting);
}

pub fn records(config: &Path) -> Vec<Record> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load(config).into_values().collect()
}

pub fn get(config: &Path, id: &str) -> Option<Record> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load(config).remove(id)
}

fn put(config: &Path, rec: Record) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map = load(config);
    let t = now();
    map.retain(|_, r| !(r.devices.iter().all(Delivery::settled) && r.updated_ms.max(r.created_ms) + KEEP_MS < t));
    map.insert(rec.id.clone(), rec);
    save(config, &map);
}

/// Change one record; returns it after the change (None = no such record).
fn update(config: &Path, id: &str, f: impl FnOnce(&mut Record)) -> Option<Record> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map = load(config);
    let rec = map.get_mut(id)?;
    let before = serde_json::to_vec(&*rec).ok();
    f(rec);
    if serde_json::to_vec(&*rec).ok() != before {
        rec.updated_ms = now();
        let out = rec.clone();
        save(config, &map);
        return Some(out);
    }
    Some(rec.clone())
}

fn set_device(config: &Path, id: &str, eid: &str, f: impl FnOnce(&mut Delivery)) -> Option<Record> {
    update(config, id, |r| {
        if let Some(d) = r.devices.iter_mut().find(|d| d.eid == eid) {
            f(d);
        }
    })
}

/// Where the send with chat link `chat_id` is on each device (for its chat card).
pub fn deliveries_for(config: &Path, chat_id: &str) -> Vec<Delivery> {
    records(config).into_iter().filter(|r| r.chat_id == chat_id)
        .max_by_key(|r| r.created_ms).map(|r| r.devices).unwrap_or_default()
}

// ── who gets it ─────────────────────────────────────────────────────────────

/// What a device is, in the words Apple uses (mirrors the UI's `deviceNoun`).
pub fn device_noun(kind: Option<&str>, os: Option<&str>) -> String {
    match (os, kind) {
        (Some("ios"), Some("tablet")) => "iPad",
        (Some("ios"), _) => "iPhone",
        (Some("macos"), _) => "Mac",
        (Some("windows"), _) => "PC",
        (Some("linux"), _) => "Linux PC",
        (Some("android"), Some("tablet")) => "Tablet",
        (Some("android"), _) => "Phone",
        (_, Some("phone")) => "Phone",
        (_, Some("tablet")) => "Tablet",
        _ => "Computer",
    }
    .to_owned()
}

/// "Mac" + "iPhone", or "Mac 1" / "Mac 2" when two would read the same.
fn label_devices(devices: &mut [Delivery]) {
    let nouns: Vec<String> = devices.iter().map(|d| device_noun(d.kind.as_deref(), d.os.as_deref())).collect();
    let mut order: Vec<usize> = (0..devices.len()).collect();
    order.sort_by(|a, b| devices[*a].eid.cmp(&devices[*b].eid));
    for i in 0..devices.len() {
        let same: Vec<usize> = order.iter().copied().filter(|j| nouns[*j] == nouns[i]).collect();
        devices[i].label = if same.len() > 1 {
            format!("{} {}", nouns[i], same.iter().position(|j| *j == i).unwrap_or(0) + 1)
        } else {
            nouns[i].clone()
        };
    }
}

/// The person owning thread `friend_id` and every device of theirs a send goes
/// to (or just `only`, one of them). Never our own devices or a blocked one.
pub fn targets(config: &Path, friend_id: &str, me: &str, only: Option<&str>) -> Result<(crate::models::Friend, Vec<Delivery>), String> {
    let owner = crate::friends::thread_owner(config, friend_id).ok_or("Friend not found.")?;
    let all = crate::friends::load_raw(config);
    let mut eids: Vec<String> = Vec::new();
    for e in crate::friends::person_endpoints(config, &owner.id) {
        if e != me && !eids.contains(&e) && !crate::block::is_blocked(config, &e) && !crate::account::is_own_device(config, &e) {
            eids.push(e);
        }
    }
    if let Some(only) = only {
        if !eids.iter().any(|e| e == only) {
            return Err(format!("That device isn't one of {}'s any more.", owner.name));
        }
        eids = vec![only.to_owned()];
    }
    let mut devices: Vec<Delivery> = eids.into_iter().map(|eid| {
        let f = all.iter().find(|f| f.endpoint_id.as_deref() == Some(eid.as_str()));
        Delivery {
            kind: f.and_then(|f| f.device_kind.clone()),
            os: f.and_then(|f| f.device_os.clone()),
            eid, state: SENDING.into(), ..Default::default()
        }
    }).collect();
    label_devices(&mut devices);
    Ok((owner, devices))
}

// ── what the card and chat bubble show ──────────────────────────────────────

/// The card's overall state from the devices' states.
pub fn overall(devices: &[Delivery], any_bytes: bool) -> TransferState {
    let n = devices.len();
    let count = |s: &str| devices.iter().filter(|d| d.state == s).count();
    if devices.iter().any(Delivery::busy) {
        return if any_bytes || count(UPLOADING) > 0 { TransferState::Transferring } else { TransferState::Connecting };
    }
    if n == 0 || count(CANCELED) == n {
        return TransferState::Canceled;
    }
    if count(PAUSED) > 0 {
        return TransferState::Paused;
    }
    if count(DELIVERED) > 0 {
        return TransferState::Completed;
    }
    if count(HELD) + count(WAITING) > 0 {
        return TransferState::Held;
    }
    TransferState::Failed
}

/// The send's card, from its record and the legs' live byte counts.
pub fn card(rec: &Record, live: &HashMap<String, (u64, f64)>) -> TransferUpdate {
    let mut u = TransferUpdate::new(rec.id.clone(), Direction::Send, rec.names.clone());
    u.friend_name = Some(rec.friend_name.clone());
    u.bytes_total = rec.total;
    let busy: Vec<(u64, f64)> = rec.devices.iter().filter(|d| d.busy())
        .map(|d| live.get(&d.eid).copied().unwrap_or((0, 0.0))).collect();
    let state = overall(&rec.devices, busy.iter().any(|b| b.0 > 0));
    if let Some(slowest) = busy.iter().copied().min_by_key(|b| b.0) {
        // The card follows the device furthest behind (an honest ETA for "all").
        u.bytes_done = slowest.0.min(rec.total);
        u.speed_bps = slowest.1;
    } else if matches!(state, TransferState::Completed | TransferState::Held) {
        u.bytes_done = rec.total;
    }
    u.percent = if rec.total > 0 { u.bytes_done as f64 * 100.0 / rec.total as f64 } else if u.bytes_done == rec.total && state == TransferState::Completed { 100.0 } else { 0.0 };
    u.eta_seconds = (u.speed_bps > 0.0).then(|| rec.total.saturating_sub(u.bytes_done) as f64 / u.speed_bps);
    u.state = state;
    u.held_on = rec.devices.iter().find(|d| d.state == HELD).and_then(|d| d.via.clone());
    if state == TransferState::Failed {
        u.error = Some(if rec.devices.iter().all(|d| d.state == DECLINED) {
            format!("{} declined", rec.friend_name)
        } else {
            rec.devices.iter().find_map(|d| d.note.clone().filter(|n| !n.contains('_')))
                .unwrap_or_else(|| format!("Couldn't deliver to {}'s devices", rec.friend_name))
        });
    }
    u.deliveries = Some(rec.devices.clone());
    u.chat_transfer = Some(crate::models::ChatTransferLink {
        id: rec.chat_id.clone(), attempt: rec.attempt.saturating_add(rec.gen), manifest: vec![], directories: vec![],
        batch_state: Some(state), bytes_done: u.bytes_done, completed_files: vec![], completed_paths: Default::default(),
        item_offset: 0, offset: 0, total: rec.total, last: true,
    });
    u
}

// ── the engine ──────────────────────────────────────────────────────────────

pub(crate) type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
/// (bytes landed on the device, bytes/s)
pub(crate) type Progress = Arc<dyn Fn(u64, f64) + Send + Sync>;

/// One device's direct leg.
pub(crate) struct Job {
    pub record: Record,
    pub eid: String,
    pub first_dial: Duration,
    pub cancel: Arc<AtomicBool>,
    /// A later try for a device that was offline/failed (not the first send).
    pub retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    Offline,
    Declined,
    Canceled,
    Paused,
    Failed(String),
}

/// What the engine needs from its surroundings (the app, or a test).
pub(crate) trait Env: Send + Sync {
    /// Send the record's files straight to one device.
    fn direct(&self, job: Job, progress: Progress) -> BoxFut<Outcome>;
    /// The card/bubble changed (`significant` = not just a progress tick).
    fn changed(&self, config: &Path, rec: &Record, live: &HashMap<String, (u64, f64)>, significant: bool);
    /// Stop every running leg of send `id`.
    fn stop_legs(&self, id: &str, reason: CancelReason);
}

pub(crate) struct Engine {
    pub env: Arc<dyn Env>,
    pub net: Arc<IrohState>,
    pub config: PathBuf,
}

/// Legs running right now, per send.
struct Live {
    cancel: Arc<AtomicBool>,
    reason: Option<CancelReason>,
    running: HashSet<String>,
    bytes: HashMap<String, (u64, f64)>,
    last_emit: Option<Instant>,
}

static LIVE: Mutex<Option<HashMap<String, Live>>> = Mutex::new(None);

fn with_live<T>(f: impl FnOnce(&mut HashMap<String, Live>) -> T) -> T {
    let mut g = LIVE.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn spawn<F: Future<Output = ()> + Send + 'static>(f: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(f);
        }
        Err(_) => {
            tauri::async_runtime::spawn(f);
        }
    }
}

fn wake_cell() -> &'static tokio::sync::Notify {
    static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
    WAKE.get_or_init(tokio::sync::Notify::new)
}

/// A device showed life: if something waits for it, try now.
pub fn device_seen(eid: &str) {
    let waiting = WAITING_EIDS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|w| w.contains(eid));
    if waiting {
        wake_cell().notify_one();
    }
}

impl Engine {
    fn me(&self) -> String {
        self.net.get().map(|e| e.id().to_string()).unwrap_or_default()
    }

    /// Start a new send (its record already lists every target device).
    pub(crate) fn begin(self: &Arc<Self>, mut rec: Record) {
        rec.created_ms = now();
        rec.updated_ms = rec.created_ms;
        let eids: Vec<String> = rec.devices.iter().map(|d| d.eid.clone()).collect();
        let id = rec.id.clone();
        put(&self.config, rec);
        self.start(id, eids, false);
    }

    /// Run the legs for `eids` of send `id` (a first send or a retry).
    fn start(self: &Arc<Self>, id: String, eids: Vec<String>, retry: bool) {
        let fresh = with_live(|l| {
            let live = l.entry(id.clone()).or_insert_with(|| Live {
                cancel: Arc::new(AtomicBool::new(false)), reason: None,
                running: HashSet::new(), bytes: HashMap::new(), last_emit: None,
            });
            // A retry after a stop starts clean.
            if live.running.is_empty() && live.cancel.load(Ordering::SeqCst) {
                live.cancel = Arc::new(AtomicBool::new(false));
                live.reason = None;
            }
            let fresh: Vec<String> = eids.iter().filter(|e| !live.running.contains(*e)).cloned().collect();
            live.running.extend(fresh.iter().cloned());
            fresh
        });
        if fresh.is_empty() {
            return;
        }
        let me = self.clone();
        spawn(async move { me.run(id, fresh, retry).await });
    }

    fn cancel_flag(&self, id: &str) -> Arc<AtomicBool> {
        with_live(|l| l.get(id).map(|x| x.cancel.clone())).unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    fn stop_state(id: &str) -> &'static str {
        match with_live(|l| l.get(id).and_then(|x| x.reason)) {
            Some(CancelReason::Pause) => PAUSED,
            _ => CANCELED,
        }
    }

    fn first_dial(&self, eids: &[String], retry: bool) -> Duration {
        // A server can hold it for a device that doesn't answer: don't wait long.
        if !crate::mailbox::client::holdable_devices(&self.config, &self.me(), eids).is_empty() {
            Duration::from_secs(6)
        } else if retry {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(15)
        }
    }

    async fn run(self: Arc<Self>, id: String, eids: Vec<String>, retry: bool) {
        let cancel = self.cancel_flag(&id);
        let first = self.first_dial(&eids, retry);
        let started = Instant::now();
        let settle = first + Duration::from_secs(3);
        let mut legs = tokio::task::JoinSet::new();
        for eid in &eids {
            legs.spawn(self.clone().leg(id.clone(), eid.clone(), first, cancel.clone(), retry));
        }
        // Devices that didn't answer are held/queued TOGETHER (one sealed copy
        // for all of them), once every device has had its first dial.
        let mut offline: Vec<String> = Vec::new();
        let mut holds = tokio::task::JoinSet::new();
        loop {
            if legs.is_empty() {
                if !offline.is_empty() {
                    holds.spawn(self.clone().hold_or_queue(id.clone(), std::mem::take(&mut offline), cancel.clone()));
                }
                break;
            }
            let wait = settle.saturating_sub(started.elapsed());
            tokio::select! {
                r = legs.join_next() => {
                    if let Some(Ok(Some(eid))) = r {
                        offline.push(eid);
                    }
                }
                _ = tokio::time::sleep(wait), if !offline.is_empty() => {
                    holds.spawn(self.clone().hold_or_queue(id.clone(), std::mem::take(&mut offline), cancel.clone()));
                }
            }
        }
        while holds.join_next().await.is_some() {}
        with_live(|l| {
            if let Some(live) = l.get_mut(&id) {
                for e in &eids {
                    live.running.remove(e);
                    live.bytes.remove(e);
                }
                if live.running.is_empty() {
                    l.remove(&id);
                }
            }
        });
        self.emit(&id, true);
    }

    /// One device, directly. Returns the device when it didn't answer.
    async fn leg(self: Arc<Self>, id: String, eid: String, first: Duration, cancel: Arc<AtomicBool>, retry: bool) -> Option<String> {
        if cancel.load(Ordering::SeqCst) {
            let stop = Self::stop_state(&id);
            set_device(&self.config, &id, &eid, |d| d.state = stop.into());
            return None;
        }
        let rec = set_device(&self.config, &id, &eid, |d| {
            d.state = SENDING.into();
            d.last_try_ms = now();
            d.via = None;
            d.item_id = None;
        })?;
        self.emit(&id, true);
        let progress: Progress = {
            let me = self.clone();
            let (id, eid) = (id.clone(), eid.clone());
            Arc::new(move |bytes, speed| me.progress(&id, &[eid.clone()], bytes, speed))
        };
        let outcome = self.env.direct(Job { record: rec, eid: eid.clone(), first_dial: first, cancel: cancel.clone(), retry }, progress).await;
        let stopped = cancel.load(Ordering::SeqCst);
        let stop = Self::stop_state(&id);
        with_live(|l| {
            if let Some(live) = l.get_mut(&id) {
                live.bytes.remove(&eid);
            }
        });
        let mut offline = false;
        set_device(&self.config, &id, &eid, |d| {
            match &outcome {
                Outcome::Delivered => {
                    d.state = DELIVERED.into();
                    d.note = None;
                }
                _ if stopped => d.state = stop.into(),
                Outcome::Offline => {
                    d.state = OFFLINE.into();
                    offline = true;
                }
                Outcome::Declined => d.state = DECLINED.into(),
                Outcome::Canceled => d.state = CANCELED.into(),
                Outcome::Paused => d.state = PAUSED.into(),
                Outcome::Failed(e) => {
                    d.attempts += 1;
                    d.note = Some(e.chars().take(200).collect());
                    // Try again later (it resumes); a device that keeps failing stops.
                    d.state = if d.attempts >= MAX_ATTEMPTS { FAILED } else { WAITING }.into();
                }
            }
        });
        if !matches!(outcome, Outcome::Delivered | Outcome::Offline) {
            log::info!("fanout: a device's leg ended ({outcome:?})");
        }
        self.emit(&id, true);
        offline.then_some(eid)
    }

    /// Devices that didn't answer: one Transfer Server copy sealed for all of
    /// them if a server can take it, else they wait here for their turn.
    async fn hold_or_queue(self: Arc<Self>, id: String, eids: Vec<String>, cancel: Arc<AtomicBool>) {
        let config = self.config.clone();
        let wait = |eids: &[String], note: Option<String>| {
            update(&config, &id, |r| {
                for d in r.devices.iter_mut().filter(|d| eids.contains(&d.eid)) {
                    d.state = WAITING.into();
                    d.via = None;
                    d.note = note.clone();
                }
            });
        };
        if cancel.load(Ordering::SeqCst) {
            let stop = Self::stop_state(&id);
            update(&config, &id, |r| {
                for d in r.devices.iter_mut().filter(|d| eids.contains(&d.eid)) {
                    d.state = stop.into();
                }
            });
            self.emit(&id, true);
            return;
        }
        let holdable = crate::mailbox::client::holdable_devices(&config, &self.me(), &eids);
        let rest: Vec<String> = eids.iter().filter(|e| !holdable.contains(e)).cloned().collect();
        if !rest.is_empty() {
            wait(&rest, None);
        }
        let Some(rec) = get(&config, &id) else { return };
        if holdable.is_empty() {
            self.emit(&id, true);
            return;
        }
        let inputs = crate::iroh_net::deposit_inputs(&rec.paths);
        let Ok((files, dirs, top)) = inputs else {
            wait(&holdable, None);
            self.emit(&id, true);
            return;
        };
        update(&config, &id, |r| {
            for d in r.devices.iter_mut().filter(|d| holdable.contains(&d.eid)) {
                d.state = UPLOADING.into();
                d.via = None;
                d.note = None;
            }
        });
        self.emit(&id, true);
        let progress = {
            let (me, id, eids) = (self.clone(), id.clone(), holdable.clone());
            move |done: u64, _all: u64| me.progress(&id, &eids, done, 0.0)
        };
        let on_start = {
            let (config, id, eids) = (config.clone(), id.clone(), holdable.clone());
            move |server: &str| {
                update(&config, &id, |r| {
                    for d in r.devices.iter_mut().filter(|d| eids.contains(&d.eid)) {
                        d.via = Some(server.to_owned());
                    }
                });
            }
        };
        let result = crate::mailbox::client::deposit_files(&self.net, &config, &rec.peer_id, &rec.chat_id, &rec.id,
            &files, &dirs, &top, &progress, &on_start, &cancel, Some(&holdable)).await;
        use crate::mailbox::client::DepositError;
        match result {
            Ok(held) => {
                log::info!("fanout: held on a Transfer Server for {} offline device(s)", holdable.len());
                update(&config, &id, |r| {
                    for d in r.devices.iter_mut().filter(|d| holdable.contains(&d.eid)) {
                        d.state = HELD.into();
                        d.via = Some(held.name.clone());
                        d.item_id = Some(held.item_id.clone());
                        d.note = None;
                    }
                });
                crate::mailbox::client::wake();
            }
            Err(DepositError::GoDirect) => {
                // One of them just came online: straight to them after all.
                let mut again = tokio::task::JoinSet::new();
                for eid in &holdable {
                    again.spawn(self.clone().leg(id.clone(), eid.clone(), Duration::from_secs(15), cancel.clone(), true));
                }
                let mut still: Vec<String> = Vec::new();
                while let Some(r) = again.join_next().await {
                    if let Ok(Some(eid)) = r {
                        still.push(eid);
                    }
                }
                if !still.is_empty() {
                    wait(&still, None);
                }
            }
            Err(DepositError::Canceled) => {
                let stop = Self::stop_state(&id);
                update(&config, &id, |r| {
                    for d in r.devices.iter_mut().filter(|d| holdable.contains(&d.eid)) {
                        d.state = stop.into();
                    }
                });
            }
            Err(e @ DepositError::Refused { .. }) => wait(&holdable, Some(e.to_string())),
            Err(_) => wait(&holdable, None),
        }
        self.emit(&id, true);
    }

    fn progress(&self, id: &str, eids: &[String], bytes: u64, speed: f64) {
        let due = with_live(|l| {
            let Some(live) = l.get_mut(id) else { return false };
            for e in eids {
                live.bytes.insert(e.clone(), (bytes, speed));
            }
            let due = live.last_emit.is_none_or(|t| t.elapsed() >= Duration::from_millis(250));
            if due {
                live.last_emit = Some(Instant::now());
            }
            due
        });
        if due {
            self.emit(id, false);
        }
    }

    fn emit(&self, id: &str, significant: bool) {
        let rec = if significant {
            // Every visible change is a new generation of the chat link, so a card
            // that already showed "Delivered" can still move on (iPhone got it).
            update(&self.config, id, |r| r.gen += 1)
        } else {
            get(&self.config, id)
        };
        let Some(rec) = rec else { return };
        let live = with_live(|l| l.get(id).map(|x| x.bytes.clone())).unwrap_or_default();
        self.env.changed(&self.config, &rec, &live, significant);
    }

    /// Stop send `id` (a cancel or a pause). False = not a fan-out send.
    pub(crate) fn stop(&self, id: &str, reason: CancelReason) -> bool {
        let Some(rec) = get(&self.config, id) else { return false };
        let running = with_live(|l| {
            l.get_mut(id).map(|live| {
                live.reason = Some(reason);
                live.cancel.store(true, Ordering::SeqCst);
                live.running.clone()
            })
        }).unwrap_or_default();
        let stop = if reason == CancelReason::Pause { PAUSED } else { CANCELED };
        let had_held = rec.devices.iter().any(|d| d.state == HELD);
        update(&self.config, id, |r| {
            for d in r.devices.iter_mut().filter(|d| !running.contains(&d.eid)) {
                if d.state == WAITING || d.state == OFFLINE || (d.state == HELD && reason == CancelReason::Cancel) {
                    d.state = stop.into();
                }
            }
        });
        // A cancel takes the copies a server holds for them back off it too.
        if had_held && reason == CancelReason::Cancel {
            crate::mailbox::client::abandon(&self.net, &self.config, &rec.chat_id);
        }
        self.env.stop_legs(id, reason);
        self.emit(id, true);
        true
    }

    /// A Transfer Server receipt for one of our sends. True = it was ours.
    pub(crate) fn receipt(&self, r: &crate::mailbox::client::Receipt) -> bool {
        let item = r.sent.item_id.as_str();
        let owner = records(&self.config).into_iter()
            .find(|rec| rec.devices.iter().any(|d| d.item_id.as_deref() == Some(item)));
        let Some(rec) = owner else { return false };
        let mut retry = false;
        update(&self.config, &rec.id, |rec| {
            for d in rec.devices.iter_mut().filter(|d| d.item_id.as_deref() == Some(item) && d.state == HELD) {
                if r.delivered_to.contains(&d.eid) || (r.state == "delivered" && r.delivered_to.is_empty()) {
                    d.state = DELIVERED.into();
                    d.note = None;
                } else {
                    match r.state.as_str() {
                        // Finished without this device (it turned it down).
                        "delivered" => {
                            d.state = FAILED.into();
                            d.note = Some("refused".into());
                        }
                        "canceled" => d.state = CANCELED.into(),
                        // Never reached it through the server: we still have the
                        // files, so send them directly when the device is back.
                        "expired" | "lost" | "rejected" => {
                            d.attempts += 1;
                            d.state = if d.attempts >= MAX_ATTEMPTS { FAILED } else { WAITING }.into();
                            d.item_id = None;
                            d.via = None;
                            d.note = None;
                            d.last_try_ms = 0;
                            retry = true;
                        }
                        _ => {}
                    }
                }
            }
        });
        self.emit(&rec.id, true);
        if retry {
            wake_cell().notify_one();
        }
        true
    }

    /// After a restart: legs that were in flight are owed again, and every
    /// waiting device is due a try now.
    pub(crate) fn recover(&self) {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut map = load(&self.config);
        let running: HashSet<String> = with_live(|l| l.keys().cloned().collect());
        let mut changed = false;
        for rec in map.values_mut().filter(|r| !running.contains(&r.id)) {
            // Everything still owed gets one try right away after a launch.
            for d in rec.devices.iter_mut().filter(|d| d.busy() || d.state == WAITING) {
                d.state = WAITING.into();
                d.last_try_ms = 0;
                changed = true;
            }
        }
        // Saving also primes the presence index of waiting devices.
        let _ = changed;
        save(&self.config, &map);
    }

    /// Start a direct try for every waiting device that's due one.
    pub(crate) fn retry_due(self: &Arc<Self>) {
        let t = now();
        for rec in records(&self.config) {
            let running = with_live(|l| l.get(&rec.id).map(|x| x.running.clone())).unwrap_or_default();
            let waiting: Vec<&Delivery> = rec.devices.iter().filter(|d| d.state == WAITING && !running.contains(&d.eid)).collect();
            if waiting.is_empty() {
                continue;
            }
            if t.saturating_sub(rec.created_ms) > WAIT_MAX_MS {
                update(&self.config, &rec.id, |r| {
                    for d in r.devices.iter_mut().filter(|d| d.state == WAITING) {
                        d.state = FAILED.into();
                        d.note = Some("expired".into());
                    }
                });
                self.emit(&rec.id, true);
                continue;
            }
            if !rec.paths.iter().all(|p| Path::new(p).exists()) {
                update(&self.config, &rec.id, |r| {
                    for d in r.devices.iter_mut().filter(|d| d.state == WAITING) {
                        d.state = FAILED.into();
                        d.note = Some("files_gone".into());
                    }
                });
                self.emit(&rec.id, true);
                continue;
            }
            let due: Vec<String> = waiting.iter().filter(|d| {
                let since = t.saturating_sub(d.last_try_ms);
                let seen = crate::mailbox::seen_within(&d.eid, Duration::from_secs(180));
                d.last_try_ms == 0 || (seen && since > 15_000) || since > 10 * 60 * 1000
            }).map(|d| d.eid.clone()).collect();
            if !due.is_empty() {
                self.start(rec.id.clone(), due, true);
            }
        }
    }
}

// ── in the app ──────────────────────────────────────────────────────────────

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// A running leg: where its card updates go.
struct Leg {
    parent: String,
    tx: Option<tokio::sync::oneshot::Sender<Outcome>>,
    progress: Progress,
    done_at: Option<Instant>,
}

static LEGS: Mutex<Option<HashMap<String, Leg>>> = Mutex::new(None);

fn with_legs<T>(f: impl FnOnce(&mut HashMap<String, Leg>) -> T) -> T {
    let mut g = LEGS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// Is `id` one device's leg of a fan-out send (so it gets no card of its own)?
pub(crate) fn is_leg(id: &str) -> bool {
    with_legs(|l| l.contains_key(id))
}

/// A leg's card update: feed it to its send. False = not a leg.
pub(crate) fn route(u: &TransferUpdate) -> bool {
    let (tx, progress) = {
        let mut g = LEGS.lock().unwrap_or_else(|p| p.into_inner());
        let Some(leg) = g.get_or_insert_with(HashMap::new).get_mut(&u.id) else { return false };
        let outcome = match u.state {
            TransferState::Completed => Some(Outcome::Delivered),
            TransferState::Canceled => Some(Outcome::Canceled),
            TransferState::Paused => Some(Outcome::Paused),
            TransferState::Failed => {
                let e = u.error.clone().unwrap_or_default();
                Some(if e == DEVICE_OFFLINE {
                    Outcome::Offline
                } else if e.contains("declined") {
                    Outcome::Declined
                } else {
                    Outcome::Failed(e)
                })
            }
            _ => None,
        };
        match outcome {
            Some(o) => {
                leg.done_at.get_or_insert_with(Instant::now);
                (leg.tx.take().map(|tx| (tx, o)), None)
            }
            None if u.state == TransferState::Transferring && leg.tx.is_some() => (None, Some(leg.progress.clone())),
            None => (None, None),
        }
    };
    if let Some((tx, o)) = tx {
        let _ = tx.send(o);
    }
    if let Some(p) = progress {
        p(u.bytes_done, u.speed_bps);
    }
    true
}

struct AppEnv {
    app: tauri::AppHandle,
}

impl AppEnv {
    fn net(&self) -> Option<Arc<IrohState>> {
        use tauri::Manager;
        self.app.try_state::<Arc<IrohState>>().map(|s| s.inner().clone())
    }
}

/// A later try for one device: its chat card goes first, so the files that
/// follow land on it (the first note only reached the devices online then).
async fn resend_note(app: &tauri::AppHandle, net: &IrohState, rec: &Record, eid: &str) {
    use tauri::Manager;
    let (Some(ep), Some(st)) = (net.get().cloned(), app.try_state::<Arc<crate::AppState>>()) else { return };
    let Some(m) = crate::chat::messages(&st.config_dir, &rec.peer_id).into_iter()
        .find(|m| m.from_me && m.file_xfer_id.as_deref() == Some(rec.chat_id.as_str())) else { return };
    let my_name = st.settings.lock().unwrap().display_name.clone();
    let payload = crate::iroh_net::chat_payload(&m, &rec.peer_id, &my_name);
    let _ = crate::iroh_net::send_chat_any(net, &ep, &[eid.to_owned()], payload).await;
}

impl Env for AppEnv {
    fn direct(&self, job: Job, progress: Progress) -> BoxFut<Outcome> {
        let app = self.app.clone();
        let net = self.net();
        Box::pin(async move {
            let Some(net) = net else { return Outcome::Failed("DropBeam is still starting".into()) };
            if job.retry {
                resend_note(&app, &net, &job.record, &job.eid).await;
            }
            let child = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = tokio::sync::oneshot::channel();
            with_legs(|l| {
                l.retain(|_, leg| leg.done_at.is_none_or(|t| t.elapsed() < Duration::from_secs(120)));
                l.insert(child.clone(), Leg { parent: job.record.id.clone(), tx: Some(tx), progress, done_at: None });
            });
            let rec = job.record;
            let started = crate::iroh_net::send_to_friend_opts(app, net, rec.friend_name.clone(), job.eid.clone(), rec.paths.clone(),
                Some(rec.chat_id.clone()), Some(rec.attempt), Some(ChildCtx { id: child.clone(), first_dial: job.first_dial }),
                Some(vec![job.eid.clone()]));
            if let Err(e) = started {
                with_legs(|l| l.remove(&child));
                return Outcome::Failed(e);
            }
            rx.await.unwrap_or(Outcome::Failed("stopped".into()))
        })
    }

    fn changed(&self, config: &Path, rec: &Record, live: &HashMap<String, (u64, f64)>, significant: bool) {
        use tauri::Emitter;
        let u = card(rec, live);
        let _ = self.app.emit("transfer://update", &u);
        if !significant {
            return;
        }
        if let Some(m) = crate::chat::set_deliveries(config, &rec.peer_id, &rec.chat_id, &rec.devices) {
            let _ = self.app.emit("chat://message", &m);
        }
        // One History row for the whole send, once it reached (or is safely on
        // its way to) the person.
        let reached = rec.devices.iter().any(|d| d.state == DELIVERED || d.state == HELD);
        if !rec.logged && reached && !rec.devices.iter().any(Delivery::busy) {
            update(config, &rec.id, |r| r.logged = true);
            crate::iroh_net::completed_side_effects(&self.app, &rec.id, Direction::Send, rec.names.clone(), rec.total,
                crate::models::Locality::Unknown, Some(rec.friend_name.clone()), None);
        }
    }

    fn stop_legs(&self, id: &str, reason: CancelReason) {
        let Some(net) = self.net() else { return };
        let legs: Vec<String> = with_legs(|l| l.iter().filter(|(_, leg)| leg.parent == id && leg.tx.is_some()).map(|(k, _)| k.clone()).collect());
        for leg in legs {
            net.cancel_with(&leg, reason);
        }
    }
}

/// Start the engine (once iroh is up): finish what's owed, then keep the
/// queue moving — every minute, and the moment a waiting device shows up.
pub fn init(app: tauri::AppHandle, net: Arc<IrohState>, config: PathBuf) {
    let engine = Arc::new(Engine { env: Arc::new(AppEnv { app }), net, config });
    if ENGINE.set(engine.clone()).is_err() {
        return;
    }
    engine.recover();
    spawn(async move {
        loop {
            engine.retry_due();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                _ = wake_cell().notified() => {
                    // Let a device that just connected finish its hello first.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}

/// Stop a fan-out send (from the card's Cancel/Pause). False = not one.
pub(crate) fn stop(_net: &IrohState, id: &str, reason: CancelReason) -> bool {
    ENGINE.get().is_some_and(|e| e.stop(id, reason))
}

/// Route a Transfer Server receipt to its fan-out send. False = not ours.
pub(crate) fn on_receipt(_net: &IrohState, _config: &Path, r: &crate::mailbox::client::Receipt) -> bool {
    ENGINE.get().is_some_and(|e| e.receipt(r))
}

/// Send files to a friend: to every one of their devices, or (`only`) one.
#[allow(clippy::too_many_arguments)]
pub fn send(app: tauri::AppHandle, net: Arc<IrohState>, config: &Path, friend_id: &str, paths: Vec<String>,
    chat_transfer_id: Option<String>, chat_attempt: Option<u64>, only: Option<String>) -> Result<TransferUpdate, String> {
    let friend = crate::friends::get(config, friend_id).ok_or("Friend not found.")?;
    let me = net.get().map(|e| e.id().to_string()).ok_or("DropBeam is still connecting — try again in a moment.")?;
    let (owner, devices) = targets(config, friend_id, &me, only.as_deref())?;
    if devices.len() <= 1 {
        // One device (or the user picked one): the plain friend send.
        let eid = devices.first().map(|d| d.eid.clone()).or(friend.endpoint_id.clone()).ok_or(
            "This friend was added on an old version — re-add them to send directly.",
        )?;
        let hold = only.map(|o| vec![o]);
        return crate::iroh_net::send_to_friend_opts(app, net, friend.name, eid, paths, chat_transfer_id, chat_attempt, None, hold);
    }
    let engine = ENGINE.get().cloned().ok_or("DropBeam is still connecting — try again in a moment.")?;
    let (names, total) = crate::iroh_net::card_summary(&paths)?;
    let id = uuid::Uuid::new_v4().to_string();
    let rec = Record {
        chat_id: chat_transfer_id.unwrap_or_else(|| id.clone()),
        id, peer_id: owner.id.clone(), friend_name: owner.name.clone(), paths, names, total,
        attempt: chat_attempt.unwrap_or(1).max(now()), devices, ..Default::default()
    };
    log::info!("fanout: sending to {} devices of one friend", rec.devices.len());
    let snapshot = card(&rec, &HashMap::new());
    engine.begin(rec);
    Ok(snapshot)
}

#[cfg(test)]
pub(crate) fn engine_for_tests(env: Arc<dyn Env>, net: Arc<IrohState>, config: PathBuf) -> Arc<Engine> {
    Arc::new(Engine { env, net, config })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(eid: &str, state: &str) -> Delivery {
        Delivery { eid: eid.into(), label: eid.into(), state: state.into(), ..Default::default() }
    }

    #[test]
    fn overall_state_reads_like_the_person_got_it() {
        assert_eq!(overall(&[dev("a", DELIVERED), dev("b", DELIVERED)], false), TransferState::Completed);
        assert_eq!(overall(&[dev("a", DELIVERED), dev("b", HELD)], false), TransferState::Completed);
        assert_eq!(overall(&[dev("a", HELD), dev("b", WAITING)], false), TransferState::Held);
        assert_eq!(overall(&[dev("a", SENDING), dev("b", HELD)], false), TransferState::Connecting);
        assert_eq!(overall(&[dev("a", SENDING), dev("b", HELD)], true), TransferState::Transferring);
        assert_eq!(overall(&[dev("a", CANCELED), dev("b", CANCELED)], false), TransferState::Canceled);
        assert_eq!(overall(&[dev("a", FAILED), dev("b", DECLINED)], false), TransferState::Failed);
        assert_eq!(overall(&[dev("a", PAUSED), dev("b", DELIVERED)], false), TransferState::Paused);
    }

    #[test]
    fn device_labels_tell_two_macs_apart() {
        let mut d = vec![
            Delivery { eid: "b".into(), os: Some("macos".into()), ..Default::default() },
            Delivery { eid: "a".into(), os: Some("macos".into()), ..Default::default() },
            Delivery { eid: "c".into(), os: Some("ios".into()), kind: Some("phone".into()), ..Default::default() },
        ];
        label_devices(&mut d);
        assert_eq!(d.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["Mac 2", "Mac 1", "iPhone"]);
    }

    #[test]
    fn card_follows_the_slowest_device_and_each_change_is_a_new_generation() {
        let rec = Record { id: "x".into(), chat_id: "c".into(), total: 100, attempt: 10, gen: 3,
            devices: vec![dev("a", SENDING), dev("b", SENDING)], ..Default::default() };
        let live = HashMap::from([("a".to_string(), (80, 5.0)), ("b".to_string(), (20, 1.0))]);
        let u = card(&rec, &live);
        assert_eq!((u.bytes_done, u.state), (20, TransferState::Transferring));
        assert_eq!(u.chat_transfer.unwrap().attempt, 13);
        let done = Record { devices: vec![dev("a", DELIVERED), dev("b", HELD)], ..rec };
        let u = card(&done, &HashMap::new());
        assert_eq!((u.bytes_done, u.state), (100, TransferState::Completed));
        assert_eq!(u.deliveries.unwrap().len(), 2);
    }
}
