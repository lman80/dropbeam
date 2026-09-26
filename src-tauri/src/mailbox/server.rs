//! The Transfer Server itself: a desktop DropBeam that holds sealed items for
//! people who are offline and hands them over when they come back.
//!
//! Storage (under the chosen root, default `<config>/transfer-server/`):
//! ```text
//! .dropbeam-server-marker            mount identity; missing/wrong = fail closed
//! receipts.json                      item id → final state (30-day ledger, ids only)
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
const MAX_ITEMS_PER_RECIPIENT: usize = 2000;
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
    /// Refuse new items when the disk has less than this free (0 = 5 GB / 5%).
    #[serde(default)]
    pub min_free: u64,
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Receipt {
    state: String,
    at: u64,
    from: String,
}

struct Store {
    root: PathBuf,
    items: HashMap<String, Item>,
    receipts: HashMap<String, Receipt>,
    uploading: HashSet<String>,
}

static STORES: Mutex<Option<HashMap<PathBuf, Store>>> = Mutex::new(None);

fn item_dir(root: &Path, id: &str) -> PathBuf {
    root.join("items").join(&id[..2]).join(id)
}

fn now() -> u64 {
    crate::chat::now_ms()
}

fn load_store(root: &Path) -> Store {
    let mut items = HashMap::new();
    let base = root.join("items");
    for shard in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        for dir in std::fs::read_dir(shard.path()).into_iter().flatten().flatten() {
            let path = dir.path();
            let record: Option<Item> = std::fs::read(path.join("item.json")).ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let keep = record.filter(|it| {
                let name_ok = path.file_name().is_some_and(|n| n.to_string_lossy() == it.id);
                let bytes_ok = match it.state.as_str() {
                    "held" => std::fs::metadata(path.join("payload")).map(|m| m.len() == it.ct_size).unwrap_or(false),
                    "uploading" => true,
                    _ => false,
                };
                name_ok && bytes_ok && path.join("header.json").is_file()
            });
            match keep {
                Some(it) => {
                    items.insert(it.id.clone(), it);
                }
                None => {
                    log::warn!("transfer-server: removing an incomplete item folder");
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
        }
    }
    let receipts = match crate::settings::read_json_store(&root.join("receipts.json")) {
        crate::settings::StoreRead::Loaded(r) => r,
        _ => HashMap::new(),
    };
    Store { root: root.to_path_buf(), items, receipts, uploading: HashSet::new() }
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

fn save_receipts(s: &Store) {
    if let Ok(bytes) = serde_json::to_vec(&s.receipts) {
        if let Err(e) = crate::settings::write_atomic_with_backup(&s.root.join("receipts.json"), &bytes, false) {
            log::warn!("transfer-server: cannot save receipts: {e}");
        }
    }
}

/// Remove an item's bytes + record and remember how it ended.
fn finish_item(s: &mut Store, id: &str, state: &str) {
    if let Some(it) = s.items.remove(id) {
        let _ = std::fs::remove_dir_all(item_dir(&s.root, id));
        s.receipts.insert(id.to_owned(), Receipt { state: state.into(), at: now(), from: it.from });
        if s.receipts.len() > RECEIPT_MAX {
            let mut by_age: Vec<_> = s.receipts.iter().map(|(k, r)| (r.at, k.clone())).collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(s.receipts.len() - RECEIPT_MAX) {
                s.receipts.remove(&k);
            }
        }
        save_receipts(s);
    }
}

fn used_bytes(s: &Store) -> u64 {
    s.items.values().map(|i| i.ct_size).sum()
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
}

impl Rights {
    pub fn any(&self) -> bool {
        self.own || self.member
    }
}

/// The person id a friend endpoint belongs to (None = not a friend).
fn person_of(config: &Path, eid: &str) -> Option<String> {
    crate::friends::chat_sender(config, eid).map(|f| f.id)
}

pub fn rights_for(config: &Path, c: &ServerConfig, eid: &str) -> Rights {
    if crate::account::is_own_device(config, eid) {
        return Rights { own: true, member: true, through: true };
    }
    let Some(person) = person_of(config, eid) else { return Rights::default() };
    if c.denied.contains(&person) {
        return Rights::default();
    }
    let member = match c.access.as_str() {
        "all" => true,
        "chosen" => c.allowed.contains(&person),
        _ => false,
    };
    Rights { own: false, member, through: member && c.through.contains(&person) }
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
    Some(json!({"name": server_name(&c), "own": r.own, "member": r.member, "through": r.through, "paused": c.paused}))
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
        "mailbox.get" => serve_get(config, &who, send, req).await?,
        other => {
            let reply = match other {
                "mailbox.hello" => hello_reply(config, &c, &who),
                "mailbox.cancel" => cancel(config, &who, req),
                "mailbox.status" => status_reply(config, &who, req),
                "mailbox.fetch" => fetch_reply(config, &who),
                "mailbox.ack" => ack(config, &who, req),
                "mailbox.push-register" => super::push::register(config, &c, &who, req),
                _ => refuse("unknown"),
            };
            write_frame(send, &reply).await?;
        }
    }
    let _ = send.finish();
    Ok(())
}

fn quota_numbers(config: &Path, c: &ServerConfig, s: &Store, person: &str, own: bool) -> Value {
    let used = used_bytes(s);
    let mine: u64 = s.items.values().filter(|i| i.person == person).map(|i| i.ct_size).sum();
    let per_user = if own { c.cap_bytes } else { c.cap_bytes / 4 };
    let _ = config;
    json!({"used": used, "cap": c.cap_bytes, "user_used": mine, "user_cap": per_user, "item_max": c.item_max})
}

fn hello_reply(config: &Path, c: &ServerConfig, who: &str) -> Value {
    let r = rights_for(config, c, who);
    let person = if r.own { "own".to_owned() } else { person_of(config, who).unwrap_or_default() };
    let quota = with_store(config, |s| quota_numbers(config, c, s, &person, r.own)).unwrap_or(json!(null));
    json!({"ok": true, "v": super::VERSION, "name": server_name(c), "paused": c.paused,
        "rights": r, "quota": quota, "expiry": {"file_days": c.file_days, "chat_days": c.chat_days},
        "push": super::push::configured()})
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
    let rights = rights_for(config, c, who);
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
    let person = if rights.own { "own".to_owned() } else { person_of(config, who).unwrap_or_else(|| who.to_owned()) };
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
        if s.receipts.contains_key(&env.item_id) {
            // Already delivered/expired/cancelled once: never resurrect it.
            return Err(refuse("conflict"));
        }
        // Quotas: total, per person, free-space floor, per recipient, concurrency.
        let used = used_bytes(s);
        if c.cap_bytes == 0 || used.saturating_add(ct_size) > c.cap_bytes {
            return Err(refuse("full"));
        }
        let mine: u64 = s.items.values().filter(|i| i.person == person).map(|i| i.ct_size).sum();
        if !rights.own && mine.saturating_add(ct_size) > c.cap_bytes / 4 {
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

struct UploadGuard<'a> {
    config: &'a Path,
    id: String,
}
impl Drop for UploadGuard<'_> {
    fn drop(&mut self) {
        let _ = with_store(self.config, |s| s.uploading.remove(&self.id));
    }
}

async fn serve_deposit(config: &Path, c: &ServerConfig, who: &str, me: Option<&str>, send: &mut SendStream, recv: &mut RecvStream, req: &Value) -> Result<()> {
    let (item, have, root) = match admit(config, c, who, me, req) {
        Ok(v) => v,
        Err(reply) => {
            write_frame(send, &reply).await?;
            return Ok(());
        }
    };
    let _guard = UploadGuard { config, id: item.id.clone() };
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
            let n = tokio::time::timeout(Duration::from_secs(90), recv.read(&mut buf[..want])).await
                .context("upload stalled")??
                .context("upload ended early")?;
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
        log::info!("transfer-server: upload interrupted (resumable): {e:#}");
        let _ = write_frame(send, &refuse("interrupted")).await;
        return Ok(());
    }
    let held = with_store(config, |s| {
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

fn cancel(config: &Path, who: &str, req: &Value) -> Value {
    let id = req["item_id"].as_str().unwrap_or("").to_owned();
    with_store(config, |s| {
        match s.items.get(&id) {
            Some(it) if it.from == who => {
                if s.uploading.contains(&id) {
                    return refuse("busy");
                }
                finish_item(s, &id, "canceled");
                json!({"ok": true})
            }
            Some(_) => refuse("denied"),
            None => json!({"ok": true, "state": s.receipts.get(&id).map(|r| r.state.clone())}),
        }
    })
    .unwrap_or_else(|_| refuse("storage"))
}

fn status_reply(config: &Path, who: &str, req: &Value) -> Value {
    let ids: Vec<String> = req["item_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).take(2000).collect()).unwrap_or_default();
    with_store(config, |s| {
        let items: Vec<Value> = ids.iter().map(|id| {
            if let Some(it) = s.items.get(id).filter(|it| it.from == who) {
                json!({"id": id, "state": it.state, "at": it.created_ms, "expires": it.expires_ms})
            } else if let Some(r) = s.receipts.get(id).filter(|r| r.from == who) {
                json!({"id": id, "state": r.state, "at": r.at})
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
        let mut mine: Vec<&Item> = s.items.values().filter(|i| i.state == "held" && i.to.iter().any(|t| t == who)).collect();
        mine.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
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
        s.items.get(&id).filter(|it| it.state == "held" && it.to.iter().any(|t| t == who)).map(|it| (it.clone(), item_dir(&s.root, &id)))
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
        let state = if ok { "delivered" } else { "rejected" };
        if !ok {
            log::info!("transfer-server: a recipient refused an item ({})", req["reason"].as_str().unwrap_or("no reason").chars().take(40).collect::<String>());
        }
        finish_item(s, &id, state);
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
    with_store(config, |s| {
        let t = now();
        let expired: Vec<String> = s.items.values()
            .filter(|i| !s.uploading.contains(&i.id))
            .filter(|i| (i.state == "held" && i.expires_ms <= t) || (i.state == "uploading" && i.created_ms + STALE_UPLOAD_MS <= t))
            .map(|i| i.id.clone())
            .collect();
        for id in &expired {
            finish_item(s, id, "expired");
        }
        let before = s.receipts.len();
        s.receipts.retain(|_, r| r.at + RECEIPT_MS > t);
        if s.receipts.len() != before {
            save_receipts(s);
        }
        expired.len()
    })
    .unwrap_or(0)
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
            for t in &it.to {
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
    let own: HashSet<String> = crate::account::own_devices(config).into_iter().filter_map(|f| f.endpoint_id).collect();
    with_store(config, |s| {
        let doomed: Vec<String> = s.items.values()
            .filter(|i| i.person == person && !i.to.iter().all(|t| own.contains(t)))
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
}

pub fn status(config: &Path) -> ServerStatus {
    let c = load_config(config);
    let friends = crate::friends::load(config);
    let name_of = |person: &str| friends.iter().find(|f| f.id == person).map(|f| f.name.clone());
    let label_for = |eid: &str| -> String {
        if crate::account::is_own_device(config, eid) {
            return "Your devices".into();
        }
        crate::friends::chat_sender(config, eid).map(|f| f.name).unwrap_or_else(|| "Someone a friend knows".into())
    };
    let mut st = ServerStatus {
        supported: hosting_supported(), config: c.clone(), storage_ok: false, storage_error: None,
        used: 0, free: None, items: 0, people: vec![], waiting: vec![],
        default_root: default_root(config).to_string_lossy().into_owned(),
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
            people.sort_by(|a, b| b.own.cmp(&a.own).then(b.bytes.cmp(&a.bytes)));
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

fn recently_seen(eid: &str) -> bool {
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        .and_then(|m| m.get(eid)).is_some_and(|t| t.elapsed() < Duration::from_secs(30))
}

/// Poke recipients that have items waiting ("mailbox.notify"); they pull. Backoff
/// per device 15s → 60s → 5min → 15min; any sighting of the device resets it.
pub fn spawn_delivery(config: PathBuf, net: Arc<crate::iroh_net::IrohState>) {
    tauri::async_runtime::spawn(async move {
        let mut backoff: HashMap<String, (u32, Instant)> = HashMap::new();
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
            let now = Instant::now();
            let due: Vec<(String, usize)> = pending.into_iter()
                .filter(|(eid, _)| recently_seen(eid) || backoff.get(eid).is_none_or(|(_, until)| now >= *until))
                .collect();
            let mut tasks = tokio::task::JoinSet::new();
            for (eid, count) in due.into_iter().take(32) {
                let ep = ep.clone();
                tasks.spawn(async move {
                    let ok = notify(&ep, &eid, count).await;
                    (eid, ok)
                });
            }
            while let Some(Ok((eid, ok))) = tasks.join_next().await {
                let fails = if ok { 0 } else { backoff.get(&eid).map(|b| b.0).unwrap_or(0) + 1 };
                let wait = match fails {
                    0 => Duration::from_secs(60), // they're pulling; don't nag
                    1 => Duration::from_secs(15),
                    2 => Duration::from_secs(60),
                    3 => Duration::from_secs(300),
                    _ => Duration::from_secs(900),
                };
                backoff.insert(eid.clone(), (fails, Instant::now() + wait));
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
