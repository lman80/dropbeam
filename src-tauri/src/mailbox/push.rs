//! iOS push through the owner's relay worker (docs/TRANSFER-SERVER-PLAN.md §6).
//!
//! Three parties, none of which learns more than it needs:
//! - the PHONE seals its APNs token to the Worker's key, bound to one server,
//!   and hands that opaque blob to its Transfer Servers (`mailbox.push-register`);
//! - the SENDER seals a short preview ("Ashton: running late") to the phone's
//!   own push key (advertised, signed, in hellos) and attaches it to the deposit;
//! - the SERVER, when it stores something for a sleeping phone, signs a request
//!   to the Worker with its endpoint key. The Worker checks that signature and
//!   the token binding, then asks APNs. The phone's Notification Service
//!   Extension opens the preview locally.
//! Without a configured Worker (no APNs key yet) everything still works — the
//! phone just learns about held items the next time it opens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::seal;
use super::server::{Item, ServerConfig};

/// The owner's relay (override per server with `pushUrl` in transfer-server.json).
pub const DEFAULT_URL: &str = "https://dropbeam-push.ashton-mcp-worker.workers.dev/push";
/// The Worker's X25519 sealing key (push-worker/genkey.js). Rotating it = new build.
pub const WORKER_SEAL_PUB: &str = "tADEhMaGV8z2Mj2BGqbJL/6zsDzNWQ7Lyo5NL0Gprjg=";
const BUNDLE: &str = "com.ashtonmiller.dropbeam";
const TOKEN_TTL_MS: u64 = 90 * 24 * 3600 * 1000;
const REREGISTER_MS: u64 = 7 * 24 * 3600 * 1000;

static LOCK: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    crate::chat::now_ms()
}

// ── phone side ──────────────────────────────────────────────────────────────

/// This phone's push registration (iOS only in practice).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// APNs device token, hex.
    pub token: String,
    /// "prod" | "sandbox".
    pub env: String,
    /// X25519 public key (base64) whose private half lives in the shared
    /// keychain, readable by the Notification Service Extension.
    pub push_key: String,
    /// Show message text in notifications (else "New message from Ashton").
    #[serde(default = "yes")]
    pub previews: bool,
    #[serde(default)]
    pub updated_ms: u64,
    /// server eid → when we last registered there.
    #[serde(default)]
    pub registered: HashMap<String, u64>,
}

fn yes() -> bool {
    true
}

fn device_path(config: &Path) -> PathBuf {
    config.join("push-device.json")
}

pub fn device(config: &Path) -> Option<Device> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    match crate::settings::read_json_store::<Device>(&device_path(config)) {
        crate::settings::StoreRead::Loaded(d) if !d.push_key.is_empty() => Some(d),
        _ => None,
    }
}

fn save_device(config: &Path, d: &Device) {
    if let Ok(bytes) = serde_json::to_vec(d) {
        if let Err(e) = crate::settings::write_atomic(&device_path(config), &bytes) {
            log::warn!("push: cannot save the device registration: {e}");
        }
    }
}

/// The phone got (or refreshed) its APNs token / push key. Returns true when
/// anything changed (servers need a fresh registration, friends a fresh hello).
pub fn set_device(config: &Path, token: &str, env: &str, push_key: &str) -> Result<bool> {
    anyhow::ensure!(token.len() >= 32 && token.len() <= 200 && token.chars().all(|c| c.is_ascii_hexdigit()), "bad device token");
    seal::key32(push_key).context("bad push key")?;
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut d = match crate::settings::read_json_store::<Device>(&device_path(config)) {
        crate::settings::StoreRead::Loaded(d) => d,
        _ => Device { previews: true, ..Default::default() },
    };
    let env = if env == "sandbox" { "sandbox" } else { "prod" };
    let changed = d.token != token || d.env != env || d.push_key != push_key;
    if changed {
        d.token = token.to_ascii_lowercase();
        d.env = env.into();
        d.push_key = push_key.into();
        d.registered.clear();
    }
    d.updated_ms = now();
    save_device(config, &d);
    Ok(changed)
}

pub fn set_previews(config: &Path, on: bool) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if let crate::settings::StoreRead::Loaded(mut d) = crate::settings::read_json_store::<Device>(&device_path(config)) {
        d.previews = on;
        save_device(config, &d);
    }
}

/// Hello fields: our push key (signed like the mailbox key) and whether
/// senders may put message text in our notifications.
pub fn push_key_advert(config: &Path, signer: &iroh::SecretKey) -> Option<(String, String)> {
    let d = device(config)?;
    let k = seal::key32(&d.push_key).ok()?;
    Some((d.push_key, seal::sign_mailbox_key(signer, &k)))
}

