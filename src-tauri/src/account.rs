//! Multi-device accounts, peer to peer. Every device that holds the same account
//! key (minted/shared by link.rs) is one of the user's OWN devices. Own devices
//! keep each other's device list, friends and chat history in step over iroh —
//! there is no server; the key is the account.
//!
//! One "account-sync" exchange runs on a single bi-stream, four frames:
//!   A → B  A's roster + friends + tombstones, one hash per chat thread (signed)
//!   B → A  B's roster + friends + tombstones, (key, hash) lists for every thread
//!          whose hash differs, avatars B wants
//!   A → B  messages B lacks or holds older, keys A wants, avatars B asked for
//!   B → A  the messages A wanted, avatars A asked for
//! B answers only a device that signs its endpoint id with the SAME account key.
//! Chat threads are matched by the friend's endpoint id (friend ids are local).
//! Merges are last-writer-wins per message (`ChatMessage::rev`) + furthest
//! delivery status, so repeated exchanges converge and stop.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::{chat, friends, iroh_net::{self, IrohState}, models::Friend, AppState};

pub(crate) const SYNC_V: u64 = 1;
/// Cap for every frame after the first (the dispatcher caps the first at 1 MiB).
const FRAME_CAP: usize = 48 << 20;
/// Serialized message bytes per frame; anything left over goes next round.
const MSG_BUDGET: usize = 24 << 20;
const AVATAR_CAP: usize = 2 << 20;
/// Re-exchange with a device at least this often even if nothing changed here,
/// so its changes still arrive if its own nudge to us was lost.
const REFRESH: Duration = Duration::from_secs(10 * 60);

// ── change notification ─────────────────────────────────────────────────────

fn changed() -> &'static Notify {
    static N: OnceLock<Notify> = OnceLock::new();
    N.get_or_init(Notify::new)
}

/// Something syncable changed (a chat message, a friend): wake the sync loop.
/// Cheap; the loop debounces and skips devices already in step.
pub fn note_change() {
    changed().notify_one();
}

// ── persistent account bookkeeping (account-state.json) ────────────────────

#[derive(Default, Serialize, Deserialize, Clone)]
struct Book {
    /// The account these entries belong to (reset when the key changes).
    #[serde(default)]
    account: String,
    /// When each own device joined the account (ms), merged by max.
    #[serde(default)]
    linked: HashMap<String, u64>,
    /// Devices removed from the account (ms), merged by max. A device is out
    /// while its removal is newer than its link time.
    #[serde(default)]
    removed_devices: HashMap<String, u64>,
    /// Friends removed on some own device (ms), by endpoint id, merged by max.
    #[serde(default)]
    removed_friends: HashMap<String, u64>,
}

static BOOK_LOCK: Mutex<()> = Mutex::new(());

fn book_path(dir: &Path) -> std::path::PathBuf {
    dir.join("account-state.json")
}

fn read_book(dir: &Path, account: &str) -> Book {
    let b: Book = std::fs::read(book_path(dir)).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if b.account == account { b } else { Book { account: account.to_owned(), ..Default::default() } }
}

fn write_book(dir: &Path, b: &Book) {
    if let Ok(bytes) = serde_json::to_vec(b) {
        if let Err(e) = crate::settings::write_atomic(&book_path(dir), &bytes) {
            log::warn!("account: cannot save account-state.json: {e}");
        }
    }
}

fn with_book<T>(dir: &Path, account: &str, f: impl FnOnce(&mut Book) -> T) -> T {
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut b = read_book(dir, account);
    let out = f(&mut b);
    write_book(dir, &b);
    out
}

impl Book {
    fn is_removed(&self, eid: &str) -> bool {
        self.removed_devices.get(eid).is_some_and(|r| *r >= self.linked.get(eid).copied().unwrap_or(0))
    }
}

pub(crate) fn my_pub(dir: &Path) -> Option<String> {
    crate::link::account_pub(dir)
}

/// True when `account_pub` is this device's account and `eid` was removed from it.
pub(crate) fn is_removed_device(dir: &Path, account_pub: &str, eid: &str) -> bool {
    if my_pub(dir).as_deref() != Some(account_pub) {
        return false;
    }
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_book(dir, account_pub).is_removed(eid)
}

/// A device joined the account now (either end of a link): record it and clear
/// any older removal so the relink sticks everywhere.
pub(crate) fn mark_linked(dir: &Path, eid: &str) {
    let Some(account) = my_pub(dir) else { return };
    let now = chat::now_ms();
    with_book(dir, &account, |b| {
        let at = b.linked.entry(eid.to_owned()).or_default();
        *at = (*at).max(now).max(b.removed_devices.get(eid).map_or(0, |r| r + 1));
    });
    note_change();
}

