//! The Transfer Server itself: a desktop DropBeam that holds sealed items for
//! people who are offline and hands them over when they come back.
//!
//! Storage (under the chosen root, default `<config>/transfer-server/`):
//! ```text
//! .dropbeam-server-marker            mount identity; missing/wrong = fail closed
//! receipts.jsonl                     item id → final state (append-only, 30 days, compacted hourly)
//! items/<id[..2]>/<id>/header.json   the sealed envelope, exactly as received
//! items/<id[..2]>/<id>/item.json     the server's record (who, to whom, size, expiry)
//! items/<id[..2]>/<id>/payload.part  ciphertext while uploading (resumable)
//! items/<id[..2]>/<id>/payload       ciphertext once complete
//! ```
//! Every item is self-describing (`item.json` next to its bytes), so there is
//! no single index file whose corruption could orphan the store: startup
//! rebuilds the in-memory index by scanning, and deletes anything incomplete
//! or unreadable. No user filename or content ever touches this disk in clear.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::seal;
use crate::iroh_net::{read_frame_cap, write_frame};

pub const DAY_MS: u64 = 24 * 3600 * 1000;
/// Receipts (id + final state only) are kept this long for senders to poll.
const RECEIPT_MS: u64 = 30 * DAY_MS;
const RECEIPT_MAX: usize = 50_000;
/// An upload nobody resumed for this long is abandoned.
const STALE_UPLOAD_MS: u64 = 7 * DAY_MS;
/// An upload nobody has touched for this long is abandoned: free its space
/// (long enough for a laptop that slept overnight to come back and resume).
const IDLE_UPLOAD_MS: u64 = 48 * 3600 * 1000;
/// Quarantined (unreadable at startup) items are kept this long for recovery.
const QUARANTINE_MS: u64 = 30 * DAY_MS;
const MAX_ITEMS_PER_RECIPIENT: usize = 2000;
/// Items one (non-owner) person may leave waiting for one device.
const MAX_ITEMS_PER_PAIR: usize = 500;
const MAX_UPLOADS_PER_PERSON: usize = 2;
const HEADER_MAX: usize = 256 * 1024;

fn d14() -> u32 { 14 }
fn d30() -> u32 { 30 }
fn d20g() -> u64 { 20 * 1_000_000_000 }
fn all() -> String { "all".into() }

/// Owner settings for this device's Transfer Server (`transfer-server.json`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub paused: bool,
    /// What friends see ("Linux Box").
    #[serde(default)]
    pub name: String,
    /// Storage folder; empty = `<config>/transfer-server`.
    #[serde(default)]
    pub root: String,
    /// Random token written to `<root>/.dropbeam-server-marker`.
    #[serde(default)]
    pub marker: String,
    /// Total storage limit in bytes (0 = not set up).
    #[serde(default)]
    pub cap_bytes: u64,
    #[serde(default = "d14")]
    pub file_days: u32,
    #[serde(default = "d30")]
    pub chat_days: u32,
    #[serde(default = "d20g")]
    pub item_max: u64,
    /// "me" | "chosen" | "all".
    #[serde(default = "all")]
    pub access: String,
    /// Friend (person) ids allowed when access == "chosen".
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Friend (person) ids who may also send to people who don't use this server.
    #[serde(default)]
    pub through: Vec<String>,
    /// Friend (person) ids the owner removed.
    #[serde(default)]
    pub denied: Vec<String>,
    /// The account that owns this server when this device isn't linked to one
    /// (picked on this device). Its devices manage the server and vouch for
    /// the owner's friends (see `members`).
    #[serde(default)]
    pub owner_account: String,
    /// Refuse new items when the disk has less than this free (0 = 5 GB / 5%).
    #[serde(default)]
    pub min_free: u64,
    /// Push relay URL override (empty = the DropBeam relay).
    #[serde(default)]
    pub push_url: String,
    /// Fixed UDP port for direct connections (0 = automatic). Applies at start.
    #[serde(default)]
    pub udp_port: u16,
    #[serde(default)]
    pub created_ms: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        serde_json::from_value(json!({})).expect("defaults")
    }
}

fn config_path(config: &Path) -> PathBuf {
    config.join("transfer-server.json")
}

pub fn load_config(config: &Path) -> ServerConfig {
    match crate::settings::read_json_store(&config_path(config)) {
        crate::settings::StoreRead::Loaded(c) => c,
        _ => ServerConfig::default(),
    }
}

pub fn save_config(config: &Path, c: &ServerConfig) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(c)?;
    crate::settings::write_atomic_with_backup(&config_path(config), &bytes, true)?;
    Ok(())
}

pub fn hosting_supported() -> bool {
    !cfg!(any(target_os = "ios", target_os = "android"))
}

pub fn default_root(config: &Path) -> PathBuf {
    config.join("transfer-server")
}

/// Resolve and verify the storage root. A custom root must still carry OUR
/// marker: an unmounted NAS leaves an empty mountpoint behind, and writing
/// there would silently fill the system disk.
pub fn root(config: &Path, c: &ServerConfig) -> Result<PathBuf> {
    let root = if c.root.trim().is_empty() { default_root(config) } else { PathBuf::from(&c.root) };
    if c.root.trim().is_empty() {
        std::fs::create_dir_all(&root)?;
    }
    let marker = std::fs::read_to_string(root.join(".dropbeam-server-marker"))
        .context("The storage folder isn't available (is the drive connected?)")?;
    anyhow::ensure!(!c.marker.is_empty() && marker.trim() == c.marker, "The storage folder changed — choose it again in Settings");
    Ok(root)
}

