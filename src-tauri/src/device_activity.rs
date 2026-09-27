//! "On your other devices" (GitHub #31): each device tells the user's OTHER own
//! devices (same account, account.rs) what it is sending or receiving right now,
//! so the phone can show the Mac's big send and vice versa. Read-only: nothing
//! here can start, pause or cancel anything.
//!
//! The digest is tiny (names, counts, bytes, state — never paths or contents),
//! goes ONLY to devices holding the account key (every frame is signed with it
//! and verified on arrival), and is sent only while something is happening:
//!   • at most one push per second, whatever the number of transfers;
//!   • a heartbeat every few seconds while anything is live, so the other side
//!     can drop a device that vanished mid-transfer (it expires after `TTL`);
//!   • a finished transfer lingers `LINGER` (so "Done" is seen), then one final
//!     empty digest clears the other side and the device goes quiet.
//!
//! Each push is one short bi-stream on the (usually cached) connection to the
//! own device: `{kind:"account-activity"}` → `{ok}`. An unreachable device is
//! skipped with a growing back-off instead of being dialled every second.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::Notify;

use crate::{account, chat, iroh_net::{self, IrohState}, AppState};

pub(crate) const ACTIVITY_V: u64 = 1;
/// Fastest the digest is pushed (all transfers together).
const MIN_GAP: Duration = Duration::from_secs(1);
/// Re-send an unchanged, non-empty digest this often (liveness).
const HEARTBEAT: Duration = Duration::from_secs(5);
/// A device's digest is dropped when nothing arrived from it for this long.
const TTL: Duration = Duration::from_secs(16);
/// A finished/failed/canceled transfer stays visible this long.
const LINGER: Duration = Duration::from_secs(8);
/// A transfer whose card hasn't changed for this long is treated as gone (a
/// paused or parked card doesn't keep the heartbeat running forever).
const STALE: Duration = Duration::from_secs(10 * 60);
const MAX_ITEMS: usize = 12;
const MAX_NAMES: usize = 3;
const NAME_CAP: usize = 120;

/// One transfer, as another device sees it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: String,
    /// "send" | "receive"
    pub direction: String,
    /// The transfer's state as the UI knows it ("transferring", "completed", …).
    pub state: String,
    /// Up to three file names (the rest is `file_count`).
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub file_count: u64,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub bytes_total: u64,
    #[serde(default)]
    pub percent: f64,
    #[serde(default)]
    pub speed_bps: f64,
    /// Who it's going to / coming from (that device's name for them).
    #[serde(default)]
    pub peer: Option<String>,
}

/// Everything one of the user's other devices is doing right now.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActivity {
    pub endpoint_id: String,
    pub name: String,
    pub kind: Option<String>,
    pub os: Option<String>,
    pub items: Vec<Activity>,
    pub updated_ms: u64,
}

fn terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "canceled" | "held")
}

/// An IP:port, a long hex id or a `.local` host — an address, not a name.
fn raw_address(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    let l = l.trim_start_matches('[');
    (l.chars().all(|c| c.is_ascii_hexdigit() || ".:[]".contains(c)) && (l.contains('.') || l.contains(':')))
        || (l.len() >= 16 && l.chars().all(|c| c.is_ascii_hexdigit()))
        || [".local", ".lan", ".home"].iter().any(|s| l.split(':').next().unwrap_or("").ends_with(s))
}

fn clip(s: &str) -> String {
    if s.chars().count() <= NAME_CAP { return s.to_owned() }
    let mut out: String = s.chars().take(NAME_CAP - 1).collect();
    out.push('…');
    out
}