/// A friend was removed on this device: remember it so the user's other devices
/// drop them too (unless they are re-added later).
pub(crate) fn record_friend_removed(dir: &Path, friend: &Friend) {
    let (Some(account), Some(eid)) = (my_pub(dir), friend.endpoint_id.as_deref()) else { return };
    with_book(dir, &account, |b| {
        let at = b.removed_friends.entry(eid.to_owned()).or_default();
        *at = (*at).max(chat::now_ms()).max(friend.created_at);
    });
    note_change();
}

/// The user's own devices (other than this one) that are still in the account.
pub(crate) fn own_devices(dir: &Path) -> Vec<Friend> {
    let Some(account) = my_pub(dir) else { return vec![] };
    let book = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(dir, &account) };
    friends::load(dir).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account.as_str()))
        .filter(|f| f.endpoint_id.as_deref().is_some_and(|e| !book.is_removed(e)))
        .collect()
}

pub(crate) fn is_own_device(dir: &Path, eid: &str) -> bool {
    own_devices(dir).iter().any(|f| f.endpoint_id.as_deref() == Some(eid))
}

// ── in-memory sync status ───────────────────────────────────────────────────

#[derive(Default)]
struct Status {
    /// Local fingerprint at the end of the last good exchange, per device.
    in_step: HashMap<String, (String, Instant)>,
    /// When each device last completed an exchange (ms since epoch).
    last_ok: HashMap<String, u64>,
    /// Retry gate after failures: (not before, consecutive failures).
    backoff: HashMap<String, (Instant, u32)>,
    running: HashSet<String>,
}

fn status() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// One of our devices just connected to us: sync with it soon, whatever the backoff.
pub(crate) fn device_seen(dir: &Path, eid: &str) {
    if !is_own_device(dir, eid) {
        return;
    }
    let mut s = status().lock().unwrap();
    s.backoff.remove(eid);
    let stale = s.in_step.get(eid).is_none_or(|(_, at)| at.elapsed() > Duration::from_secs(60));
    if stale {
        s.in_step.remove(eid);
        drop(s);
        note_change();
    }
}

pub(crate) fn last_sync(eid: &str) -> Option<u64> {
    status().lock().unwrap().last_ok.get(eid).copied()
}

// ── wire records ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
struct DeviceRec {
    eid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    os: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Roster {
    me: DeviceRec,
    #[serde(default)]
    devices: Vec<DeviceRec>,
    #[serde(default)]
    linked: HashMap<String, u64>,
    #[serde(default)]
    removed: HashMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct FriendRec {
    eid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    name_custom: bool,
    #[serde(default)]
    created_at: u64,
    #[serde(default = "yes")]
    auto_accept: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    account: Option<String>,
    /// mtime (ms) of the cached profile picture, 0 = none.
    #[serde(default)]
    avatar: u64,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    roster: Roster,
    #[serde(default)]
    friends: Vec<FriendRec>,
    #[serde(default)]
    removed_friends: HashMap<String, u64>,
}

/// Everything this device shares about the account, plus thread summaries.
struct Local {
    account: String,
    me: String,
    meta: Meta,
    /// friend endpoint id → local friend id, for friends whose threads sync.
    threads: HashMap<String, String>,
    /// friend endpoint id → current avatar mtime / path.
    avatars: HashMap<String, (u64, String)>,
}

fn mtime_ms(path: &str) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

fn this_device(st: &AppState, me: &str) -> DeviceRec {
    let s = st.settings.lock().unwrap();
    DeviceRec { eid: me.to_owned(), name: s.display_name.clone(), kind: Some(s.device_kind.clone()),
        os: Some(std::env::consts::OS.to_owned()) }
}

fn gather(st: &AppState, account: &str, me: &str) -> Local {
    let dir = &st.config_dir;
    let book = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(dir, account) };
    let all = friends::load(dir);
    let mut devices = vec![];
    let mut recs = vec![];
    let mut threads = HashMap::new();
    let mut avatars = HashMap::new();
    for f in all {
        let Some(eid) = f.endpoint_id.clone() else { continue };
        if eid == me {
            continue;
        }
        if f.account_pub.as_deref() == Some(account) {
            if !book.is_removed(&eid) {
                devices.push(DeviceRec { eid, name: f.name, kind: f.device_kind, os: f.device_os });
            }
            continue;
        }
        let avatar = f.avatar.as_deref().map(|p| (mtime_ms(p), p.to_owned())).filter(|(m, _)| *m > 0);
        if let Some(a) = &avatar {
            avatars.insert(eid.clone(), a.clone());
        }
        threads.insert(eid.clone(), f.id.clone());
        recs.push(FriendRec { eid, name: f.name, name_custom: f.name_custom, created_at: f.created_at,
            auto_accept: f.auto_accept, kind: f.device_kind, os: f.device_os, account: f.account_pub,
            avatar: avatar.map_or(0, |a| a.0) });
    }
    recs.sort_by(|a, b| a.eid.cmp(&b.eid));
    devices.sort_by(|a, b| a.eid.cmp(&b.eid));
    Local {
        account: account.to_owned(),
        me: me.to_owned(),
        meta: Meta {
            roster: Roster { me: this_device(st, me), devices, linked: book.linked, removed: book.removed_devices },
            friends: recs,
            removed_friends: book.removed_friends,
        },
        threads,
        avatars,
    }
}

fn thread_hash(digest: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    for (k, v) in digest {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(v.as_bytes());
        h.update(b";");
    }
    hex::encode(&h.finalize()[..8])
}

fn summaries(dir: &Path, local: &Local) -> HashMap<String, String> {
    local.threads.iter().filter_map(|(eid, fid)| {
        let d = chat::sync_digest(dir, fid);
        (!d.is_empty()).then(|| (eid.clone(), thread_hash(&d)))
    }).collect()
}

/// A digest of everything this device would send, to skip devices already in step.
fn fingerprint(dir: &Path, local: &Local) -> String {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&local.meta.friends).unwrap_or_default());
    h.update(serde_json::to_vec(&local.meta.roster.devices).unwrap_or_default());
    let mut sums: Vec<_> = summaries(dir, local).into_iter().collect();
    sums.sort();
    h.update(serde_json::to_vec(&sums).unwrap_or_default());
    let mut tombs: Vec<_> = local.meta.removed_friends.iter().chain(local.meta.roster.removed.iter()).collect();
    tombs.sort();
    h.update(serde_json::to_vec(&tombs).unwrap_or_default());
    hex::encode(&h.finalize()[..12])
}