pub fn previews_allowed(config: &Path) -> bool {
    device(config).is_none_or(|d| d.previews)
}

/// Seal the APNs token for the Worker, usable only by `server`:
/// `eph(32) || nonce(12) || AES-256-GCM`, key = HKDF-SHA256(X25519(eph, W),
/// eph || W, "dropbeam-push-token-v1").
pub fn seal_token(d: &Device, me: &str, server: &str, worker_pub: &str) -> Result<String> {
    let w = seal::key32(worker_pub)?;
    let body = serde_json::to_vec(&json!({
        "token": d.token, "env": d.env, "bundle": BUNDLE, "allowed_server": server,
        "device": me, "exp": now() + TOKEN_TTL_MS,
    }))?;
    let eph: [u8; 32] = rand::random();
    let epk = seal::x25519_public(&eph);
    let ss = seal::x25519(&eph, &w).context("bad worker key")?;
    let mut salt = epk.to_vec();
    salt.extend_from_slice(&w);
    let k = seal::hkdf32_pub(&ss, &salt, b"dropbeam-push-token-v1");
    let nonce: [u8; 12] = rand::random();
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).map_err(|_| anyhow::anyhow!("aes key"))?);
    let mut buf = body;
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| anyhow::anyhow!("seal failed"))?;
    let mut out = epk.to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(seal::b64(&out))
}

/// The iOS shell drops the APNs token + push key in `push-token.json`
/// (PushRegistration.swift). Adopt it; true when it changed.
pub fn import_token_file(config: &Path) -> bool {
    let path = config.join("push-token.json");
    let Ok(bytes) = std::fs::read(&path) else { return false };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return false };
    let (Some(token), Some(env), Some(key)) = (v["token"].as_str(), v["env"].as_str(), v["pushKey"].as_str()) else { return false };
    match set_device(config, token, env, key) {
        Ok(changed) => changed,
        Err(e) => {
            log::warn!("push: ignoring a malformed device registration: {e}");
            false
        }
    }
}

/// Register (or refresh) this phone at every server that holds our messages.
pub async fn register_everywhere(net: &crate::iroh_net::IrohState, config: &Path) {
    // Runs every few minutes: keep the extension's blocked list current.
    share_blocked(config);
    let Some(d) = device(config) else { return };
    let Some(ep) = net.get().cloned() else { return };
    let me = ep.id().to_string();
    let t = now();
    for s in super::client::servers(config).into_iter().filter(|s| s.usable() && (s.own || s.hold_for_me)) {
        if d.registered.get(&s.eid).is_some_and(|at| t.saturating_sub(*at) < REREGISTER_MS) {
            continue;
        }
        let Ok(sealed) = seal_token(&d, &me, &s.eid, WORKER_SEAL_PUB) else { continue };
        let ok = super::client::push_register_with(&ep, &s.eid, &sealed, push_key_advert(config, ep.secret_key())).await;
        if ok {
            let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
            if let crate::settings::StoreRead::Loaded(mut cur) = crate::settings::read_json_store::<Device>(&device_path(config)) {
                cur.registered.insert(s.eid.clone(), t);
                save_device(config, &cur);
            }
            log::info!("push: registered this phone with a Transfer Server");
        }
    }
}

// ── sender side: sealed previews ─────────────────────────────────────────────

fn my_name(config: &Path) -> String {
    let n = crate::settings::load(config, "", "").display_name;
    if n.trim().is_empty() { "A friend".into() } else { n.trim().chars().take(60).collect() }
}

fn seal_for(config: &Path, to: &[seal::Recipient], msg_id: Option<&str>, body: impl Fn(bool) -> String) -> Value {
    let peers = super::keys::peers(config);
    let title = my_name(config);
    let me = identity(config).map(|k| k.public().to_string()).unwrap_or_default();
    let mut out = serde_json::Map::new();
    for r in to {
        let Some(p) = peers.get(&r.eid) else { continue };
        let Some(pk) = p.push_key.as_deref().and_then(|k| seal::key32(k).ok()) else { continue };
        let mut pt = json!({"t": title, "b": body(p.push_text), "f": me});
        // The message id lets the phone keep one banner per message.
        if let Some(id) = msg_id {
            pt["i"] = json!(id);
        }
        let pt = pt.to_string();
        if let Ok(s) = seal::seal_small(&pk, pt.as_bytes()) {
            out.insert(r.eid.clone(), json!(s));
        }
    }
    Value::Object(out)
}