/// A `transfer://update` payload → the digest's view of it (None: not worth
/// showing — a ghost card with nothing in it).
pub(crate) fn from_update(u: &Value) -> Option<Activity> {
    let id = u["id"].as_str()?.to_owned();
    let direction = u["direction"].as_str()?.to_owned();
    let state = u["state"].as_str()?.to_owned();
    let all: Vec<&str> = u["fileNames"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    let bytes_total = u["bytesTotal"].as_u64().unwrap_or(0);
    if all.is_empty() && bytes_total == 0 { return None }
    let names = all.iter().take(MAX_NAMES).map(|n| {
        // Only the file's own name travels, never where it lives.
        clip(n.rsplit(['/', '\\']).find(|p| !p.is_empty()).unwrap_or(n))
    }).collect();
    let peer = u["friendName"].as_str().or_else(|| u["peer"].as_str())
        .map(str::trim).filter(|p| !p.is_empty() && !raw_address(p)).map(clip);
    Some(Activity {
        id, direction, state, names,
        file_count: u["fileCount"].as_u64().unwrap_or(all.len() as u64).max(all.len() as u64),
        bytes_done: u["bytesDone"].as_u64().unwrap_or(0),
        bytes_total,
        percent: u["percent"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0),
        speed_bps: u["speedBps"].as_f64().unwrap_or(0.0).max(0.0),
        peer,
    })
}

// ── this device's transfers (outbound digest) ───────────────────────────────

struct Tracked {
    item: Activity,
    changed: Instant,
    /// When it reached a terminal state.
    ended: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Local {
    items: HashMap<String, Tracked>,
    order: Vec<String>,
}

impl Local {
    /// Record a card update; true when the digest changed.
    pub(crate) fn observe(&mut self, a: Activity, now: Instant) -> bool {
        let ended = terminal(&a.state).then_some(now);
        match self.items.get_mut(&a.id) {
            Some(t) if t.item == a => false,
            Some(t) => {
                // Keep the first end time (a completed card re-emits on reveal etc.).
                t.ended = if ended.is_some() { t.ended.or(ended) } else { None };
                t.item = a;
                t.changed = now;
                true
            }
            None => {
                // Something that was already over before we ever saw it running
                // (history replays, restored cards) isn't news for other devices.
                if ended.is_some() { return false }
                self.order.push(a.id.clone());
                self.items.insert(a.id.clone(), Tracked { item: a, changed: now, ended: None });
                true
            }
        }
    }
    /// The digest to send now, dropping what has lingered long enough.
    pub(crate) fn snapshot(&mut self, now: Instant) -> Vec<Activity> {
        self.items.retain(|_, t| match t.ended {
            Some(end) => now.duration_since(end) < LINGER,
            None => now.duration_since(t.changed) < STALE,
        });
        let items = &self.items;
        self.order.retain(|id| items.contains_key(id));
        // Newest first, capped.
        self.order.iter().rev().take(MAX_ITEMS).filter_map(|id| items.get(id)).map(|t| t.item.clone()).collect()
    }
}

fn local() -> &'static Mutex<Local> {
    static L: OnceLock<Mutex<Local>> = OnceLock::new();
    L.get_or_init(Default::default)
}

fn wake() -> &'static Notify {
    static N: OnceLock<Notify> = OnceLock::new();
    N.get_or_init(Notify::new)
}

/// Build the signed frame this device sends (None: not in an account).
pub(crate) fn frame(dir: &Path, me: &str, name: &str, kind: &str, items: &[Activity]) -> Option<Value> {
    let account = account::my_pub(dir)?;
    let sig = crate::link::sign_endpoint(dir, me)?;
    Some(json!({
        "kind": "account-activity", "v": ACTIVITY_V, "account": account, "sig": sig,
        "device": { "name": name, "kind": kind, "os": std::env::consts::OS },
        "items": items,
    }))
}

/// Dialer side: push one frame over `conn` and wait for the ack.
pub(crate) async fn push(conn: &iroh::endpoint::Connection, frame: &Value) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    iroh_net::write_frame(&mut send, frame).await?;
    send.finish()?;
    let ack = tokio::time::timeout(Duration::from_secs(5), iroh_net::read_frame_cap(&mut recv, 4096)).await??;
    anyhow::ensure!(ack["ok"] == true, "activity refused");
    Ok(())
}

/// The digest for one own device: without the transfers going to or coming
/// from that very device (it already shows those as its own cards).
pub(crate) fn for_device(items: &[Activity], device_name: &str) -> Vec<Activity> {
    let name = device_name.trim();
    items.iter().filter(|a| name.is_empty() || a.peer.as_deref().map(str::trim) != Some(name)).cloned().collect()
}

// ── other devices' transfers (inbound) ──────────────────────────────────────

struct Remote {
    view: DeviceActivity,
    at: Instant,
}

fn remote() -> &'static Mutex<HashMap<String, Remote>> {
    static R: OnceLock<Mutex<HashMap<String, Remote>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Check and read a digest from `who`. None = not one of this account's