// ── applying what another own device told us ───────────────────────────────

struct Applied {
    /// Friends newly learned (this device should introduce itself to them).
    new_friends: Vec<String>,
    /// Friends whose picture the other device has newer.
    want_avatars: Vec<String>,
    left_account: bool,
    changed: bool,
}

fn apply_meta(dir: &Path, local: &Local, from: &str, meta: &Meta) -> Applied {
    let account = &local.account;
    let mut out = Applied { new_friends: vec![], want_avatars: vec![], left_account: false, changed: false };
    // 1. Tombstones + link times, merged by max.
    let book = with_book(dir, account, |b| {
        for (e, t) in &meta.roster.linked { let x = b.linked.entry(e.clone()).or_default(); *x = (*x).max(*t); }
        for (e, t) in &meta.roster.removed { let x = b.removed_devices.entry(e.clone()).or_default(); *x = (*x).max(*t); }
        for (e, t) in &meta.removed_friends { let x = b.removed_friends.entry(e.clone()).or_default(); *x = (*x).max(*t); }
        b.clone()
    });
    if book.is_removed(&local.me) {
        leave_account(dir, account);
        out.left_account = true;
        out.changed = true;
        return out;
    }
    // 2. Devices: the sender itself and every device it lists.
    let mut devices = meta.roster.devices.clone();
    let mut sender = meta.roster.me.clone();
    sender.eid = from.to_owned();
    devices.push(sender);
    let mut device_ids = HashSet::new();
    for d in &devices {
        if d.eid == local.me || d.eid.parse::<iroh::EndpointId>().is_err() || book.is_removed(&d.eid) {
            continue;
        }
        device_ids.insert(d.eid.clone());
        let linked = book.linked.get(&d.eid).copied().unwrap_or(0);
        out.changed |= friends::upsert_own_device(dir, &d.eid, &d.name, d.kind.as_deref(), d.os.as_deref(), account, linked);
    }
    // Devices removed from the account: forget them here too.
    for f in friends::load(dir) {
        let Some(eid) = f.endpoint_id.as_deref() else { continue };
        if f.account_pub.as_deref() == Some(account.as_str()) && book.is_removed(eid) {
            let _ = friends::remove(dir, &f.id);
            out.changed = true;
        }
    }
    // 3. Friends.
    for r in &meta.friends {
        if r.eid == local.me || device_ids.contains(&r.eid) || r.account.as_deref() == Some(account.as_str())
            || r.eid.parse::<iroh::EndpointId>().is_err() {
            continue;
        }
        if book.removed_friends.get(&r.eid).is_some_and(|t| *t >= r.created_at) {
            continue;
        }
        let (friend, added) = friends::import_synced_friend(dir, &friends::SyncedFriend {
            endpoint_id: &r.eid, name: &r.name, name_custom: r.name_custom, created_at: r.created_at,
            auto_accept: r.auto_accept, device_kind: r.kind.as_deref(), device_os: r.os.as_deref(),
            account_pub: r.account.as_deref(),
        });
        if added {
            out.new_friends.push(r.eid.clone());
            out.changed = true;
        }
        let mine = friend.avatar.as_deref().map_or(0, mtime_ms);
        if r.avatar > mine {
            out.want_avatars.push(r.eid.clone());
        }
    }
    for f in friends::load(dir) {
        let Some(eid) = f.endpoint_id.as_deref() else { continue };
        if f.account_pub.as_deref() == Some(account.as_str()) {
            continue;
        }
        if book.removed_friends.get(eid).is_some_and(|t| *t >= f.created_at) {
            let _ = friends::remove(dir, &f.id);
            out.changed = true;
        }
    }
    out
}