/// Sealed per-device notification previews for a chat frame.
pub fn previews(config: &Path, to: &[seal::Recipient], frame: &Value) -> Value {
    let kind = frame["msgKind"].as_str().unwrap_or("text");
    if matches!(kind, "reaction" | "edit" | "delete") {
        return json!({});
    }
    let text: String = frame["text"].as_str().unwrap_or("").chars().take(180).collect();
    let files = frame["files"].as_array().map(|a| a.len()).unwrap_or(0);
    let id = frame["id"].as_str().filter(|i| i.len() <= 64);
    seal_for(config, to, id, |show_text| match kind {
        "gif" => "Sent a GIF".into(),
        "file" if files > 1 => format!("Sent you {files} files"),
        "file" => "Sent you a file".into(),
        _ if show_text && !text.trim().is_empty() => text.clone(),
        _ => "New message".into(),
    })
}

/// Sealed per-device previews for a file send.
pub fn file_previews(config: &Path, to: &[seal::Recipient], names: &[String]) -> Value {
    let body = match names.len() {
        0 | 1 => "Sent you a file".to_owned(),
        n => format!("Sent you {n} files"),
    };
    let first = names.first().cloned().unwrap_or_default();
    seal_for(config, to, None, |show| if show && names.len() == 1 && !first.is_empty() { format!("Sent you {first}") } else { body.clone() })
}

// ── server side ─────────────────────────────────────────────────────────────

/// Whether this server will try to wake phones (a relay URL is set).
pub fn configured() -> bool {
    !DEFAULT_URL.is_empty()
}

fn reg_dir(root: &Path) -> PathBuf {
    root.join("push")
}

fn root_of(config: &Path) -> Option<PathBuf> {
    let c = super::server::load_config(config);
    super::server::root(config, &c).ok()
}

/// `mailbox.push-register {sealed_token}` (or `{remove:true}`) from a phone.
pub fn register(config: &Path, c: &ServerConfig, who: &str, req: &Value) -> Value {
    if !super::server::rights_for(config, c, who).any() {
        return json!({"ok": false, "reason": "denied"});
    }
    register_unchecked(config, c, who, req)
}

/// `register` after the rights check.
fn register_unchecked(config: &Path, _c: &ServerConfig, who: &str, req: &Value) -> Value {
    let Some(root) = root_of(config) else { return json!({"ok": false, "reason": "storage"}) };
    let path = reg_dir(&root).join(format!("{who}.json"));
    if req["remove"].as_bool() == Some(true) {
        let _ = std::fs::remove_file(path);
        return json!({"ok": true});
    }
    let Some(token) = req["sealed_token"].as_str().filter(|t| t.len() <= 2048 && seal::unb64(t).is_ok()) else {
        return json!({"ok": false, "reason": "invalid"});
    };
    let _ = std::fs::create_dir_all(reg_dir(&root));
    let mut rec = json!({"sealed_token": token, "at": now()});
    // Optional: the phone's notification key, signed by the phone's endpoint key
    // (newer clients). With it this server seals the sender id for the phone's
    // eyes only; older clients fall back to the push key from the phone's hello.
    if let Some(pk) = verified_push_key(who, req) {
        rec["push_key"] = json!(pk);
    }
    match crate::settings::write_atomic(&path, rec.to_string().as_bytes()) {
        Ok(()) => json!({"ok": true}),
        Err(_) => json!({"ok": false, "reason": "storage"}),
    }
}

/// A `push_key`/`push_sig` pair in a request, if it verifies against `who`.
fn verified_push_key(who: &str, req: &Value) -> Option<String> {
    let pk = req["push_key"].as_str()?;
    let k = seal::key32(pk).ok()?;
    seal::verify_mailbox_key(who, &k, req["push_sig"].as_str()?).then(|| pk.to_owned())
}

fn identity(config: &Path) -> Option<iroh::SecretKey> {
    let bytes = std::fs::read(config.join("iroh-identity.key")).ok()?;
    let seed = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
    Some(iroh::SecretKey::from_bytes(&seed))
}

/// Canonical bytes the Worker verifies (see push-worker/worker.js).
pub fn request_message(server: &str, sealed_token: &str, collapse: &str, payload: &str, ts: u64) -> Vec<u8> {
    format!("dropbeam-push-v1\n1\n{server}\n{sealed_token}\n{collapse}\n{payload}\n{ts}").into_bytes()
}

pub fn signed_request(signer: &iroh::SecretKey, sealed_token: &str, collapse: &str, payload: &str) -> Value {
    let server = signer.public().to_string();
    let ts = now();
    let sig = signer.sign(&request_message(&server, sealed_token, collapse, payload, ts));
    json!({"v": 1, "server": server, "sealed_token": sealed_token, "collapse": collapse, "payload": payload, "ts": ts,
        "sig": seal::b64(&sig.to_bytes())})
}