/// Prepare `root` as server storage: create it, write a fresh marker.
pub fn init_root(config: &Path, c: &mut ServerConfig) -> Result<PathBuf> {
    let root = if c.root.trim().is_empty() { default_root(config) } else { PathBuf::from(&c.root) };
    std::fs::create_dir_all(&root)?;
    if !c.root.trim().is_empty() {
        crate::locations::validate_root(config, &root)?;
    }
    let existing = std::fs::read_to_string(root.join(".dropbeam-server-marker")).ok();
    let token = match existing {
        Some(t) if !t.trim().is_empty() && (c.marker.is_empty() || c.marker == t.trim()) => t.trim().to_owned(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    crate::settings::write_atomic(&root.join(".dropbeam-server-marker"), token.as_bytes())?;
    std::fs::create_dir_all(root.join("items"))?;
    c.marker = token;
    Ok(root)
}

/// One stored item, as recorded next to its bytes.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    /// Depositor = sender endpoint (the envelope's `from`).
    pub from: String,
    /// Usage bucket: the depositor's person id, or "own" for this account.
    pub person: String,
    pub to: Vec<String>,
    pub kind: String,
    pub ct_size: u64,
    pub header_sha: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    /// "uploading" | "held".
    pub state: String,
    /// Sealed notification previews per recipient device (phase 2).
    #[serde(default)]
    pub push: HashMap<String, String>,
    /// Header bytes (counted against quotas along with the payload).
    #[serde(default)]
    pub hbytes: u64,
    /// Recipient devices that turned this item down (it stays for the others).
    #[serde(default)]
    pub refused: Vec<String>,
    /// Recipient devices that already took a FILE item. A file goes to every
    /// device it was sealed for, so it stays until each one has it (or turned
    /// it down, or it expires); chat/op items finish on the first ack because a
    /// person's devices sync their conversation among themselves.
    #[serde(default)]
    pub delivered: Vec<String>,
    /// The sender asked for every sealed-for device to get it. Items from older
    /// senders (sealed for a whole person, "whoever fetches first") finish on
    /// the first ack as they always did.
    #[serde(default)]
    pub all_devices: bool,
}

impl Item {
    /// Devices that still have to take this item.
    fn waiting_for(&self, eid: &str) -> bool {
        self.to.iter().any(|t| t == eid) && !self.refused.iter().any(|r| r == eid) && !self.delivered.iter().any(|d| d == eid)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Receipt {
    state: String,
    at: u64,
    from: String,
    /// Which recipient devices took it (a file goes to each of them).
    #[serde(default)]
    delivered_to: Vec<String>,
}

struct Store {
    root: PathBuf,
    items: HashMap<String, Item>,
    receipts: HashMap<String, Receipt>,
    uploading: HashSet<String>,
    /// Depositor asked to cancel while its upload was still streaming.
    cancel_requested: HashSet<String>,
}

static STORES: Mutex<Option<HashMap<PathBuf, Store>>> = Mutex::new(None);

fn item_dir(root: &Path, id: &str) -> PathBuf {
    root.join("items").join(&id[..2]).join(id)
}

fn now() -> u64 {
    crate::chat::now_ms()
}

fn read_receipts(root: &Path) -> HashMap<String, Receipt> {
    let mut out = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(root.join("receipts.jsonl")) {
        for line in text.lines() {
            #[derive(Deserialize)]
            struct Line { id: String, state: String, at: u64, from: String, #[serde(default)] delivered_to: Vec<String> }
            if let Ok(l) = serde_json::from_str::<Line>(line) {
                out.insert(l.id, Receipt { state: l.state, at: l.at, from: l.from, delivered_to: l.delivered_to });
            }
        }
    }
    out
}

/// What a startup scan may do with an item folder it can't load.
#[derive(Debug, PartialEq, Eq)]
enum Unloadable {
    /// Provably finished or never written: safe to delete.
    Delete,
    /// Couldn't be READ (I/O error — a NAS waking up, a permissions hiccup):
    /// never delete a possibly-held item on a read error. Set aside instead.
    Quarantine,
}

/// Classify an item folder whose record didn't load cleanly.
fn unloadable(path: &Path, finished: bool) -> Unloadable {
    if finished { return Unloadable::Delete; }
    let io_error = |p: PathBuf| matches!(std::fs::metadata(&p), Err(e) if e.kind() != std::io::ErrorKind::NotFound);
    match std::fs::read(path.join("item.json")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Unloadable::Quarantine,
        _ if io_error(path.join("payload")) || io_error(path.join("header.json")) => Unloadable::Quarantine,
        // A complete payload with an unreadable/garbled record is still someone's file.
        _ if std::fs::metadata(path.join("payload")).is_ok_and(|m| m.len() > 0) => Unloadable::Quarantine,
        _ => Unloadable::Delete,
    }
}

fn quarantine(root: &Path, path: &Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = root.join("quarantine").join(format!("{name}-{}", now()));
    let moved = std::fs::create_dir_all(root.join("quarantine")).and_then(|_| std::fs::rename(path, &target));
    match moved {
        Ok(()) => log::warn!("transfer-server: an item couldn't be read at startup; set aside in quarantine (not deleted)"),
        Err(_) => log::warn!("transfer-server: an item couldn't be read at startup; left in place (not deleted)"),
    }
}

fn load_store(root: &Path) -> Store {
    let receipts = read_receipts(root);
    let mut items = HashMap::new();
    let base = root.join("items");
    for shard in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        for dir in std::fs::read_dir(shard.path()).into_iter().flatten().flatten() {
            let path = dir.path();
            let record: Option<Item> = std::fs::read(path.join("item.json")).ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let finished = record.as_ref().is_some_and(|it| receipts.contains_key(&it.id))
                || path.file_name().is_some_and(|n| receipts.contains_key(n.to_string_lossy().as_ref()));
            let keep = record.filter(|it| {
                let name_ok = path.file_name().is_some_and(|n| n.to_string_lossy() == it.id);
                let bytes_ok = match it.state.as_str() {
                    "held" => std::fs::metadata(path.join("payload")).map(|m| m.len() == it.ct_size).unwrap_or(false),
                    "uploading" => true,
                    _ => false,
                };
                // Already finished (a delete that failed before a restart): gone.
                name_ok && bytes_ok && path.join("header.json").is_file() && !receipts.contains_key(&it.id)
            });
            match keep {
                Some(it) => {
                    items.insert(it.id.clone(), it);
                }
                None => match unloadable(&path, finished) {
                    Unloadable::Delete => {
                        log::warn!("transfer-server: removing an incomplete item folder");
                        let _ = std::fs::remove_dir_all(&path);
                    }
                    Unloadable::Quarantine => quarantine(root, &path),
                },
            }
        }
    }
    Store { root: root.to_path_buf(), items, receipts, uploading: HashSet::new(), cancel_requested: HashSet::new() }
}

/// Run `f` against this config's store (loading/reloading it as needed).
fn with_store<T>(config: &Path, f: impl FnOnce(&mut Store) -> T) -> Result<T> {
    let c = load_config(config);
    let root = root(config, &c)?;
    let mut guard = STORES.lock().unwrap_or_else(|p| p.into_inner());
    let stores = guard.get_or_insert_with(HashMap::new);
    let reload = stores.get(config).is_none_or(|s| s.root != root);
    if reload {
        stores.insert(config.to_path_buf(), load_store(&root));
    }
    Ok(f(stores.get_mut(config).expect("inserted above")))
}

fn save_item(root: &Path, it: &Item) -> Result<()> {
    let dir = item_dir(root, &it.id);
    std::fs::create_dir_all(&dir)?;
    crate::settings::write_atomic(&dir.join("item.json"), &serde_json::to_vec(it)?)?;
    Ok(())
}

/// Rewrite the receipts ledger compactly (hourly, from `gc`).
fn compact_receipts(s: &Store) {
    let mut text = String::new();
    for (id, r) in &s.receipts {
        if let Ok(line) = serde_json::to_string(&json!({"id": id, "state": r.state, "at": r.at, "from": r.from, "delivered_to": r.delivered_to})) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    if let Err(e) = crate::settings::write_atomic(&s.root.join("receipts.jsonl"), text.as_bytes()) {
        log::warn!("transfer-server: cannot save receipts: {e}");
    }
}

/// Append one final state (cheap, durable) — the sender's proof of what happened.
fn append_receipt(root: &Path, id: &str, r: &Receipt) {
    use std::io::Write;
    let line = json!({"id": id, "state": r.state, "at": r.at, "from": r.from, "delivered_to": r.delivered_to}).to_string();
    let result = std::fs::OpenOptions::new().create(true).append(true).open(root.join("receipts.jsonl"))
        .and_then(|mut f| { f.write_all(format!("{line}\n").as_bytes())?; f.sync_data() });
    if let Err(e) = result {
        log::warn!("transfer-server: cannot record a receipt: {e}");
    }
}

/// Remove an item's bytes + record and remember how it ended.
fn finish_item(s: &mut Store, id: &str, state: &str) {
    if let Some(it) = s.items.remove(id) {
        // Receipt first: if the delete below fails (file busy, NAS hiccup), the
        // next load sees the receipt and finishes the job instead of re-serving it.
        let r = Receipt { state: state.into(), at: now(), from: it.from, delivered_to: it.delivered };
        append_receipt(&s.root, id, &r);
        s.receipts.insert(id.to_owned(), r);
        if let Err(e) = std::fs::remove_dir_all(item_dir(&s.root, id)) {
            log::warn!("transfer-server: couldn't delete a finished item yet: {e}");
        }
        s.cancel_requested.remove(id);
        if s.receipts.len() > RECEIPT_MAX {
            let mut by_age: Vec<_> = s.receipts.iter().map(|(k, r)| (r.at, k.clone())).collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(s.receipts.len() - RECEIPT_MAX) {
                s.receipts.remove(&k);
            }
        }
    }
}

fn used_bytes(s: &Store) -> u64 {
    s.items.values().map(|i| i.ct_size + i.hbytes).sum()
}

// ── access control ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Rights {
    /// One of this account's own devices: full use.
    pub own: bool,
    /// May leave items for other people who use this server.
    pub member: bool,
    /// May also leave items for anyone at all.
    pub through: bool,
    /// One of the owner's devices (it may share this server with its friends).
    #[serde(default)]
    pub owner: bool,
}

impl Rights {
    pub fn any(&self) -> bool {
        self.own || self.member
    }
}

/// The person id an endpoint uses this server as: a friend of this device, or
/// a friend of the owner that an owner device vouched for ("v:…"). None = neither.
fn person_of(config: &Path, c: &ServerConfig, eid: &str) -> Option<String> {
    crate::friends::chat_sender(config, eid).map(|f| f.id)
        .or_else(|| super::members::vouched(config, c, eid).map(|v| v.person))
}

pub fn rights_for(config: &Path, c: &ServerConfig, eid: &str) -> Rights {
    if super::members::is_owner_device(config, c, eid) {
        return Rights { own: true, member: true, through: true, owner: true };
    }
    let Some(person) = person_of(config, c, eid) else { return Rights::default() };
    // "e:<device>" = a removed friend-of-the-owner's device (their person id
    // may differ between the owner's devices, their devices can't).
    if c.denied.contains(&person) || c.denied.iter().any(|d| d.strip_prefix("e:") == Some(eid)) {
        return Rights::default();
    }
    let member = match c.access.as_str() {
        "all" => true,
        "chosen" => c.allowed.contains(&person),
        _ => false,
    };
    Rights { own: false, member, through: member && c.through.contains(&person), owner: false }
}

/// A device that may no longer collect anything: removed from this account, or
/// that exact device denied by the owner. (Denying a PERSON only stops their
/// deposits — what is held FOR them still reaches them.)
fn removed_device(config: &Path, c: &ServerConfig, eid: &str) -> bool {
    crate::account::device_was_removed(config, eid) || c.denied.iter().any(|d| d.strip_prefix("e:") == Some(eid))
}

/// Whether items may be left for this device without "send through".
fn is_member_device(config: &Path, c: &ServerConfig, eid: &str) -> bool {
    rights_for(config, c, eid).any()
}

/// What this server tells a peer about itself in hellos (None = nothing to say).
pub fn grant_for(config: &Path, eid: &str) -> Option<Value> {
    if !hosting_supported() {
        return None;
    }
    let c = load_config(config);
    if !c.enabled {
        return None;
    }
    let r = rights_for(config, &c, eid);
    if !r.any() {
        return None;
    }
    Some(json!({"name": server_name(&c), "own": r.own, "member": r.member, "through": r.through, "paused": c.paused,
        "owner": r.owner, "access": c.access}))
}

pub fn server_name(c: &ServerConfig) -> String {
    if c.name.trim().is_empty() { "Transfer Server".into() } else { c.name.trim().to_owned() }
}

// ── rate limiting ───────────────────────────────────────────────────────────

fn rate_ok(eid: &str) -> bool {
    static RL: Mutex<Option<HashMap<String, (Instant, u32)>>> = Mutex::new(None);
    let mut g = RL.lock().unwrap_or_else(|p| p.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() > 4096 {
        map.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(10));
    }
    let e = map.entry(eid.to_owned()).or_insert((Instant::now(), 0));
    if e.0.elapsed() > Duration::from_secs(2) {
        *e = (Instant::now(), 0);
    }
    e.1 += 1;
    e.1 <= 40
}

fn refuse(reason: &str) -> Value {
    json!({"ok": false, "reason": reason})
}

// ── protocol handlers ───────────────────────────────────────────────────────

pub async fn serve(config: &Path, me: Option<&str>, conn: &Connection, send: &mut SendStream, recv: &mut RecvStream, kind: &str, req: &Value) -> Result<()> {
    let who = conn.remote_id().to_string();
    if !rate_ok(&who) {
        write_frame(send, &refuse("busy")).await?;
        let _ = send.finish();
        return Ok(());
    }
    let c = load_config(config);
    if !hosting_supported() || !c.enabled {
        write_frame(send, &refuse("off")).await?;
        let _ = send.finish();
        return Ok(());
    }
    match kind {
        "mailbox.deposit" => serve_deposit(config, &c, &who, me, send, recv, req).await?,
        "mailbox.get" if removed_device(config, &c, &who) => write_frame(send, &refuse("gone")).await?,
        "mailbox.get" => serve_get(config, &who, send, req).await?,
        "mailbox.fetch" | "mailbox.ack" if removed_device(config, &c, &who) => {
            // A device removed from the account (or a denied person's device)
            // can't collect what is still held here.
            write_frame(send, &refuse("denied")).await?;
        }
        other => {
            let (config, other, who, req, me) = (config.to_path_buf(), other.to_owned(), who.clone(), req.clone(), me.map(str::to_owned));
            // Store work touches the disk (a slow NAS): never on an async worker.
            let reply = tokio::task::spawn_blocking(move || {
                let config = config.as_path();
                match other.as_str() {
                    "mailbox.hello" => hello_reply(config, &c, &who),
                    "mailbox.cancel" => cancel(config, &who, &req),
                    "mailbox.status" => status_reply(config, &who, &req),
                    "mailbox.fetch" => fetch_reply(config, &who),
                    "mailbox.ack" => ack(config, &who, &req),
                    "mailbox.push-register" => super::push::register(config, &c, &who, &req),
                    "mailbox.members" => super::members::receive(config, &c, me.as_deref(), &who, &req),
                    _ => refuse("unknown"),
                }
            }).await?;
            write_frame(send, &reply).await?;
        }
    }
    let _ = send.finish();
    Ok(())
}

fn quota_numbers(config: &Path, c: &ServerConfig, s: &Store, person: &str, own: bool) -> Value {
    let used = used_bytes(s);
    let mine: u64 = s.items.values().filter(|i| i.person == person).map(|i| i.ct_size + i.hbytes).sum();
    let per_user = if own { c.cap_bytes } else { c.cap_bytes / 4 };
    let _ = config;
    json!({"used": used, "cap": c.cap_bytes, "user_used": mine, "user_cap": per_user, "item_max": c.item_max})
}

fn hello_reply(config: &Path, c: &ServerConfig, who: &str) -> Value {
    let r = rights_for(config, c, who);
    if !r.any() {
        return refuse("denied");
    }
    let person = if r.own { "own".to_owned() } else { person_of(config, c, who).unwrap_or_default() };
    let quota = with_store(config, |s| quota_numbers(config, c, s, &person, r.own)).unwrap_or(json!(null));
    json!({"ok": true, "v": super::VERSION, "name": server_name(c), "paused": c.paused, "access": c.access,
        "rights": r, "quota": quota, "expiry": {"file_days": c.file_days, "chat_days": c.chat_days},
        "push": super::push::configured(),
        // Files wait for every device they were sealed for (see Item::delivered).
        "per_device": true})
}

fn header_sha(header: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(header).unwrap_or_default()))
}

/// Keep this much of the disk free: 5 GB, or 5% on a small disk (min 1 GB).
fn free_floor(c: &ServerConfig, total: u64) -> u64 {
    if c.min_free > 0 { c.min_free } else { (5 * 1_000_000_000u64).min(total / 20).max(1_000_000_000) }
}

/// Validate + reserve a deposit. Ok((item, have)) or Err(reply to send).
fn admit(config: &Path, c: &ServerConfig, who: &str, me: Option<&str>, req: &Value) -> std::result::Result<(Item, u64, PathBuf), Value> {
    if c.paused {
        return Err(refuse("paused"));
    }
    // The server's own app leaving something here (it sends a chat while it
    // is the Transfer Server itself): it owns the place.
    let rights = if Some(who) == me { Rights { own: true, member: true, through: true, owner: true } } else { rights_for(config, c, who) };
    if !rights.any() {
        return Err(refuse("denied"));
    }
    let header = req.get("header").cloned().unwrap_or(Value::Null);
    if serde_json::to_vec(&header).map(|b| b.len()).unwrap_or(usize::MAX) > HEADER_MAX {
        return Err(refuse("invalid"));
    }
    let env: seal::Envelope = serde_json::from_value(header.clone()).map_err(|_| refuse("invalid"))?;
    if env.validate().is_err() || env.from != who || !env.verify_sig() {
        return Err(refuse("invalid"));
    }
    let to = env.to();
    if to.iter().any(|t| Some(t.as_str()) == me) {
        return Err(json!({"ok": false, "reason": "recipient", "not_members": [me]}));
    }
    if !rights.through {
        let outside: Vec<&String> = to.iter().filter(|t| !is_member_device(config, c, t)).collect();
        if !outside.is_empty() {
            return Err(json!({"ok": false, "reason": "recipient", "not_members": outside}));
        }
    }
    let ct_size = env.ct_size();
    if env.kind != "file" && ct_size != 0 {
        return Err(refuse("invalid"));
    }
    if env.size > c.item_max {
        return Err(refuse("too_big"));
    }
    let person = if rights.own { "own".to_owned() } else { person_of(config, c, who).unwrap_or_else(|| who.to_owned()) };
    let sha = header_sha(&header);
    let days = if env.kind == "file" { c.file_days } else { c.chat_days }.clamp(1, 90) as u64;
    let push: HashMap<String, String> = req["push"].as_object().map(|o| {
        o.iter().filter(|(k, _)| to.contains(k)).filter_map(|(k, v)| Some((k.clone(), v.as_str()?.chars().take(4096).collect())))
            .collect()
    }).unwrap_or_default();
    with_store(config, |s| {
        if let Some(existing) = s.items.get(&env.item_id) {
            if existing.from != who || existing.header_sha != sha {
                return Err(refuse("conflict"));
            }
            if existing.state == "held" {
                return Err(json!({"ok": true, "have": ct_size, "state": "held", "held_until": existing.expires_ms}));
            }
            if s.uploading.contains(&env.item_id) {
                return Err(refuse("busy"));
            }
            let existing = existing.clone();
            let part = item_dir(&s.root, &env.item_id).join("payload.part");
            let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0).min(ct_size);
            let have = (len / seal::CT_SEG) * seal::CT_SEG;
            s.uploading.insert(env.item_id.clone());
            return Ok((existing, have, s.root.clone()));
        }
        if let Some(r) = s.receipts.get(&env.item_id) {
            // Already finished once: never resurrect it. For its own sender this
            // is a lost "held" reply — say so, and the receipt poll does the rest.
            return Err(if r.from == who {
                json!({"ok": true, "state": "held", "held_until": 0, "finished": r.state})
            } else {
                refuse("conflict")
            });
        }
        // Quotas: total, per person, free-space floor, per recipient, concurrency.
        let hbytes = serde_json::to_vec(&header).map(|b| b.len() as u64).unwrap_or(0);
        let need = ct_size + hbytes;
        let used = used_bytes(s);
        if c.cap_bytes == 0 || used.saturating_add(need) > c.cap_bytes {
            return Err(refuse("full"));
        }
        let mine: u64 = s.items.values().filter(|i| i.person == person).map(|i| i.ct_size + i.hbytes).sum();
        if !rights.own && mine.saturating_add(need) > c.cap_bytes / 4 {
            return Err(refuse("user_quota"));
        }
        let pending_writes: u64 = s.items.values().filter(|i| i.state == "uploading").map(|i| i.ct_size).sum();
        if let Some((free, total)) = crate::locations::volume_bytes(&s.root) {
            if free.saturating_sub(pending_writes).saturating_sub(ct_size) < free_floor(c, total) {
                return Err(refuse("full"));
            }
        }
        for t in &to {
            if s.items.values().filter(|i| i.to.contains(t)).count() >= MAX_ITEMS_PER_RECIPIENT {
                return Err(refuse("recipient_full"));
            }
            // One depositor can't fill someone's whole allowance by itself.
            if !rights.own && s.items.values().filter(|i| i.person == person && i.to.contains(t)).count() >= MAX_ITEMS_PER_PAIR {
                return Err(refuse("recipient_full"));
            }
        }
        let active = s.items.values().filter(|i| i.person == person && s.uploading.contains(&i.id)).count();
        if active >= MAX_UPLOADS_PER_PERSON {
            return Err(refuse("busy"));
        }
        let created = now();
        let item = Item {
            id: env.item_id.clone(), from: who.to_owned(), person: person.clone(), to: to.clone(),
            kind: env.kind.clone(), ct_size, header_sha: sha.clone(), created_ms: created,
            expires_ms: created + days * DAY_MS, state: "uploading".into(), push: push.clone(),
            hbytes, refused: vec![], delivered: vec![],
            all_devices: req["all_devices"].as_bool().unwrap_or(false),
        };
        let dir = item_dir(&s.root, &item.id);
        let write = (|| -> Result<()> {
            std::fs::create_dir_all(&dir)?;
            crate::settings::write_atomic(&dir.join("header.json"), &serde_json::to_vec(&header)?)?;
            save_item(&s.root, &item)
        })();
        if let Err(e) = write {
            log::warn!("transfer-server: cannot store a new item: {}", crate::telemetry::redact_paths_only(&format!("{e:#}")));
            let _ = std::fs::remove_dir_all(&dir);
            return Err(refuse("storage"));
        }
        s.items.insert(item.id.clone(), item.clone());
        s.uploading.insert(item.id.clone());
        Ok((item, 0, s.root.clone()))
    })
    .unwrap_or_else(|e| {
        log::warn!("transfer-server: storage unavailable: {}", crate::telemetry::redact_paths_only(&format!("{e:#}")));
        Err(refuse("storage"))
    })
}