/// devices (wrong/missing signature, a removed device) or malformed.
pub(crate) fn accept(dir: &Path, who: &str, req: &Value) -> Option<DeviceActivity> {
    let account = account::my_pub(dir)?;
    if req["account"].as_str() != Some(account.as_str())
        || !crate::link::verify_account(&account, req["sig"].as_str().unwrap_or(""), who)
        || account::device_was_removed(dir, who) {
        return None;
    }
    let mut items: Vec<Activity> = serde_json::from_value(req["items"].clone()).ok()?;
    items.truncate(MAX_ITEMS);
    for a in &mut items {
        a.names.truncate(MAX_NAMES);
        a.names.iter_mut().for_each(|n| *n = clip(n));
        a.peer = a.peer.as_deref().map(clip);
        a.percent = a.percent.clamp(0.0, 100.0);
    }
    // This device's own record of that device (its label here) wins over what
    // it calls itself.
    let known = account::own_devices(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(who));
    let dev = &req["device"];
    let sent_name = dev["name"].as_str().map(clip).unwrap_or_default();
    Some(DeviceActivity {
        endpoint_id: who.to_owned(),
        name: known.as_ref().map(|f| f.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or(sent_name),
        kind: known.as_ref().and_then(|f| f.device_kind.clone()).or_else(|| dev["kind"].as_str().map(str::to_owned)),
        os: known.as_ref().and_then(|f| f.device_os.clone()).or_else(|| dev["os"].as_str().map(str::to_owned)),
        items,
        updated_ms: chat::now_ms(),
    })
}

/// Store a device's digest; true when what the UI shows changed.
fn store(view: DeviceActivity) -> bool {
    let mut r = remote().lock().unwrap();
    if view.items.is_empty() {
        return r.remove(&view.endpoint_id).is_some();
    }
    let changed = r.get(&view.endpoint_id).is_none_or(|old| old.view.items != view.items || old.view.name != view.name);
    r.insert(view.endpoint_id.clone(), Remote { view, at: Instant::now() });
    changed
}

/// Drop devices that stopped reporting; true when any went.
fn prune() -> bool {
    let mut r = remote().lock().unwrap();
    let before = r.len();
    r.retain(|_, d| d.at.elapsed() < TTL);
    r.len() != before
}

/// What the user's other devices are doing now (for the UI).
pub(crate) fn current() -> Vec<DeviceActivity> {
    prune();
    let r = remote().lock().unwrap();
    let mut v: Vec<DeviceActivity> = r.values().map(|d| d.view.clone()).collect();
    v.sort_by(|a, b| a.name.cmp(&b.name).then(a.endpoint_id.cmp(&b.endpoint_id)));
    v
}

fn announce(app: &AppHandle) {
    let _ = app.emit("account://activity", current());
}

/// Listener side of one `account-activity` stream from `who`.
pub(crate) async fn serve(state: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream) -> anyhow::Result<()> {
    let app = state.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.clone();
    let dir = app.try_state::<Arc<AppState>>().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.config_dir.clone();
    let Some(view) = accept(&dir, who, req) else {
        iroh_net::write_frame(send, &json!({"ok": false})).await?;
        send.finish()?;
        return Ok(());
    };
    iroh_net::write_frame(send, &json!({"ok": true})).await?;
    send.finish()?;
    if store(view) { announce(&app); }
    // Expire it if that device goes quiet (asleep, out of range) mid-transfer.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(TTL + Duration::from_millis(200)).await;
        if prune() { announce(&app); }
    });
    Ok(())
}

// ── the push loop ───────────────────────────────────────────────────────────