/// At most one wake-up per phone per 30s: a burst of messages is one banner,
/// and the relay's hourly budget lasts through a real conversation.
fn coalesce_ok(to: &str) -> bool {
    static LAST: Mutex<Option<HashMap<String, std::time::Instant>>> = Mutex::new(None);
    let mut g = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.get(to).is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
        return false;
    }
    m.insert(to.to_owned(), std::time::Instant::now());
    true
}

/// (item id, device) pairs a wake-up already covered — so a device that was
/// online at deposit time (no push then) gets exactly one later, when it stops
/// answering, and never a second for the same item.
static PUSHED: Mutex<Option<std::collections::HashSet<(String, String)>>> = Mutex::new(None);

fn mark_pushed(item: &str, to: &str) -> bool {
    let mut g = PUSHED.lock().unwrap_or_else(|p| p.into_inner());
    let set = g.get_or_insert_with(Default::default);
    if set.len() > 20_000 {
        set.clear();
    }
    set.insert((item.to_owned(), to.to_owned()))
}

fn was_pushed(item: &str, to: &str) -> bool {
    PUSHED.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        .is_some_and(|s| s.contains(&(item.to_owned(), to.to_owned())))
}

/// A short, non-identifying tag for logs.
fn short(eid: &str) -> String {
    eid.chars().take(6).collect()
}

/// A device's push registration here: the sealed APNs token (only the relay
/// can open it) and, when known, the phone's notification key.
struct Registration {
    path: PathBuf,
    token: String,
    push_key: Option<[u8; 32]>,
}

/// The sealed APNs registration a device left here, if any.
fn registration(config: &Path, to: &str) -> Option<Registration> {
    let path = reg_dir(&root_of(config)?).join(format!("{to}.json"));
    let rec = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())?;
    let token = rec["sealed_token"].as_str().map(String::from)?;
    // The key the phone registered with, else the one its (signature-checked)
    // hello advertised to this device.
    let push_key = rec["push_key"].as_str().map(String::from)
        .or_else(|| super::keys::peers(config).get(to).and_then(|p| p.push_key.clone()))
        .and_then(|k| seal::key32(&k).ok());
    Some(Registration { path, token, push_key })
}

/// The relay forwards at most this much payload to APNs (push-worker MAX_PAYLOAD).
const MAX_PAYLOAD: usize = 3000;

/// What the relay and Apple see of a wake-up. When this server knows the
/// phone's notification key the sender id (and the sender's sealed preview)
/// travel sealed to that key: `{"s": seal({"f": from, "e": preview})}` — the
/// relay and APNs see only ciphertext. Otherwise (a phone that never told us
/// its key) the legacy `{"f": from, "e": preview}` is sent so its banner still
/// names the sender.
fn wake_payload(from: &str, preview: Option<&String>, push_key: Option<&[u8; 32]>) -> String {
    let inner = |with_preview: bool| match preview.filter(|_| with_preview) {
        Some(sealed) => json!({"f": from, "e": sealed}).to_string(),
        None => json!({"f": from}).to_string(),
    };
    let Some(pk) = push_key else { return inner(true) };
    for with_preview in [true, false] {
        if let Ok(s) = seal::seal_small(pk, inner(with_preview).as_bytes()) {
            let out = json!({"s": s}).to_string();
            if out.len() <= MAX_PAYLOAD {
                return out;
            }
        }
    }
    // Can't seal (bad key): say nothing identifying — the phone shows the generic banner.
    String::new()
}

/// Notification thread for (sender → this phone). Salted with the recipient so
/// the relay/Apple can't link one sender across different phones.
fn thread_tag(from: &str, to: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"dropbeam-push-thread-v2\n");
    h.update(to.as_bytes());
    h.update(b"\n");
    h.update(from.as_bytes());
    hex::encode(&h.finalize()[..8])
}

/// The signed relay request that wakes `to` about `item` (None = not a phone
/// that registered here, or no server identity).
pub(crate) fn wake_request(config: &Path, item: &Item, to: &str) -> Option<(PathBuf, Value)> {
    let reg = registration(config, to)?;
    let signer = identity(config)?;
    // Same sender → one notification thread on the phone.
    let collapse = thread_tag(&item.from, to);
    // The sender id is the one the server verified on the item; the phone
    // names the banner from its own contacts and checks the preview matches.
    let payload = wake_payload(&item.from, item.push.get(to), reg.push_key.as_ref());
    Some((reg.path, signed_request(&signer, &reg.token, &collapse, &payload)))
}