/// A running upload's "stop" bell: a re-deposit of the same item by the same
/// sender (its connection blipped and it came back) takes over at once instead
/// of being told "busy" while the dead stream waits out its read timeout.
static STOPS: Mutex<Option<HashMap<String, Arc<tokio::sync::Notify>>>> = Mutex::new(None);
fn stop_bell(id: &str) -> Arc<tokio::sync::Notify> {
    STOPS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new)
        .entry(id.to_owned()).or_default().clone()
}

/// If `who` is already uploading this exact item, stop that stale upload and
/// wait (bounded) for it to let go.
async fn supersede_stale_upload(config: &Path, who: &str, req: &Value) {
    let Some(id) = req["header"]["item_id"].as_str().map(str::to_owned) else { return };
    let busy = || with_store(config, |s| s.uploading.contains(&id) && s.items.get(&id).is_some_and(|it| it.from == who)).unwrap_or(false);
    if !busy() { return; }
    log::info!("transfer-server: the sender came back for an upload still marked in progress; taking over");
    stop_bell(&id).notify_waiters();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !busy() { return; }
        stop_bell(&id).notify_waiters();
    }
}

struct UploadGuard<'a> {
    config: &'a Path,
    id: String,
}
impl Drop for UploadGuard<'_> {
    fn drop(&mut self) {
        let _ = with_store(self.config, |s| s.uploading.remove(&self.id));
        STOPS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).remove(&self.id);
    }
}