fn leave_account(dir: &Path, account: &str) {
    log::warn!("account: this device was removed from its account; unlinking");
    crate::link::forget_key(dir);
    friends::clear_account(dir, account);
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = std::fs::remove_file(book_path(dir));
}

fn avatars_for(local: &Local, wanted: &[String]) -> HashMap<String, Value> {
    wanted.iter().filter_map(|eid| {
        let (mtime, path) = local.avatars.get(eid)?;
        let bytes = std::fs::read(path).ok().filter(|b| !b.is_empty() && b.len() <= AVATAR_CAP)?;
        Some((eid.clone(), json!({"mtime": mtime, "b64": STANDARD.encode(bytes)})))
    }).collect()
}

fn apply_avatars(dir: &Path, avatars: &Value) -> bool {
    let Some(map) = avatars.as_object() else { return false };
    let mut changed = false;
    for (eid, v) in map {
        if eid.parse::<iroh::EndpointId>().is_err() {
            continue;
        }
        let Some(bytes) = v["b64"].as_str().filter(|s| s.len() <= AVATAR_CAP * 4 / 3 + 4)
            .and_then(|s| STANDARD.decode(s).ok()).filter(|b| !b.is_empty() && b.len() <= AVATAR_CAP) else { continue };
        let mtime = v["mtime"].as_u64().unwrap_or(0);
        let path = dir.join(format!("friend-avatar-{eid}.jpg"));
        let current = mtime_ms(&path.to_string_lossy());
        if mtime <= current || std::fs::write(&path, &bytes).is_err() {
            continue;
        }
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(mtime));
        }
        changed |= friends::set_avatar_by_endpoint(dir, eid, path.to_string_lossy().into_owned());
    }
    changed
}

/// (key, hash) lists of our threads whose summary differs from theirs.
fn differing_lists(dir: &Path, local: &Local, theirs: &HashMap<String, String>) -> HashMap<String, Vec<(String, String)>> {
    let mine = summaries(dir, local);
    let mut out = HashMap::new();
    for eid in mine.keys().chain(theirs.keys()).collect::<HashSet<_>>() {
        if mine.get(eid) == theirs.get(eid) {
            continue;
        }
        let Some(fid) = local.threads.get(eid) else { continue };
        out.insert(eid.clone(), chat::sync_digest(dir, fid));
    }
    out
}

/// Messages to send + keys to request, given the other side's lists.
fn plan(dir: &Path, local: &Local, lists: &HashMap<String, Vec<(String, String)>>)
    -> (HashMap<String, Vec<chat::ChatMessage>>, HashMap<String, Vec<String>>) {
    let mut send = HashMap::new();
    let mut want = HashMap::new();
    let mut budget = MSG_BUDGET;
    for (eid, theirs) in lists {
        let Some(fid) = local.threads.get(eid) else { continue };
        let theirs: HashMap<&str, &str> = theirs.iter().map(|(k, h)| (k.as_str(), h.as_str())).collect();
        let mine = chat::sync_digest(dir, fid);
        let mine_map: HashMap<&str, &str> = mine.iter().map(|(k, h)| (k.as_str(), h.as_str())).collect();
        let out_keys: HashSet<String> = mine.iter()
            .filter(|(k, h)| theirs.get(k.as_str()) != Some(&h.as_str()))
            .map(|(k, _)| k.clone()).collect();
        let in_keys: Vec<String> = theirs.iter()
            .filter(|(k, h)| mine_map.get(*k) != Some(*h))
            .map(|(k, _)| (*k).to_owned()).collect();
        if !in_keys.is_empty() {
            want.insert(eid.clone(), in_keys);
        }
        if out_keys.is_empty() || budget == 0 {
            continue;
        }
        // Newest first so a capped round still brings the recent conversation.
        let mut msgs = chat::sync_messages(dir, fid, &out_keys);
        msgs.reverse();
        let mut kept = vec![];
        for m in msgs {
            let size = serde_json::to_vec(&m).map_or(0, |v| v.len());
            if size > budget { budget = 0; break; }
            budget -= size;
            kept.push(m);
        }
        send.insert(eid.clone(), kept);
    }
    (send, want)
}

fn wanted_messages(dir: &Path, local: &Local, want: &Value) -> HashMap<String, Vec<chat::ChatMessage>> {
    let mut out = HashMap::new();
    let mut budget = MSG_BUDGET;
    for (eid, keys) in want.as_object().into_iter().flatten() {
        let Some(fid) = local.threads.get(eid) else { continue };
        let keys: HashSet<String> = keys.as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect();
        let mut msgs = chat::sync_messages(dir, fid, &keys);
        msgs.reverse();
        let mut kept = vec![];
        for m in msgs {
            let size = serde_json::to_vec(&m).map_or(0, |v| v.len());
            if size > budget { budget = 0; break; }
            budget -= size;
            kept.push(m);
        }
        out.insert(eid.clone(), kept);
    }
    out
}