/// Watch this device's transfer cards and keep the other own devices told.
pub fn spawn(app: AppHandle, net: Arc<IrohState>) {
    app.listen_any("transfer://update", |ev| {
        let Ok(u) = serde_json::from_str::<Value>(ev.payload()) else { return };
        let Some(a) = from_update(&u) else { return };
        if local().lock().unwrap().observe(a, Instant::now()) { wake().notify_one(); }
    });
    tauri::async_runtime::spawn(async move {
        let mut sent_empty = true;
        let mut last_sent: Option<(Instant, Vec<Activity>)> = None;
        // Unreachable devices: (not before, failures).
        let mut backoff: HashMap<String, (Instant, u32)> = HashMap::new();
        loop {
            let tick = if sent_empty { Duration::from_secs(3600) } else { HEARTBEAT };
            tokio::select! {
                _ = wake().notified() => {}
                _ = tokio::time::sleep(tick) => {}
            }
            // Rate limit: never faster than MIN_GAP, whatever is changing.
            if let Some((at, _)) = &last_sent {
                let since = at.elapsed();
                if since < MIN_GAP { tokio::time::sleep(MIN_GAP - since).await; }
            }
            let items = local().lock().unwrap().snapshot(Instant::now());
            if items.is_empty() && sent_empty { continue }
            let unchanged = last_sent.as_ref().is_some_and(|(at, prev)| *prev == items && at.elapsed() < HEARTBEAT);
            if unchanged { continue }
            let Some(st) = app.try_state::<Arc<AppState>>() else { continue };
            let dir = st.config_dir.clone();
            let Some(ep) = net.get() else { continue };
            let me = ep.id().to_string();
            let kind = st.settings.lock().unwrap().device_kind.clone();
            let name = account::device_name(&kind);
            if account::my_pub(&dir).is_none() {
                sent_empty = true;
                continue;
            }
            let now = Instant::now();
            let targets: Vec<(String, String)> = account::own_devices(&dir).into_iter()
                .filter_map(|f| Some((f.endpoint_id?, f.name)))
                .filter(|(e, _)| backoff.get(e).is_none_or(|(until, _)| now >= *until)).collect();
            let jobs: Vec<_> = targets.into_iter().filter_map(|(eid, their_name)| {
                // A transfer TO that device already shows there as its own card.
                let Some(frame) = frame(&dir, &me, &name, &kind, &for_device(&items, &their_name)) else { return None };
                let net = net.clone();
                Some(tauri::async_runtime::spawn(async move {
                    let res = tokio::time::timeout(Duration::from_secs(8), async {
                        let conn = iroh_net::friend_connection(&net, &eid).await?;
                        push(&conn, &frame).await
                    }).await.unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
                    (eid, res)
                }))
            }).collect();
            for job in jobs {
                let Ok((eid, res)) = job.await else { continue };
                match res {
                    Ok(()) => { backoff.remove(&eid); }
                    Err(e) => {
                        let fails = backoff.get(&eid).map_or(0, |b| b.1) + 1;
                        let wait = Duration::from_secs((15u64 << fails.min(3)).min(120));
                        backoff.insert(eid.clone(), (Instant::now() + wait, fails));
                        log::debug!("activity to {} failed ({fails}x): {e:#}", &eid[..eid.len().min(10)]);
                    }
                }
            }
            sent_empty = items.is_empty();
            last_sent = Some((Instant::now(), items));
        }
    });
}