async fn serve_deposit(config: &Path, c: &ServerConfig, who: &str, me: Option<&str>, send: &mut SendStream, recv: &mut RecvStream, req: &Value) -> Result<()> {
    supersede_stale_upload(config, who, req).await;
    let admitted = {
        let (config, c, who, me, req) = (config.to_path_buf(), c.clone(), who.to_owned(), me.map(str::to_owned), req.clone());
        // Disk work (a sleeping NAS can take seconds) off the async workers.
        tokio::task::spawn_blocking(move || admit(&config, &c, &who, me.as_deref(), &req)).await?
    };
    let (item, have, root) = match admitted {
        Ok(v) => v,
        Err(reply) => {
            write_frame(send, &reply).await?;
            return Ok(());
        }
    };
    let _guard = UploadGuard { config, id: item.id.clone() };
    let bell = stop_bell(&item.id);
    write_frame(send, &json!({"ok": true, "have": have})).await?;
    let dir = item_dir(&root, &item.id);
    let part = dir.join("payload.part");
    let result: Result<()> = async {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let mut f = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part).await?;
        f.set_len(have).await?;
        f.seek(std::io::SeekFrom::Start(have)).await?;
        let mut remaining = item.ct_size - have;
        let mut buf = vec![0u8; 256 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(90), recv.read(&mut buf[..want])) =>
                    r.context("upload stalled")??.context("upload ended early")?,
                _ = bell.notified() => anyhow::bail!("superseded by the sender's new connection"),
            };
            f.write_all(&buf[..n]).await?;
            remaining -= n as u64;
        }
        f.sync_all().await?;
        drop(f);
        tokio::fs::rename(&part, dir.join("payload")).await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        // Canceled while it streamed: release its quota NOW, not after 7 days.
        let canceled = with_store(config, |s| {
            let asked = s.cancel_requested.remove(&item.id);
            if asked { finish_item(s, &item.id, "canceled"); }
            asked
        }).unwrap_or(false);
        if canceled {
            log::info!("transfer-server: a canceled upload stopped; its space is free again");
            return Ok(());
        }
        log::info!("transfer-server: upload interrupted (resumable): {e:#}");
        let _ = write_frame(send, &refuse("interrupted")).await;
        return Ok(());
    }
    let held = with_store(config, |s| {
        if s.cancel_requested.remove(&item.id) {
            finish_item(s, &item.id, "canceled");
            return None;
        }
        let Some(it) = s.items.get_mut(&item.id) else { return None };
        it.state = "held".into();
        let copy = it.clone();
        save_item(&s.root, &copy).ok().map(|_| copy)
    })?;
    let Some(held) = held else {
        write_frame(send, &refuse("storage")).await?;
        return Ok(());
    };
    log::info!("transfer-server: holding a {} item ({} bytes) for {} device(s)", held.kind, held.ct_size, held.to.len());
    write_frame(send, &json!({"ok": true, "state": "held", "held_until": held.expires_ms})).await?;
    super::push::on_stored(config, &held);
    wake_delivery();
    Ok(())
}