fn apply_messages(dir: &Path, messages: &Value) -> usize {
    let mut changed = 0;
    for (eid, msgs) in messages.as_object().into_iter().flatten() {
        let Some(friend) = friends::load(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(eid.as_str())) else { continue };
        let Ok(msgs) = serde_json::from_value::<Vec<chat::ChatMessage>>(msgs.clone()) else { continue };
        changed += chat::merge_synced(dir, &friend.id, msgs);
    }
    changed
}

fn announce(app: &AppHandle, net: &Arc<IrohState>, applied: &Applied, chats: usize) {
    if applied.changed || !applied.new_friends.is_empty() {
        let _ = app.emit("friends://changed", ());
    }
    if chats > 0 {
        let _ = app.emit("chat://changed", ());
    }
    if applied.left_account {
        let _ = app.emit("account://left", ());
    }
    // Friends this device just learned about: introduce ourselves so they can
    // reach (and recognize) this device too.
    if !applied.new_friends.is_empty() {
        if let Some(st) = app.try_state::<Arc<AppState>>() {
            let name = st.settings.lock().unwrap().display_name.clone();
            for eid in &applied.new_friends {
                iroh_net::say_hello_to_endpoint(net.clone(), eid.clone(), name.clone());
            }
        }
    }
}

// ── the exchange ────────────────────────────────────────────────────────────

fn sign(dir: &Path, me: &str) -> Option<String> {
    crate::link::sign_endpoint(dir, me)
}

/// Dialer side: run one full exchange with own device `eid`.
async fn sync_with(app: &AppHandle, net: &Arc<IrohState>, eid: &str) -> anyhow::Result<()> {
    let st = app.state::<Arc<AppState>>();
    let dir = st.config_dir.clone();
    let me = net.get().ok_or_else(|| anyhow::anyhow!("network not ready"))?.id().to_string();
    let account = my_pub(&dir).ok_or_else(|| anyhow::anyhow!("no account"))?;
    let local = gather(&st, &account, &me);
    let hello = json!({
        "kind": "account-sync", "v": SYNC_V, "account": account,
        "sig": sign(&dir, &me).ok_or_else(|| anyhow::anyhow!("no account key"))?,
        "meta": local.meta, "summaries": summaries(&dir, &local),
    });
    let conn = iroh_net::friend_connection(net, eid).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    iroh_net::write_frame(&mut send, &hello).await?;
    let t = Duration::from_secs(60);
    let reply = tokio::time::timeout(t, iroh_net::read_frame_cap(&mut recv, FRAME_CAP)).await??;
    match reply["kind"].as_str() {
        Some("account-sync-ok") => {}
        Some("account-sync-removed") => {
            // The other device removed this one from the account.
            leave_account(&dir, &account);
            let _ = app.emit("account://left", ());
            let _ = app.emit("friends://changed", ());
            anyhow::bail!("removed from account");
        }
        other => anyhow::bail!("account sync refused ({})", other.unwrap_or("?")),
    }
    let meta: Meta = serde_json::from_value(reply["meta"].clone())?;
    let applied = apply_meta(&dir, &local, eid, &meta);
    if applied.left_account {
        announce(app, net, &applied, 0);
        return Ok(());
    }
    let local = gather(&st, &account, &me);
    let lists: HashMap<String, Vec<(String, String)>> = serde_json::from_value(reply["lists"].clone()).unwrap_or_default();
    let (messages, want) = plan(&dir, &local, &lists);
    let their_wants: Vec<String> = serde_json::from_value(reply["want_avatars"].clone()).unwrap_or_default();
    iroh_net::write_frame(&mut send, &json!({
        "messages": messages, "want": want,
        "avatars": avatars_for(&local, &their_wants), "want_avatars": applied.want_avatars,
    })).await?;
    send.finish()?;
    let last = tokio::time::timeout(t, iroh_net::read_frame_cap(&mut recv, FRAME_CAP)).await??;
    let chats = apply_messages(&dir, &last["messages"]);
    let avatars = apply_avatars(&dir, &last["avatars"]);
    let applied = Applied { changed: applied.changed || avatars, ..applied };
    announce(app, net, &applied, chats);
    Ok(())
}