#[cfg_attr(test, allow(dead_code))]
fn relay_url(config: &Path) -> String {
    let c = super::server::load_config(config);
    if c.push_url.trim().is_empty() { DEFAULT_URL.to_owned() } else { c.push_url.trim().to_owned() }
}

/// Test hook: requests that would have gone to the relay.
#[cfg(test)]
pub(crate) static SENT_FOR_TESTS: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new());

fn send_wake(config: &Path, item: &Item, to: &str, why: &'static str) {
    let Some((path, body)) = wake_request(config, item, to) else { return };
    if !coalesce_ok(to) {
        log::info!("push: skipped waking {} ({why}): one went out in the last 30s", short(to));
        return;
    }
    #[cfg(test)]
    {
        SENT_FOR_TESTS.lock().unwrap().push((to.to_owned(), body));
        let _ = path;
        return;
    }
    #[cfg(not(test))]
    {
        let url = relay_url(config);
        let tag = short(to);
        tauri::async_runtime::spawn(async move {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build();
            let Ok(client) = client else { return };
            match client.post(&url).json(&body).send().await {
                Ok(res) => {
                    let http = res.status().as_u16();
                    let v: Value = res.json().await.unwrap_or(Value::Null);
                    let reason: String = v["reason"].as_str().unwrap_or("?").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(24).collect();
                    if v["gone"].as_bool() == Some(true) {
                        let _ = std::fs::remove_file(&path);
                        log::info!("push: {tag}'s registration expired; removed it");
                    } else if v["ok"].as_bool() == Some(true) {
                        log::info!("push: woke {tag} ({why}) — relay {http}, APNs {}", v["status"].as_u64().unwrap_or(0));
                    } else {
                        log::info!("push: relay didn't wake {tag} ({why}): {http} {reason}");
                    }
                }
                Err(e) => log::info!("push: relay unreachable waking {tag}: {}", e.without_url()),
            }
        });
    }
}

/// Something new is held: wake each addressed phone that registered here and
/// isn't connected right now. A phone that was online a moment ago is poked
/// instead (it pulls at once); if that poke fails, `on_unreachable` wakes it.
pub fn on_stored(config: &Path, item: &Item) {
    if item.kind == "op" {
        return;
    }
    // Never ring a phone for someone this account blocked (the block list
    // rides account sync, so an own Transfer Server has the phone's list).
    if crate::block::is_blocked(config, &item.from) {
        log::info!("push: no wake-up for an item from a blocked sender");
        return;
    }
    for to in &item.to {
        if registration(config, to).is_none() {
            continue;
        }
        if super::server::recently_seen_device(to) {
            log::info!("push: {} was online moments ago; poking it first", short(to));
            continue;
        }
        mark_pushed(&item.id, to);
        send_wake(config, item, to, "new item");
    }
}

/// A poke to `eid` just failed: wake it (once) for recent items the deposit
/// didn't push because it looked online then.
pub fn on_unreachable(config: &Path, eid: &str) {
    if registration(config, eid).is_none() {
        return;
    }
    let t = now();
    let mut due: Vec<Item> = super::server::waiting_items(config, eid).into_iter()
        .filter(|i| i.kind != "op" && t.saturating_sub(i.created_ms) < 15 * 60 * 1000 && !was_pushed(&i.id, eid))
        .filter(|i| !crate::block::is_blocked(config, &i.from))
        .collect();
    if due.is_empty() {
        return;
    }
    due.sort_by_key(|i| i.created_ms);
    for i in &due {
        mark_pushed(&i.id, eid);
    }
    send_wake(config, due.last().unwrap(), eid, "didn't answer");
}

// ── notification de-duplication on the phone ────────────────────────────────
//
// The Notification Service Extension shows a banner for a push; the app may
// later get the same message (from the server, or synced from the Mac). Both
// share the App Group container (its path is left in `app-group-path` by the
// iOS shell): the extension lists the message ids it announced in
// `nse-notified.json`, the app lists the ids it already has in
// `app-have.json`, and each checks the other's before notifying.

fn group_dir(config: &Path) -> Option<PathBuf> {
    let p = std::fs::read_to_string(config.join("app-group-path")).ok()?;
    let p = PathBuf::from(p.trim());
    p.is_dir().then_some(p)
}

fn read_ids(path: &Path) -> HashMap<String, u64> {
    let map: HashMap<String, Value> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    map.into_iter().map(|(k, v)| (k, v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)).unwrap_or(0))).collect()
}