/// This device's own app leaves a chat/op item on its own Transfer Server (no
/// network hop: an endpoint can't dial itself). Same checks and bookkeeping
/// as a `mailbox.deposit` from a member. Returns when it's held until.
pub fn deposit_local(config: &Path, me: &str, req: &Value) -> std::result::Result<u64, Value> {
    let c = load_config(config);
    if !hosting_supported() || !c.enabled {
        return Err(refuse("off"));
    }
    let (item, _, root) = match admit(config, &c, me, Some(me), req) {
        Ok(v) => v,
        // Already held (a retry of the same item).
        Err(r) if r["ok"].as_bool() == Some(true) => return Ok(r["held_until"].as_u64().unwrap_or(0)),
        Err(r) => return Err(r),
    };
    let _guard = UploadGuard { config, id: item.id.clone() };
    if item.ct_size != 0 {
        let _ = with_store(config, |s| finish_item(s, &item.id, "rejected"));
        return Err(refuse("invalid"));
    }
    let dir = item_dir(&root, &item.id);
    if std::fs::write(dir.join("payload"), b"").is_err() {
        let _ = with_store(config, |s| finish_item(s, &item.id, "rejected"));
        return Err(refuse("storage"));
    }
    let held = with_store(config, |s| {
        let it = s.items.get_mut(&item.id)?;
        it.state = "held".into();
        let copy = it.clone();
        save_item(&s.root, &copy).ok().map(|_| copy)
    })
    .ok()
    .flatten()
    .ok_or_else(|| refuse("storage"))?;
    log::info!("transfer-server: holding a {} item (from this device) for {} device(s)", held.kind, held.to.len());
    super::push::on_stored(config, &held);
    wake_delivery();
    Ok(held.expires_ms)
}

/// `mailbox.status` / `mailbox.cancel` for this device's own items (local).
pub fn local_rpc(config: &Path, me: &str, req: &Value) -> Value {
    match req["kind"].as_str() {
        Some("mailbox.status") => status_reply(config, me, req),
        Some("mailbox.cancel") => cancel(config, me, req),
        _ => refuse("unknown"),
    }
}

/// Held chat/file items still waiting for `eid` (for a late wake-up push).
pub fn waiting_items(config: &Path, eid: &str) -> Vec<Item> {
    with_store(config, |s| s.items.values().filter(|i| i.state == "held" && i.waiting_for(eid)).cloned().collect())
        .unwrap_or_default()
}

fn cancel(config: &Path, who: &str, req: &Value) -> Value {
    let id = req["item_id"].as_str().unwrap_or("").to_owned();
    with_store(config, |s| {
        match s.items.get(&id) {
            Some(it) if it.from == who => {
                if s.uploading.contains(&id) {
                    // Still streaming: finish the job when the upload stops.
                    s.cancel_requested.insert(id.clone());
                    return json!({"ok": true, "canceled": true});
                }
                // Devices that already took it: an unsend must still reach them.
                let delivered = it.delivered.clone();
                finish_item(s, &id, "canceled");
                json!({"ok": true, "canceled": true, "delivered_to": delivered})
            }
            Some(_) => refuse("denied"),
            None => json!({"ok": true, "canceled": false,
                "state": s.receipts.get(&id).filter(|r| r.from == who).map(|r| r.state.clone())}),
        }
    })
    .unwrap_or_else(|_| refuse("storage"))
}