/// Listener side: serve one exchange started by `who`.
pub(crate) async fn serve(state: &IrohState, who: &str, req: &Value,
    send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<()> {
    let app = state.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.clone();
    let net = app.try_state::<Arc<IrohState>>().ok_or_else(|| anyhow::anyhow!("network unavailable"))?.inner().clone();
    let net = &net;
    let st = app.state::<Arc<AppState>>();
    let dir = st.config_dir.clone();
    let me = net.get().ok_or_else(|| anyhow::anyhow!("network not ready"))?.id().to_string();
    let account = my_pub(&dir);
    let verified = account.as_deref().is_some_and(|a| req["account"].as_str() == Some(a)
        && crate::link::verify_account(a, req["sig"].as_str().unwrap_or(""), who));
    let Some(account) = account.filter(|_| verified) else {
        iroh_net::write_frame(send, &json!({"kind": "account-sync-denied"})).await?;
        send.finish()?;
        return Ok(());
    };
    let removed = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(&dir, &account).is_removed(who) };
    if removed {
        iroh_net::write_frame(send, &json!({"kind": "account-sync-removed"})).await?;
        send.finish()?;
        return Ok(());
    }
    let local = gather(&st, &account, &me);
    let meta: Meta = serde_json::from_value(req["meta"].clone())?;
    let applied = apply_meta(&dir, &local, who, &meta);
    if applied.left_account {
        iroh_net::write_frame(send, &json!({"kind": "account-sync-denied"})).await?;
        send.finish()?;
        announce(&app, net, &applied, 0);
        return Ok(());
    }
    let local = gather(&st, &account, &me);
    let theirs: HashMap<String, String> = serde_json::from_value(req["summaries"].clone()).unwrap_or_default();
    iroh_net::write_frame(send, &json!({
        "kind": "account-sync-ok", "meta": local.meta,
        "lists": differing_lists(&dir, &local, &theirs), "want_avatars": applied.want_avatars,
    })).await?;
    let t = Duration::from_secs(60);
    let third = tokio::time::timeout(t, iroh_net::read_frame_cap(recv, FRAME_CAP)).await??;
    let chats = apply_messages(&dir, &third["messages"]);
    let avatars = apply_avatars(&dir, &third["avatars"]);
    let their_wants: Vec<String> = serde_json::from_value(third["want_avatars"].clone()).unwrap_or_default();
    let local = gather(&st, &account, &me);
    iroh_net::write_frame(send, &json!({
        "messages": wanted_messages(&dir, &local, &third["want"]),
        "avatars": avatars_for(&local, &their_wants),
    })).await?;
    send.finish()?;
    let _ = tokio::time::timeout(Duration::from_secs(10), send.stopped()).await;
    {
        let mut s = status().lock().unwrap();
        s.last_ok.insert(who.to_owned(), chat::now_ms());
        s.backoff.remove(who);
    }
    let applied = Applied { changed: applied.changed || avatars, ..applied };
    announce(&app, net, &applied, chats);
    let _ = app.emit("account://synced", who);
    Ok(())
}

// ── the loop ────────────────────────────────────────────────────────────────

async fn round(app: &AppHandle, net: &Arc<IrohState>, force: bool) {
    let Some(st) = app.try_state::<Arc<AppState>>() else { return };
    let dir = st.config_dir.clone();
    let Some(account) = my_pub(&dir) else { return };
    let Some(me) = net.get().map(|e| e.id().to_string()) else { return };
    let fp = fingerprint(&dir, &gather(&st, &account, &me));
    let devices: Vec<String> = own_devices(&dir).into_iter().filter_map(|f| f.endpoint_id).collect();
    let targets: Vec<String> = {
        let mut s = status().lock().unwrap();
        let mut picked = vec![];
        for eid in devices {
            let busy = s.running.contains(&eid);
            let waiting = !force && s.backoff.get(&eid).is_some_and(|(until, _)| Instant::now() < *until);
            let in_step = !force && s.in_step.get(&eid).is_some_and(|(f, at)| *f == fp && at.elapsed() < REFRESH);
            if !busy && !waiting && !in_step {
                s.running.insert(eid.clone());
                picked.push(eid);
            }
        }
        picked
    };
    let jobs = targets.into_iter().map(|eid| {
        let (app, net, dir, account, me) = (app.clone(), net.clone(), dir.clone(), account.clone(), me.clone());
        async move {
            let res = tokio::time::timeout(Duration::from_secs(150), sync_with(&app, &net, &eid)).await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
            let mut s = status().lock().unwrap();
            s.running.remove(&eid);
            match res {
                Ok(()) => {
                    let fp = app.try_state::<Arc<AppState>>().map(|st| fingerprint(&dir, &gather(&st, &account, &me))).unwrap_or_default();
                    s.in_step.insert(eid.clone(), (fp, Instant::now()));
                    s.last_ok.insert(eid.clone(), chat::now_ms());
                    s.backoff.remove(&eid);
                    drop(s);
                    let _ = app.emit("account://synced", &eid);
                }
                Err(e) => {
                    let fails = s.backoff.get(&eid).map_or(0, |b| b.1) + 1;
                    let wait = Duration::from_secs((30u64 << fails.min(5)).min(600));
                    s.backoff.insert(eid.clone(), (Instant::now() + wait, fails));
                    log::debug!("account sync with {} failed ({fails}x): {e:#}", &eid[..eid.len().min(10)]);
                }
            }
        }
    });
    let handles: Vec<_> = jobs.map(tauri::async_runtime::spawn).collect();
    for h in handles {
        let _ = h.await;
    }
}