/// Hand the Notification Service Extension this account's blocked endpoints
/// (`push-blocked.json`, a JSON array) so a push about a blocked sender — e.g.
/// from a friend's Transfer Server that doesn't know our block list — never
/// shows their name or text. Rewritten only when it changed. No-op off iOS.
pub fn share_blocked(config: &Path) {
    let Some(dir) = group_dir(config) else { return };
    let mut ids: Vec<String> = crate::block::snapshot(config).into_iter().filter(|(_, r)| r.blocked).map(|(e, _)| e).collect();
    ids.sort();
    let path = dir.join("push-blocked.json");
    let cur: Option<Vec<String>> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if cur.as_ref() == Some(&ids) {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(&ids) {
        let _ = crate::settings::write_atomic(&path, &bytes);
    }
}

/// The extension already announced this message.
pub fn already_announced(config: &Path, msg_id: &str) -> bool {
    group_dir(config).is_some_and(|d| read_ids(&d.join("nse-notified.json")).contains_key(msg_id))
}

/// The app has these messages (so a later push for them stays quiet).
pub fn note_have(config: &Path, ids: &[&str]) {
    let Some(dir) = group_dir(config) else { return };
    if ids.is_empty() {
        return;
    }
    share_blocked(config);
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = dir.join("app-have.json");
    let mut map = read_ids(&path);
    let t = now();
    for id in ids {
        map.insert((*id).to_owned(), t);
    }
    map.retain(|_, at| t.saturating_sub(*at) < 7 * 24 * 3600 * 1000);
    if map.len() > 3000 {
        let mut v: Vec<(u64, String)> = map.iter().map(|(k, a)| (*a, k.clone())).collect();
        v.sort();
        for (_, k) in v.into_iter().take(map.len() - 3000) {
            map.remove(&k);
        }
    }
    if let Ok(bytes) = serde_json::to_vec(&map) {
        let _ = crate::settings::write_atomic(&path, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_seal_matches_the_worker_layout() {
        let d = Device { token: "ab".repeat(32), env: "prod".into(), push_key: seal::b64(&[1u8; 32]), previews: true, ..Default::default() };
        let wsk: [u8; 32] = rand::random();
        let wpk = seal::b64(&seal::x25519_public(&wsk));
        let sealed = seal_token(&d, "me", "server-eid", &wpk).unwrap();
        // Open it the way the Worker does.
        let raw = seal::unb64(&sealed).unwrap();
        let (eph, nonce, ct) = (&raw[..32], &raw[32..44], &raw[44..]);
        let ss = seal::x25519(&wsk, eph.try_into().unwrap()).unwrap();
        let mut salt = eph.to_vec();
        salt.extend_from_slice(&seal::x25519_public(&wsk));
        let k = seal::hkdf32_pub(&ss, &salt, b"dropbeam-push-token-v1");
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).unwrap());
        let mut buf = ct.to_vec();
        let pt = key.open_in_place(Nonce::try_assume_unique_for_key(nonce).unwrap(), Aad::empty(), &mut buf).unwrap();
        let v: Value = serde_json::from_slice(pt).unwrap();
        assert_eq!(v["allowed_server"], "server-eid");
        assert_eq!(v["token"], "ab".repeat(32));
    }

    #[test]
    fn phone_announces_each_message_once() {
        let base = std::env::temp_dir().join(format!("dropbeam-nse-{}", uuid::Uuid::new_v4()));
        let (config, group) = (base.join("config"), base.join("group"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&group).unwrap();
        // No shared container (desktop): nothing is ever suppressed.
        assert!(!already_announced(&config, "m1"));
        note_have(&config, &["m1"]);
        assert!(!group.join("app-have.json").exists());
        std::fs::write(config.join("app-group-path"), group.to_string_lossy().as_bytes()).unwrap();
        // The extension wrote (as Swift does) an integer-ms map.
        std::fs::write(group.join("nse-notified.json"), br#"{"m1": 1790000000000}"#).unwrap();
        assert!(already_announced(&config, "m1"));
        assert!(!already_announced(&config, "m2"));
        note_have(&config, &["m2"]);
        let have: HashMap<String, u64> = serde_json::from_slice(&std::fs::read(group.join("app-have.json")).unwrap()).unwrap();
        assert!(have.contains_key("m2"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn previews_carry_the_message_id() {
        let base = std::env::temp_dir().join(format!("dropbeam-prev-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let phone_sk: [u8; 32] = rand::random();
        let eid = iroh::SecretKey::generate().public().to_string();
        let pk = seal::x25519_public(&phone_sk);
        let info: super::super::keys::PeerInfo = serde_json::from_value(json!({"push_key": seal::b64(&pk), "push_text": true})).unwrap();
        super::super::keys::set_peer_for_tests(&base, &eid, info);
        let to = vec![seal::Recipient { eid: eid.clone(), key: pk }];
        let v = previews(&base, &to, &json!({"msgKind": "text", "text": "hi", "id": "msg-1"}));
        let sealed = v[&eid].as_str().expect("a preview for the phone");
        let plain: Value = serde_json::from_slice(&seal::open_small(&phone_sk, sealed).unwrap()).unwrap();
        assert_eq!((plain["i"].as_str(), plain["b"].as_str()), (Some("msg-1"), Some("hi")));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A server set up in a temp dir, with its identity key on disk.
    fn test_server() -> PathBuf {
        let config = std::env::temp_dir().join(format!("dropbeam-pushsrv-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config).unwrap();
        let mut c = super::super::server::load_config(&config);
        super::super::server::init_root(&config, &mut c).unwrap();
        super::super::server::save_config(&config, &c).unwrap();
        std::fs::write(config.join("iroh-identity.key"), iroh::SecretKey::generate().to_bytes()).unwrap();
        config
    }

    fn item(from: &str, to: &str) -> Item {
        serde_json::from_value(json!({"id": uuid::Uuid::new_v4().to_string(), "from": from, "person": "p", "to": [to],
            "kind": "chat", "ctSize": 1, "headerSha": "", "createdMs": now(), "expiresMs": now() + 60_000, "state": "held",
            "push": {to: "c2VhbGVk"}})).unwrap()
    }

    fn sent_to(to: &str) -> Vec<Value> {
        SENT_FOR_TESTS.lock().unwrap().iter().filter(|(t, _)| t == to).map(|(_, b)| b.clone()).collect()
    }

    #[test]
    fn the_sender_id_travels_sealed_to_the_phone() {
        let phone_sk: [u8; 32] = rand::random();
        let pk = seal::x25519_public(&phone_sk);
        let preview = "c2VhbGVkLXByZXZpZXc=".to_owned();
        let p = wake_payload("SENDER-EID", Some(&preview), Some(&pk));
        assert!(!p.contains("SENDER-EID"), "relay/APNs never see the sender id");
        let v: Value = serde_json::from_str(&p).unwrap();
        let inner: Value = serde_json::from_slice(&seal::open_small(&phone_sk, v["s"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!((inner["f"].as_str(), inner["e"].as_str()), (Some("SENDER-EID"), Some(preview.as_str())));
        // An oversized preview is dropped (still sealed, still names the sender).
        let big = "A".repeat(4000);
        let p = wake_payload("SENDER-EID", Some(&big), Some(&pk));
        assert!(p.len() <= MAX_PAYLOAD && !p.contains("SENDER-EID"));
        let v: Value = serde_json::from_str(&p).unwrap();
        let inner: Value = serde_json::from_slice(&seal::open_small(&phone_sk, v["s"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(inner["f"], "SENDER-EID");
        assert!(inner.get("e").is_none());
        // No key known: the legacy shape (older phones still get named banners).
        let legacy: Value = serde_json::from_str(&wake_payload("SENDER-EID", Some(&preview), None)).unwrap();
        assert_eq!(legacy["f"], "SENDER-EID");
    }

    #[test]
    fn thread_tags_are_per_recipient() {
        assert_eq!(thread_tag("a", "phone1"), thread_tag("a", "phone1"));
        assert_ne!(thread_tag("a", "phone1"), thread_tag("a", "phone2"));
        assert_ne!(thread_tag("a", "phone1"), thread_tag("b", "phone1"));
        assert_eq!(thread_tag("a", "phone1").len(), 16);
    }

    #[test]
    fn registration_with_a_signed_push_key_seals_wakeups() {
        let config = test_server();
        let c = super::super::server::load_config(&config);
        let phone = iroh::SecretKey::generate();
        let who = phone.public().to_string();
        let phone_sk: [u8; 32] = rand::random();
        let pk = seal::x25519_public(&phone_sk);
        // A forged key (signed by someone else) is ignored.
        let other = iroh::SecretKey::generate();
        let bad = json!({"sealed_token": seal::b64(b"tok"), "push_key": seal::b64(&pk), "push_sig": seal::sign_mailbox_key(&other, &pk)});
        assert_eq!(register_unchecked(&config, &c, &who, &bad)["ok"], true);
        assert!(registration(&config, &who).unwrap().push_key.is_none());
        let good = json!({"sealed_token": seal::b64(b"tok"), "push_key": seal::b64(&pk), "push_sig": seal::sign_mailbox_key(&phone, &pk)});
        assert_eq!(register_unchecked(&config, &c, &who, &good)["ok"], true);
        assert_eq!(registration(&config, &who).unwrap().push_key, Some(pk));
        let from = iroh::SecretKey::generate().public().to_string();
        let (_, body) = wake_request(&config, &item(&from, &who), &who).unwrap();
        let payload = body["payload"].as_str().unwrap();
        assert!(!payload.contains(&from) && !body["collapse"].as_str().unwrap().contains(&from));
        let v: Value = serde_json::from_str(payload).unwrap();
        let inner: Value = serde_json::from_slice(&seal::open_small(&phone_sk, v["s"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(inner["f"].as_str(), Some(from.as_str()));
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn no_banner_for_a_blocked_sender() {
        let config = test_server();
        let c = super::super::server::load_config(&config);
        let phone = iroh::SecretKey::generate().public().to_string();
        assert_eq!(register_unchecked(&config, &c, &phone, &json!({"sealed_token": seal::b64(b"tok")}))["ok"], true);
        let (friend, spammer) = (iroh::SecretKey::generate().public().to_string(), iroh::SecretKey::generate().public().to_string());
        let f = crate::friends::upsert_by_endpoint(&config, &spammer, "Spam");
        crate::block::block_friend(&config, &f.id).unwrap();
        on_stored(&config, &item(&spammer, &phone));
        assert!(sent_to(&phone).is_empty(), "a blocked sender never wakes the phone");
        on_unreachable(&config, &phone);
        assert!(sent_to(&phone).is_empty(), "not even later");
        on_stored(&config, &item(&friend, &phone));
        assert_eq!(sent_to(&phone).len(), 1, "anyone else still does");
        let _ = std::fs::remove_dir_all(config);
    }

    #[test]
    fn the_extension_gets_the_blocked_list() {
        let base = std::env::temp_dir().join(format!("dropbeam-nseblk-{}", uuid::Uuid::new_v4()));
        let (config, group) = (base.join("config"), base.join("group"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(config.join("app-group-path"), group.to_string_lossy().as_bytes()).unwrap();
        let spam = iroh::SecretKey::generate().public().to_string();
        let f = crate::friends::upsert_by_endpoint(&config, &spam, "Spam");
        crate::block::block_friend(&config, &f.id).unwrap();
        share_blocked(&config);
        let list: Vec<String> = serde_json::from_slice(&std::fs::read(group.join("push-blocked.json")).unwrap()).unwrap();
        assert_eq!(list, vec![spam.clone()]);
        crate::block::unblock(&config, &spam).unwrap();
        share_blocked(&config);
        let list: Vec<String> = serde_json::from_slice(&std::fs::read(group.join("push-blocked.json")).unwrap()).unwrap();
        assert!(list.is_empty(), "an unblock clears it");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn signed_request_verifies() {
        let k = iroh::SecretKey::generate();
        let r = signed_request(&k, "tok", "c", "p");
        let sig = seal::unb64(r["sig"].as_str().unwrap()).unwrap();
        let msg = request_message(r["server"].as_str().unwrap(), "tok", "c", "p", r["ts"].as_u64().unwrap());
        assert!(k.public().verify(&msg, &iroh::Signature::from_bytes(&sig.try_into().unwrap())).is_ok());
        assert_eq!(r["server"].as_str().unwrap().len(), 64, "the worker reads the server id as 64 hex chars");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    /// `cargo test --lib push_worker_live -- --ignored --nocapture`: talks to the
    /// deployed relay. A valid signature gets past auth (then "not_configured"
    /// until the APNs key is in); a bad one is refused.
    #[tokio::test]
    #[ignore]
    async fn push_worker_live() {
        let k = iroh::SecretKey::generate();
        let d = Device { token: "ab".repeat(32), env: "sandbox".into(), push_key: seal::b64(&[1u8; 32]), previews: true, ..Default::default() };
        let tok = seal_token(&d, "me", &k.public().to_string(), WORKER_SEAL_PUB).unwrap();
        println!("SEALED_TOKEN {tok}");
        let body = signed_request(&k, &tok, "c", "");
        let r = reqwest::Client::new().post(DEFAULT_URL).json(&body).send().await.unwrap();
        let (s, v): (u16, Value) = (r.status().as_u16(), r.json().await.unwrap());
        println!("good sig → {s} {v}");
        assert_ne!(v["reason"], "signature");
        let mut bad = body.clone();
        bad["payload"] = json!("tampered");
        let r = reqwest::Client::new().post(DEFAULT_URL).json(&bad).send().await.unwrap();
        let v: Value = r.json().await.unwrap();
        println!("bad sig → {v}");
        assert_eq!(v["reason"], "signature");
    }
}