fn status_reply(config: &Path, who: &str, req: &Value) -> Value {
    let ids: Vec<String> = req["item_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).take(2000).collect()).unwrap_or_default();
    with_store(config, |s| {
        let items: Vec<Value> = ids.iter().map(|id| {
            if let Some(it) = s.items.get(id).filter(|it| it.from == who) {
                json!({"id": id, "state": it.state, "at": it.created_ms, "expires": it.expires_ms, "delivered_to": it.delivered})
            } else if let Some(r) = s.receipts.get(id).filter(|r| r.from == who) {
                json!({"id": id, "state": r.state, "at": r.at, "delivered_to": r.delivered_to})
            } else {
                json!({"id": id, "state": "unknown"})
            }
        }).collect();
        json!({"ok": true, "items": items})
    })
    .unwrap_or_else(|_| refuse("storage"))
}

/// Items waiting for `who`, oldest first. Small (chat/op) items carry their
/// sealed header inline so the recipient needs no second round trip.
fn fetch_reply(config: &Path, who: &str) -> Value {
    with_store(config, |s| {
        let mut mine: Vec<&Item> = s.items.values()
            .filter(|i| i.state == "held" && i.waiting_for(who))
            .collect();
        // Chat/ops first: a message must never wait behind a big file.
        mine.sort_by(|a, b| (a.ct_size > 0, a.created_ms, &a.id).cmp(&(b.ct_size > 0, b.created_ms, &b.id)));
        let mut inline_budget: i64 = 600 * 1024;
        let items: Vec<Value> = mine.iter().take(500).map(|it| {
            let mut v = json!({"item_id": it.id, "kind": it.kind, "from": it.from, "ct_size": it.ct_size, "created_ms": it.created_ms});
            if it.ct_size == 0 && inline_budget > 0 {
                if let Ok(h) = std::fs::read(item_dir(&s.root, &it.id).join("header.json")) {
                    inline_budget -= h.len() as i64;
                    if let Ok(h) = serde_json::from_slice::<Value>(&h) {
                        v["header"] = h;
                    }
                }
            }
            v
        }).collect();
        json!({"ok": true, "items": items, "more": mine.len() > 500, "name": server_name(&load_config(config))})
    })
    .unwrap_or_else(|_| refuse("storage"))
}

async fn serve_get(config: &Path, who: &str, send: &mut SendStream, req: &Value) -> Result<()> {
    let id = req["item_id"].as_str().unwrap_or("").to_owned();
    let have = req["have"].as_u64().unwrap_or(0);
    let found = with_store(config, |s| {
        s.items.get(&id).filter(|it| it.state == "held" && it.waiting_for(who))
            .map(|it| (it.clone(), item_dir(&s.root, &id)))
    })
    .ok()
    .flatten();
    let Some((it, dir)) = found else {
        write_frame(send, &refuse("gone")).await?;
        return Ok(());
    };
    let header: Value = serde_json::from_slice(&std::fs::read(dir.join("header.json"))?)?;
    let have = have.min(it.ct_size);
    write_frame(send, &json!({"ok": true, "header": header, "ct_size": it.ct_size, "from_offset": have})).await?;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut f = tokio::fs::File::open(dir.join("payload")).await?;
    f.seek(std::io::SeekFrom::Start(have)).await?;
    let mut remaining = it.ct_size - have;
    let mut buf = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..want]).await?;
        send.write_all(&buf[..want]).await?;
        remaining -= want as u64;
    }
    Ok(())
}