/// Keep this device's account in step with the user's other devices.
pub fn spawn(app: AppHandle, net: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(12)).await;
        loop {
            let force = FORCE.swap(false, std::sync::atomic::Ordering::SeqCst);
            round(&app, &net, force).await;
            tokio::select! {
                _ = changed().notified() => {
                    // Debounce bursts (a thread of messages, a hello storm).
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(120)) => {}
            }
        }
    });
}

static FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ── commands ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DeviceView {
    friend_id: Option<String>,
    endpoint_id: String,
    name: String,
    device_kind: Option<String>,
    device_os: Option<String>,
    last_sync_ms: Option<u64>,
    this_device: bool,
}

pub(crate) fn device_views(st: &AppState, me: &str) -> Vec<DeviceView> {
    let s = st.settings.lock().unwrap();
    let mut out = vec![DeviceView { friend_id: None, endpoint_id: me.to_owned(), name: s.display_name.clone(),
        device_kind: Some(s.device_kind.clone()), device_os: Some(std::env::consts::OS.to_owned()),
        last_sync_ms: None, this_device: true }];
    drop(s);
    for f in own_devices(&st.config_dir) {
        let eid = f.endpoint_id.clone().unwrap_or_default();
        out.push(DeviceView { friend_id: Some(f.id), last_sync_ms: last_sync(&eid), endpoint_id: eid, name: f.name,
            device_kind: f.device_kind, device_os: f.device_os, this_device: false });
    }
    out
}

/// Sync with every own device now (pull-to-refresh / "Sync now").
#[tauri::command]
pub fn account_sync_now() {
    {
        let mut s = status().lock().unwrap();
        s.backoff.clear();
        s.in_step.clear();
    }
    FORCE.store(true, std::sync::atomic::Ordering::SeqCst);
    note_change();
}

/// Remove one of the user's devices from the account, on every device.
#[tauri::command]
pub fn account_remove_device(app: AppHandle, state: State<'_, Arc<AppState>>, endpoint_id: String) -> Result<(), String> {
    let dir = &state.config_dir;
    let account = my_pub(dir).ok_or("This device isn't linked to an account.")?;
    with_book(dir, &account, |b| {
        let at = b.removed_devices.entry(endpoint_id.clone()).or_default();
        *at = (*at).max(chat::now_ms()).max(b.linked.get(&endpoint_id).map_or(0, |l| l + 1));
    });
    if let Some(f) = friends::load(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(endpoint_id.as_str())) {
        friends::remove(dir, &f.id)?;
    }
    let _ = app.emit("friends://changed", ());
    account_sync_now();
    Ok(())
}