/// What the user's other devices are sending and receiving right now.
#[tauri::command]
pub fn other_device_activity() -> Vec<DeviceActivity> {
    current()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::testkit::Dev;

    fn update(id: &str, state: &str, done: u64) -> Value {
        json!({"id": id, "direction": "send", "state": state, "fileNames": ["/Users/me/Movies/Drone footage.mov", "b.jpg", "c.jpg", "d.jpg"],
            "fileCount": 4, "bytesDone": done, "bytesTotal": 1000, "percent": done as f64 / 10.0, "speedBps": 5.0, "friendName": "Alex"})
    }

    #[test]
    fn digest_is_names_only_and_capped() {
        let a = from_update(&update("t1", "transferring", 100)).unwrap();
        assert_eq!(a.names, ["Drone footage.mov", "b.jpg", "c.jpg"]);
        assert_eq!(a.file_count, 4);
        assert_eq!(a.peer.as_deref(), Some("Alex"));
        // A ghost card (nothing in it) isn't shown.
        assert!(from_update(&json!({"id": "g", "direction": "receive", "state": "canceled", "fileNames": [], "bytesTotal": 0})).is_none());
        // A raw address never travels as a name.
        let raw = from_update(&json!({"id": "r", "direction": "receive", "state": "transferring", "fileNames": ["x"], "peer": "192.168.1.4:5000"})).unwrap();
        assert_eq!(raw.peer, None);
    }

    #[test]
    fn a_device_is_not_told_about_its_own_transfer() {
        let to_phone = from_update(&json!({"id": "p", "direction": "send", "state": "transferring", "fileNames": ["a"], "friendName": "iPhone"})).unwrap();
        let to_alex = from_update(&update("t1", "transferring", 1)).unwrap();
        let items = vec![to_phone.clone(), to_alex.clone()];
        assert_eq!(for_device(&items, "iPhone"), vec![to_alex]);
        assert_eq!(for_device(&items, "iPad").len(), 2);
    }

    #[test]
    fn local_digest_lingers_then_clears() {
        let mut l = Local::default();
        let t0 = Instant::now();
        // Already-finished cards seen for the first time are not news.
        assert!(!l.observe(from_update(&update("old", "completed", 1000)).unwrap(), t0));
        assert!(l.observe(from_update(&update("t1", "transferring", 100)).unwrap(), t0));
        assert!(!l.observe(from_update(&update("t1", "transferring", 100)).unwrap(), t0), "same card twice isn't a change");
        assert!(l.observe(from_update(&update("t1", "completed", 1000)).unwrap(), t0));
        assert_eq!(l.snapshot(t0 + Duration::from_secs(2))[0].state, "completed");
        assert!(l.snapshot(t0 + LINGER + Duration::from_secs(1)).is_empty());
        // A card that stopped changing long ago (paused, parked) drops out too.
        assert!(l.observe(from_update(&update("t2", "paused", 10)).unwrap(), t0));
        assert!(l.snapshot(t0 + STALE + Duration::from_secs(1)).is_empty());
    }

    /// Two devices of one account over a real loopback connection: the Mac's
    /// digest reaches the phone under the phone's name for it; a device from
    /// another account is refused.
    #[tokio::test]
    async fn digest_reaches_own_device_and_only_it() {
        let k = iroh::SecretKey::generate();
        let (mac, phone) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let stranger = Dev::new("Other", "laptop", Some(&iroh::SecretKey::generate())).await;
        // The phone knows the Mac as one of its own devices.
        crate::friends::upsert_by_endpoint(&phone.dir, &mac.eid(), "Ashton’s MacBook Pro");
        crate::friends::set_device_info(&phone.dir, &mac.eid(), Some("laptop"), crate::account::my_pub(&phone.dir).as_deref());

        let items = vec![from_update(&update("t1", "transferring", 250)).unwrap()];
        for (from, want) in [(&mac, true), (&stranger, false)] {
            let frame = frame(&from.dir, &from.eid(), &from.label, "laptop", &items).unwrap();
            let (srv, dir) = (phone.ep.clone(), phone.dir.clone());
            let served = tokio::spawn(async move {
                let conn = srv.accept().await.unwrap().await.unwrap();
                let who = conn.remote_id().to_string();
                let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                let req = iroh_net::read_frame(&mut recv).await.unwrap();
                let got = accept(&dir, &who, &req);
                iroh_net::write_frame(&mut send, &json!({"ok": got.is_some()})).await.unwrap();
                send.finish().unwrap();
                let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
                got
            });
            let conn = from.ep.connect(phone.ep.addr(), iroh_net::ALPN).await.unwrap();
            let pushed = push(&conn, &frame).await;
            let got = served.await.unwrap();
            assert_eq!(pushed.is_ok(), want, "{}", from.label);
            if want {
                let got = got.unwrap();
                assert_eq!(got.endpoint_id, mac.eid());
                assert_eq!(got.name, "Ashton’s MacBook Pro");
                assert_eq!(got.items, items);
            } else {
                assert!(got.is_none());
            }
        }
        // A digest with a forged signature (right account, wrong device) is refused.
        let mut forged = frame(&mac.dir, &mac.eid(), "Mac", "laptop", &items).unwrap();
        forged["sig"] = json!(crate::link::sign_endpoint(&mac.dir, "someone-else").unwrap());
        assert!(accept(&phone.dir, &mac.eid(), &forged).is_none());
    }

    #[test]
    fn receiver_store_changes_and_clears() {
        let view = |items: Vec<Activity>| DeviceActivity { endpoint_id: "store-test-dev".into(), name: "Mac".into(), kind: None, os: None, items, updated_ms: 0 };
        let a = from_update(&update("s1", "transferring", 1)).unwrap();
        assert!(store(view(vec![a.clone()])));
        assert!(!store(view(vec![a.clone()])), "a heartbeat isn't a change");
        assert!(current().iter().any(|d| d.endpoint_id == "store-test-dev"));
        assert!(store(view(vec![])), "the final empty digest clears the device");
        assert!(!current().iter().any(|d| d.endpoint_id == "store-test-dev"));
    }
}