fn ack(config: &Path, who: &str, req: &Value) -> Value {
    let id = req["item_id"].as_str().unwrap_or("").to_owned();
    let ok = req["ok"].as_bool().unwrap_or(false);
    with_store(config, |s| {
        let Some(it) = s.items.get(&id) else { return json!({"ok": true}) };
        if !it.to.iter().any(|t| t == who) {
            return refuse("denied");
        }
        if ok {
            let mut it = it.clone();
            if !it.delivered.iter().any(|d| d == who) {
                it.delivered.push(who.to_owned());
            }
            // Sent "to every device" (files; chat from senders that fan out
            // per device, like iMessage): it stays until each one has it.
            // Otherwise (older senders) the first device is enough — a person's
            // devices sync their conversation among themselves.
            let done = !it.all_devices || it.to.iter().all(|t| it.delivered.contains(t) || it.refused.contains(t));
            if done {
                s.items.insert(id.clone(), it);
                finish_item(s, &id, "delivered");
            } else {
                log::info!("transfer-server: one of the recipient's devices took an item; holding it for the others");
                let _ = save_item(&s.root, &it);
                s.items.insert(id.clone(), it);
            }
            return json!({"ok": true});
        }
        let reason: String = req["reason"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').take(24).collect();
        log::info!("transfer-server: a recipient device turned an item down ({reason})");
        // One device can't open it; another of theirs may. Only when every
        // addressed device has refused is it gone for good.
        let mut it = it.clone();
        if !it.refused.iter().any(|r| r == who) {
            it.refused.push(who.to_owned());
        }
        if it.to.iter().all(|t| it.refused.contains(t) || it.delivered.contains(t)) {
            let state = if it.delivered.is_empty() { "rejected" } else { "delivered" };
            s.items.insert(id.clone(), it);
            finish_item(s, &id, state);
        } else {
            let _ = save_item(&s.root, &it);
            s.items.insert(id.clone(), it);
        }
        json!({"ok": true})
    })
    .unwrap_or_else(|_| refuse("storage"))
}

// ── maintenance ─────────────────────────────────────────────────────────────

/// Expire overdue items, drop abandoned uploads, age out receipts. Returns how
/// many items were removed.
pub fn gc(config: &Path) -> usize {
    let c = load_config(config);
    if !c.enabled {
        return 0;
    }
    // Stat paused uploads OUTSIDE the store lock (a slow NAS must not stall
    // every other store user): which have had no byte for IDLE_UPLOAD_MS?
    let t = now();
    let paused: Vec<(String, PathBuf, u64)> = with_store(config, |s| s.items.values()
        .filter(|i| i.state == "uploading" && !s.uploading.contains(&i.id) && i.created_ms + IDLE_UPLOAD_MS <= t)
        .map(|i| (i.id.clone(), item_dir(&s.root, &i.id).join("payload.part"), i.created_ms)).collect()).unwrap_or_default();
    let idle: HashSet<String> = paused.into_iter().filter(|(_, part, created)| {
        let touched = std::fs::metadata(part).and_then(|m| m.modified()).ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(*created, |d| d.as_millis() as u64);
        touched.max(*created) + IDLE_UPLOAD_MS <= t
    }).map(|(id, _, _)| id).collect();
    let removed = with_store(config, |s| {
        let expired: Vec<String> = s.items.values()
            .filter(|i| !s.uploading.contains(&i.id))
            .filter(|i| (i.state == "held" && i.expires_ms <= t)
                || (i.state == "uploading" && (i.created_ms + STALE_UPLOAD_MS <= t || idle.contains(&i.id))))
            .map(|i| i.id.clone())
            .collect();
        for id in &expired {
            finish_item(s, id, "expired");
        }
        s.receipts.retain(|_, r| r.at + RECEIPT_MS > t);
        compact_receipts(s);
        // Retry deletes that failed earlier (the receipt already says "done").
        for id in s.receipts.keys() {
            if id.len() >= 2 {
                let dir = item_dir(&s.root, id);
                if dir.exists() && !s.items.contains_key(id) {
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
        }
        expired.len()
    })
    .unwrap_or(0);
    // Quarantined items age out after a month (kept that long for recovery).
    if let Ok(root) = root(config, &c) {
        for e in std::fs::read_dir(root.join("quarantine")).into_iter().flatten().flatten() {
            // Age from when it was SET ASIDE (the "-<ms>" suffix), never the
            // folder's own mtime, which a rename keeps from long before.
            let name = e.file_name().to_string_lossy().into_owned();
            let set_aside = name.rsplit('-').next().and_then(|ms| ms.parse::<u64>().ok());
            let old = set_aside.is_some_and(|at| t.saturating_sub(at) > QUARANTINE_MS);
            if old { let _ = std::fs::remove_dir_all(e.path()); }
        }
    }
    removed
}

/// Recipient devices with items waiting, and how many.
pub fn pending_recipients(config: &Path) -> Vec<(String, usize)> {
    let c = load_config(config);
    if !c.enabled || c.paused {
        return vec![];
    }
    with_store(config, |s| {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for it in s.items.values().filter(|i| i.state == "held") {
            for t in it.to.iter().filter(|t| it.waiting_for(t)) {
                *counts.entry(t.clone()).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    })
    .unwrap_or_default()
}

/// Drop items for/from a removed person. Items they left FOR this account's own
/// devices still deliver (decision 14); everything else they deposited goes.
pub fn remove_person(config: &Path, person: &str) -> usize {
    let c = load_config(config);
    with_store(config, |s| {
        let doomed: Vec<String> = s.items.values()
            .filter(|i| i.person == person && !i.to.iter().all(|t| super::members::is_owner_device(config, &c, t)))
            .map(|i| i.id.clone())
            .collect();
        for id in &doomed {
            if !s.uploading.contains(id) {
                finish_item(s, id, "rejected");
            }
        }
        doomed.len()
    })
    .unwrap_or(0)
}

/// Delete every stored item (the owner's "Delete everything").
pub fn wipe(config: &Path) -> usize {
    with_store(config, |s| {
        let ids: Vec<String> = s.items.keys().filter(|id| !s.uploading.contains(*id)).cloned().collect();
        for id in &ids {
            finish_item(s, id, "rejected");
        }
        ids.len()
    })
    .unwrap_or(0)
}

#[cfg(test)]
pub fn age_all_for_tests(config: &Path, by: u64) {
    let _ = with_store(config, |s| {
        for it in s.items.values_mut() {
            it.created_ms = it.created_ms.saturating_sub(by);
            it.expires_ms = it.expires_ms.saturating_sub(by);
            let _ = save_item(&s.root, it);
        }
    });
}

/// Forget the cached store (after the owner changes the storage folder).
pub fn unload(config: &Path) {
    if let Some(m) = STORES.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        m.remove(config);
    }
}

// ── owner view ──────────────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PersonUsage {
    pub id: String,
    pub name: String,
    pub own: bool,
    pub items: usize,
    pub bytes: u64,
    pub through: bool,
    /// A friend of the owner (not of this device) that an owner device vouched for.
    #[serde(default)]
    pub via_owner: bool,
    /// Removed on this server (listed so it can be allowed again).
    #[serde(default)]
    pub removed: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub label: String,
    pub items: usize,
    pub bytes: u64,
    pub oldest_ms: u64,
    pub expires_ms: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub supported: bool,
    pub config: ServerConfig,
    pub storage_ok: bool,
    pub storage_error: Option<String>,
    pub used: u64,
    pub free: Option<u64>,
    pub items: usize,
    pub people: Vec<PersonUsage>,
    pub waiting: Vec<Waiting>,
    pub default_root: String,
    /// Who owns this server (None = not set; this device isn't linked either).
    pub owner: Option<OwnerView>,
    /// This device is linked to an account (it owns itself; nothing to pick).
    pub linked: bool,
    /// Accounts this device knows (its friends' verified accounts) that could own it.
    pub owner_choices: Vec<OwnerView>,
}

/// An account as the owner picker shows it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OwnerView {
    pub account: String,
    pub name: String,
    /// How many of that account's devices this device knows.
    pub devices: usize,
    /// Owner devices currently sharing the server with their friends.
    pub sharing: usize,
    /// Owner devices that report each other removed from the account.
    #[serde(default)]
    pub disputed: usize,
}

/// The accounts this device's friends proved, as the owner picker lists them.
pub fn owner_choices(config: &Path) -> Vec<OwnerView> {
    let mine = crate::account::my_pub(config);
    let friends = crate::friends::load(config);
    let mut by: HashMap<String, OwnerView> = HashMap::new();
    for f in &friends {
        let (Some(acct), Some(eid)) = (f.account_pub.as_deref(), f.endpoint_id.as_deref()) else { continue };
        if Some(acct) == mine.as_deref() || crate::block::is_blocked(config, eid) {
            continue;
        }
        let name = crate::friends::thread_owner(config, &f.id).map(|o| o.name).unwrap_or_else(|| f.name.clone());
        let v = by.entry(acct.to_owned()).or_insert_with(|| OwnerView { account: acct.to_owned(), name, devices: 0, sharing: 0, disputed: 0 });
        v.devices += 1;
    }
    let mut out: Vec<OwnerView> = by.into_values().collect();
    out.sort_by(|a, b| b.devices.cmp(&a.devices).then(a.name.cmp(&b.name)));
    out
}

pub fn status(config: &Path) -> ServerStatus {
    let c = load_config(config);
    let friends = crate::friends::load(config);
    let vouched = super::members::everyone(config, &c);
    let name_of = |person: &str| friends.iter().find(|f| f.id == person).map(|f| f.name.clone())
        .or_else(|| vouched.iter().find(|(id, _, _)| id == person).map(|(_, n, _)| n.clone()))
        .or_else(|| super::members::name_of(config, &c, person));
    let label_for = |eid: &str| -> String {
        if super::members::is_owner_device(config, &c, eid) {
            return "Your devices".into();
        }
        crate::friends::chat_sender(config, eid).map(|f| f.name)
            .or_else(|| super::members::vouched(config, &c, eid).map(|v| v.name))
            .unwrap_or_else(|| "Someone a friend knows".into())
    };
    let linked = crate::account::my_pub(config).is_some();
    let choices = if linked { vec![] } else { owner_choices(config) };
    let owner = super::members::owner_account(config, &c).map(|account| {
        let sharing = super::members::sharing_devices(config, &c);
        let disputed = super::members::disputed_devices(config, &c);
        choices.iter().find(|o| o.account == account).cloned()
            .map(|o| OwnerView { sharing, disputed, ..o })
            .unwrap_or(OwnerView { account, name: if linked { "You".into() } else { "Owner".into() }, devices: 0, sharing, disputed })
    });
    let mut st = ServerStatus {
        supported: hosting_supported(), config: c.clone(), storage_ok: false, storage_error: None,
        used: 0, free: None, items: 0, people: vec![], waiting: vec![],
        default_root: default_root(config).to_string_lossy().into_owned(),
        owner, linked, owner_choices: choices,
    };
    if !c.enabled {
        return st;
    }
    match with_store(config, |s| {
        let mut people: HashMap<String, PersonUsage> = HashMap::new();
        let mut waiting: HashMap<String, Waiting> = HashMap::new();
        for it in s.items.values() {
            let p = people.entry(it.person.clone()).or_insert_with(|| PersonUsage {
                id: it.person.clone(),
                name: if it.person == "own" { "You".into() } else { name_of(&it.person).unwrap_or_else(|| "A friend".into()) },
                own: it.person == "own", items: 0, bytes: 0, through: c.through.contains(&it.person),
                via_owner: it.person.starts_with("v:"), removed: c.denied.contains(&it.person),
            });
            p.items += 1;
            p.bytes += it.ct_size;
            if it.state == "held" {
                let label = it.to.first().map(|t| label_for(t)).unwrap_or_default();
                let w = waiting.entry(label.clone()).or_insert(Waiting { label, items: 0, bytes: 0, oldest_ms: u64::MAX, expires_ms: 0 });
                w.items += 1;
                w.bytes += it.ct_size;
                w.oldest_ms = w.oldest_ms.min(it.created_ms);
                w.expires_ms = w.expires_ms.max(it.expires_ms);
            }
        }
        (used_bytes(s), s.items.len(), people.into_values().collect::<Vec<_>>(), waiting.into_values().collect::<Vec<_>>(),
            crate::locations::volume_bytes(&s.root).map(|(f, _)| f))
    }) {
        Ok((used, items, mut people, mut waiting, free)) => {
            // Everyone who may use it, even with nothing here right now.
            if c.access != "me" {
                for (id, name, _) in &vouched {
                    let removed = c.denied.contains(id) || super::members::devices_of(config, &c, id).iter()
                        .any(|e| c.denied.iter().any(|d| d.strip_prefix("e:") == Some(e.as_str())));
                    if !people.iter().any(|p| &p.id == id) && (removed || c.access == "all" || c.allowed.contains(id)) {
                        people.push(PersonUsage { id: id.clone(), name: name.clone(), own: false, items: 0, bytes: 0,
                            through: c.through.contains(id), via_owner: true, removed });
                    }
                }
            }
            people.sort_by(|a, b| b.own.cmp(&a.own).then(b.bytes.cmp(&a.bytes)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            waiting.sort_by_key(|w| w.oldest_ms);
            st.storage_ok = true;
            st.used = used;
            st.items = items;
            st.people = people;
            st.waiting = waiting;
            st.free = free;
        }
        Err(e) => st.storage_error = Some(format!("{e:#}")),
    }
    st
}

// ── delivery loop ───────────────────────────────────────────────────────────

static DELIVERY_WAKE: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
fn delivery_wake() -> &'static tokio::sync::Notify {
    DELIVERY_WAKE.get_or_init(tokio::sync::Notify::new)
}
static SEEN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

pub fn wake_delivery() {
    delivery_wake().notify_one();
}

/// A device connected to us: if we hold something for it, deliver right away.
pub fn device_seen(eid: &str) {
    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let fresh = m.get(eid).is_none_or(|t| t.elapsed() > Duration::from_secs(20));
    m.insert(eid.to_owned(), Instant::now());
    if m.len() > 4096 {
        m.retain(|_, t| t.elapsed() < Duration::from_secs(600));
    }
    if fresh {
        wake_delivery();
    }
}

/// Connected (or heard from) in the last minute — no need to wake its phone.
pub fn recently_seen_device(eid: &str) -> bool {
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        .and_then(|m| m.get(eid)).is_some_and(|t| t.elapsed() < Duration::from_secs(60))
}

fn recently_seen(eid: &str) -> bool {
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        .and_then(|m| m.get(eid)).is_some_and(|t| t.elapsed() < Duration::from_secs(30))
}

/// Poke recipients that have items waiting ("mailbox.notify"); they pull. Backoff
/// per device 15s → 60s → 5min → 15min; any sighting of the device resets it.
pub fn spawn_delivery(config: PathBuf, net: Arc<crate::iroh_net::IrohState>) {
    tauri::async_runtime::spawn(async move {
        let mut backoff: HashMap<String, (u32, Instant)> = HashMap::new();
        let mut last_count: HashMap<String, usize> = HashMap::new();
        let mut last_gc = Instant::now() - Duration::from_secs(3600);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => {},
                _ = delivery_wake().notified() => {},
            }
            if last_gc.elapsed() > Duration::from_secs(3600) {
                let cfg = config.clone();
                let n = tokio::task::spawn_blocking(move || gc(&cfg)).await.unwrap_or(0);
                if n > 0 {
                    log::info!("transfer-server: expired {n} item(s)");
                }
                last_gc = Instant::now();
            }
            let Some(ep) = net.get().cloned() else { continue };
            let cfg = config.clone();
            let pending = tokio::task::spawn_blocking(move || pending_recipients(&cfg)).await.unwrap_or_default();
            backoff.retain(|eid, _| pending.iter().any(|(e, _)| e == eid));
            last_count.retain(|eid, _| pending.iter().any(|(e, _)| e == eid));
            let now = Instant::now();
            let due: Vec<(String, usize)> = pending.into_iter()
                // A sighting skips the wait only when the last poke failed (they
                // were away); a device that answered is already pulling.
                .filter(|(eid, count)| (recently_seen(eid) && !last_count.contains_key(eid))
                    // Something new arrived for a device that's been answering.
                    || last_count.get(eid).is_some_and(|c| c != count)
                    || backoff.get(eid).is_none_or(|(_, until)| now >= *until))
                .collect();
            let mut tasks = tokio::task::JoinSet::new();
            for (eid, count) in due.into_iter().take(32) {
                let ep = ep.clone();
                tasks.spawn(async move {
                    let ok = notify(&ep, &eid, count).await;
                    (eid, count, ok)
                });
            }
            while let Some(Ok((eid, count, ok))) = tasks.join_next().await {
                // Reachable but nothing got taken since the last poke (an item
                // waiting on its owner's OK, an edit waiting for its message):
                // poke less and less often instead of every minute.
                let stuck = ok && last_count.get(&eid) == Some(&count);
                if ok {
                    last_count.insert(eid.clone(), count);
                } else {
                    last_count.remove(&eid);
                }
                let prev = backoff.get(&eid).map(|b| b.0).unwrap_or(0);
                let fails = if ok && !stuck { 0 } else { prev + 1 };
                let wait = match (ok, fails) {
                    (true, 0) => Duration::from_secs(60), // they're pulling; don't nag
                    (true, 1) => Duration::from_secs(300),
                    (true, _) => Duration::from_secs(1800),
                    (false, 1) => Duration::from_secs(15),
                    (false, 2) => Duration::from_secs(60),
                    (false, 3) => Duration::from_secs(300),
                    _ => Duration::from_secs(900),
                };
                backoff.insert(eid.clone(), (fails, Instant::now() + wait));
                if !ok {
                    // Didn't answer: if it's a phone that was online a moment
                    // ago (so the deposit skipped its push), wake it now.
                    super::push::on_unreachable(&config, &eid);
                }
                if let Some(m) = SEEN.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
                    // A sighting only buys ONE immediate attempt.
                    m.remove(&eid);
                }
            }
        }
    });
}

async fn notify(ep: &iroh::Endpoint, eid: &str, count: usize) -> bool {
    let Ok(id) = eid.parse::<iroh::EndpointId>() else { return false };
    let fut = async {
        let conn = ep.connect(crate::iroh_net::dial_addr(id), crate::iroh_net::ALPN).await.ok()?;
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        write_frame(&mut send, &json!({"kind": "mailbox.notify", "v": super::VERSION, "count": count})).await.ok()?;
        send.finish().ok()?;
        let reply = read_frame_cap(&mut recv, 4096).await.ok()?;
        Some(reply["ok"].as_bool() == Some(true))
    };
    matches!(tokio::time::timeout(Duration::from_secs(10), fut).await, Ok(Some(true)))
}