/// Take this device out of its account (it keeps its data as a standalone device).
#[tauri::command]
pub async fn account_leave(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<(), String> {
    let dir = state.config_dir.clone();
    let account = my_pub(&dir).ok_or("This device isn't linked to an account.")?;
    let me = iroh.get().ok_or("Network not ready")?.id().to_string();
    // Tell the other devices first (best effort), then forget the key.
    with_book(&dir, &account, |b| { b.removed_devices.insert(me.clone(), chat::now_ms().max(b.linked.get(&me).map_or(0, |l| l + 1))); });
    let net = iroh.inner().clone();
    let targets: Vec<String> = own_devices(&dir).into_iter().filter_map(|f| f.endpoint_id).collect();
    let handles: Vec<_> = targets.into_iter().map(|eid| {
        let (app, net) = (app.clone(), net.clone());
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(20), sync_with(&app, &net, &eid)).await;
        })
    }).collect();
    for h in handles {
        let _ = h.await;
    }
    if my_pub(&dir).is_some() {
        leave_account(&dir, &account);
    }
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("account://left", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("db-account-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn eid() -> String {
        iroh::SecretKey::generate().public().to_string()
    }
    fn local(dir: &Path, account: &str, me: &str) -> Local {
        let st = AppState::for_tests(dir.to_path_buf());
        gather(&st, account, me)
    }
    fn text(id: &str, peer: &str, from_me: bool, ts: u64) -> chat::ChatMessage {
        serde_json::from_value(json!({"id": id, "peerId": peer, "fromMe": from_me, "kind": "text",
            "text": format!("m{id}"), "files": [], "bytes": 0, "status": if from_me { Some("sending") } else { None }, "ts": ts, "seq": ts})).unwrap()
    }

    #[test]
    fn book_link_and_removal_ordering() {
        let mut b = Book::default();
        assert!(!b.is_removed("x"));
        b.removed_devices.insert("x".into(), 10);
        assert!(b.is_removed("x"));
        b.linked.insert("x".into(), 11);
        assert!(!b.is_removed("x"), "a relink after the removal wins");
        b.removed_devices.insert("x".into(), 11);
        assert!(b.is_removed("x"), "a removal at the link instant removes");
    }

    #[test]
    fn two_devices_converge_friends_and_chats() {
        let (a, b) = (dir(), dir());
        let key = iroh::SecretKey::generate();
        let account = hex::encode(key.public().as_bytes());
        for d in [&a, &b] { crate::link::adopt_key_for_tests(d, &key); }
        let (me_a, me_b, f1, f2) = (eid(), eid(), eid(), eid());
        // A knows friend 1 with a thread; B knows friend 2 with a thread.
        let fa = friends::upsert_by_endpoint(&a, &f1, "Mong");
        chat::append(&a, &text("1", &fa.id, true, 1));
        chat::append(&a, &text("2", &fa.id, false, 2));
        let fb = friends::upsert_by_endpoint(&b, &f2, "Ethan");
        chat::append(&b, &text("3", &fb.id, false, 3));
        // B → A meta, then A → B meta (both directions of one exchange).
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        apply_meta(&a, &la, &me_b, &lb.meta);
        apply_meta(&b, &lb, &me_a, &la.meta);
        // Each now knows both friends and the other device.
        for (d, other) in [(&a, &me_b), (&b, &me_a)] {
            let all = friends::load(d);
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(f1.as_str())));
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(f2.as_str())));
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(other.as_str()) && f.account_pub.as_deref() == Some(account.as_str())));
        }
        // Chat anti-entropy: A's lists vs B's summaries, both ways.
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        let lists_from_b = differing_lists(&b, &lb, &summaries(&a, &la));
        let (to_b, a_wants) = plan(&a, &la, &lists_from_b);
        let to_b = serde_json::to_value(&to_b).unwrap();
        assert!(apply_messages(&b, &to_b) >= 2);
        let back = wanted_messages(&b, &lb, &serde_json::to_value(&a_wants).unwrap());
        assert!(apply_messages(&a, &serde_json::to_value(&back).unwrap()) >= 1);
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        assert_eq!(summaries(&a, &la), summaries(&b, &lb), "threads converge");
        // A copy that was still "sending" on A reads "sent" on B (no double send).
        let fb1 = friends::load(&b).into_iter().find(|f| f.endpoint_id.as_deref() == Some(f1.as_str())).unwrap();
        let m = chat::messages(&b, &fb1.id).into_iter().find(|m| m.id == "1").unwrap();
        assert_eq!(m.status.as_deref(), Some("sent"));
        assert!(chat::outbox(&b).is_empty());
        // Nothing left to exchange.
        assert!(differing_lists(&b, &lb, &summaries(&a, &la)).is_empty());
        for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn edits_win_by_rev_and_status_only_moves_forward() {
        let d = dir();
        let mut m = text("x", "p", true, 5);
        m.status = Some("read".into());
        chat::append(&d, &m);
        let mut older = m.clone();
        older.status = Some("delivered".into());
        older.text = "stale".into();
        assert_eq!(chat::merge_synced(&d, "p", vec![older]), 0);
        let mut newer = m.clone();
        newer.text = "edited".into();
        newer.edited = true;
        newer.rev = 99;
        assert_eq!(chat::merge_synced(&d, "p", vec![newer]), 1);
        let got = chat::messages(&d, "p").remove(0);
        assert_eq!(got.text, "edited");
        assert_eq!(got.status.as_deref(), Some("read"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn removed_friend_and_removed_device_propagate() {
        let (a, b) = (dir(), dir());
        let key = iroh::SecretKey::generate();
        let account = hex::encode(key.public().as_bytes());
        for d in [&a, &b] { crate::link::adopt_key_for_tests(d, &key); }
        let (me_a, me_b, f1, phone) = (eid(), eid(), eid(), eid());
        let fa = friends::upsert_by_endpoint(&a, &f1, "Mong");
        friends::upsert_by_endpoint(&b, &f1, "Mong");
        friends::upsert_own_device(&a, &phone, "Phone", Some("phone"), Some("ios"), &account, 1);
        friends::upsert_own_device(&b, &phone, "Phone", Some("phone"), Some("ios"), &account, 1);
        // A removes the friend and the phone.
        std::thread::sleep(Duration::from_millis(5));
        record_friend_removed(&a, &fa);
        friends::remove(&a, &fa.id).unwrap();
        with_book(&a, &account, |bk| { bk.removed_devices.insert(phone.clone(), chat::now_ms()); });
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        apply_meta(&b, &lb, &me_a, &la.meta);
        let all = friends::load(&b);
        assert!(!all.iter().any(|f| f.endpoint_id.as_deref() == Some(f1.as_str())), "friend removal propagates");
        assert!(!all.iter().any(|f| f.endpoint_id.as_deref() == Some(phone.as_str())), "device removal propagates");
        // The removed phone itself learns it left the account.
        let p = dir();
        crate::link::adopt_key_for_tests(&p, &key);
        let lp = local(&p, &account, &phone);
        let applied = apply_meta(&p, &lp, &me_a, &la.meta);
        assert!(applied.left_account);
        assert!(my_pub(&p).is_none());
        for d in [a, b, p] { let _ = std::fs::remove_dir_all(d); }
    }
}
