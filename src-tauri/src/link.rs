//! Serverless account bootstrap over the authenticated iroh connection.
//!
//! Two one-time codes, both shown on a device's screen and scanned by the other:
//!   dropbeamlink1: "link me"   — shown by a device that wants to JOIN an account;
//!   dropbeamjoin1: "join me"   — shown by a device that HAS the account.
//! Either code works whichever way round the user scans: the scanning device
//! decides the direction from the code (which says whether the shower already
//! shares an account with other devices), so the account that has devices is
//! the one both end up in, and two different multi-device accounts are refused
//! before anything is touched.
use std::{collections::HashMap, path::Path, sync::{Arc, Mutex}, time::{Duration, Instant}};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use crate::{chat::{self, ChatMessage}, friends, iroh_net::{self, IrohState}, AppState};

const PREFIX: &str = "dropbeamlink1:";
/// Shown by a device that already has the account: a new device scans it and
/// asks to join (the reverse of `PREFIX`, where the new device shows the code).
const JOIN_PREFIX: &str = "dropbeamjoin1:";
const TTL: Duration = Duration::from_secs(600);
pub(crate) const MAX_OFFER: usize = 64 << 20;
const MAX_AVATAR: usize = 2 << 20;
static ACCOUNT_LOCK: Mutex<()> = Mutex::new(());

// ── what people read when something goes wrong ─────────────────────────────

pub(crate) const DEVICE_CODE_AS_FRIEND: &str = "That code links your own devices — it isn't a friend code. To add this device to your account, open Settings → Devices.";
const FRIEND_CODE: &str = "That's a friend code, not a device code. To add a friend, use Add Friend. To link your own devices, open Settings → Devices on the other device and scan the code it shows.";
const NOT_A_CODE: &str = "That isn't a DropBeam device code. On your other device open Settings → Devices and scan the code it shows.";
const SELF_CODE: &str = "That's this device's own code. Scan it with your other device instead.";
const EXPIRED: &str = "That code has expired or was already used. Show a new code on the other device and scan it again.";
const UNREACHABLE: &str = "Couldn't reach your other device. Make sure DropBeam is open on it and both devices are online, then try again.";
const DROPPED: &str = "The connection to your other device dropped while linking. Keep DropBeam open on both devices and try again.";
const BOTH_ACCOUNTS: &str = "Both devices already belong to different accounts, so they can't be linked. On the device you want to move, open Settings → Devices → Remove This Device from Account, then try again.";
const ALREADY: &str = "These devices are already linked.";
const NOT_READY: &str = "DropBeam is still connecting — try again in a moment.";

/// A refusal the other device sent (older builds send terse reasons) → words.
fn explain(reason: &str) -> String {
    match reason {
        "no pending link" | "link expired" | "invalid link token" => EXPIRED.into(),
        "already linked to another account" => BOTH_ACCOUNTS.into(),
        "" | "link rejected" => "Your other device turned the link down. Show a new code on it and try again.".into(),
        r if r.starts_with("invalid") || r.starts_with("unsupported") => "Your other device couldn't read that link. Update DropBeam on both devices and try again.".into(),
        r => r.to_owned(),
    }
}

pub struct PendingLink { token: [u8; 16], created_at: Instant }
/// A code is on screen (either kind): an offer may arrive, so allow a big frame.
pub(crate) fn pending_active(st: &AppState) -> bool {
    let active = |p: &mut Option<PendingLink>| {
        if p.as_ref().is_some_and(|p| p.created_at.elapsed() >= TTL) { *p = None; }
        p.is_some()
    };
    active(&mut st.pending_link.lock().unwrap()) | with_host(&st.config_dir, active)
}
fn consume(pending: &mut Option<PendingLink>, token: &str) -> Result<(), String> {
    let p = pending.as_ref().ok_or("no pending link")?;
    if p.created_at.elapsed() >= TTL { *pending = None; return Err("link expired".into()); }
    let bytes: [u8; 16] = hex::decode(token).ok().and_then(|v| v.try_into().ok()).ok_or("invalid link token")?;
    // Fixed-size constant-time comparison provided by the existing crypto library.
    #[allow(deprecated)]
    ring::constant_time::verify_slices_are_equal(&p.token, &bytes).map_err(|_| "invalid link token")?;
    *pending = None;
    Ok(())
}
/// Accept the token of whichever code this device has on screen (the scanning
/// device may have chosen either direction). One use, then both are cleared.
fn consume_any(st: &AppState, token: &str) -> Result<(), String> {
    let mut link = st.pending_link.lock().unwrap();
    with_host(&st.config_dir, |host| {
    let a = consume(&mut link, token);
    if a.is_ok() { *host = None; return Ok(()); }
    let b = consume(host, token);
    if b.is_ok() { *link = None; return Ok(()); }
    // The more telling reason: a code that was on screen beats "no code".
    Err(if a.as_ref().err().map(String::as_str) == Some("no pending link") { b.unwrap_err() } else { a.unwrap_err() })
    })
}
/// A refusal about the token itself (a stale or foreign scan): the code on
/// screen, if any, is still good, so the screen showing it shouldn't fail.
fn token_error(e: &str) -> bool {
    matches!(e, "no pending link" | "link expired" | "invalid link token")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct LinkCode {
    v: u8, eid: String, name: String, token: String,
    /// The shower's account (hex public key), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acct: Option<String>,
    /// How many OTHER devices share that account (absent in older codes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    devices: Option<u32>,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum CodeKind { Link, Join }
fn encode(code: &LinkCode) -> String { format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(code).unwrap())) }
fn encode_join(code: &LinkCode) -> String { format!("{JOIN_PREFIX}{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(code).unwrap())) }
#[cfg(test)]
fn parse(code: &str) -> Result<LinkCode, String> { parse_with(code, PREFIX) }
fn parse_with(code: &str, prefix: &str) -> Result<LinkCode, String> {
    if code.len() > 8192 { return Err(NOT_A_CODE.into()); }
    let payload = crate::codes::strip_prefix(code, prefix).ok_or(NOT_A_CODE)?;
    let c: LinkCode = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim()).map_err(|_| NOT_A_CODE)?).map_err(|_| NOT_A_CODE)?;
    if c.v != 1 || hex::decode(&c.token).map_or(true, |v| v.len() != 16) || c.eid.parse::<iroh::EndpointId>().is_err() { return Err(NOT_A_CODE.into()); }
    Ok(c)
}
/// Either device code, or a clear word on what was scanned instead.
fn parse_any(code: &str) -> Result<(LinkCode, CodeKind), String> {
    if crate::codes::strip_prefix(code, PREFIX).is_some() { return Ok((parse_with(code, PREFIX)?, CodeKind::Link)); }
    if crate::codes::strip_prefix(code, JOIN_PREFIX).is_some() { return Ok((parse_with(code, JOIN_PREFIX)?, CodeKind::Join)); }
    let c = code.trim().to_ascii_lowercase();
    if c.starts_with("dropbeam:") || c.starts_with("dropbeamf1:") { return Err(FRIEND_CODE.into()); }
    Err(NOT_A_CODE.into())
}
/// A device-link code of either kind (so friend flows can point elsewhere).
pub(crate) fn is_device_code(code: &str) -> bool {
    crate::codes::strip_prefix(code, PREFIX).is_some() || crate::codes::strip_prefix(code, JOIN_PREFIX).is_some()
}
/// This device's account id (hex public key), if it belongs to an account.
pub(crate) fn account_pub(dir: &Path) -> Option<String> {
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    read_key(dir).ok().flatten().map(|k| hex::encode(k.public().as_bytes()))
}
/// The account's signature over `endpoint`, proving this device holds the key.
pub(crate) fn sign_endpoint(dir: &Path, endpoint: &str) -> Option<String> {
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    read_key(dir).ok().flatten().map(|k| hex::encode(k.sign(endpoint.as_bytes()).to_bytes()))
}
/// Leave the account: this device forgets the key (it can be linked again).
pub(crate) fn forget_key(dir: &Path) {
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    let _ = std::fs::remove_file(dir.join("account.key"));
}
#[cfg(test)]
pub(crate) fn adopt_key_for_tests(dir: &Path, key: &iroh::SecretKey) { write_key(dir, key).unwrap(); }
fn read_key(dir: &Path) -> Result<Option<iroh::SecretKey>, String> {
    match std::fs::read(dir.join("account.key")) {
        Ok(bytes) => Ok(Some(iroh::SecretKey::from_bytes(&bytes.try_into().map_err(|_| "invalid account key")?))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("cannot read account key".into()),
    }
}
fn write_key(dir: &Path, key: &iroh::SecretKey) -> Result<(), String> {
    use std::io::Write;
    let path = dir.join(format!(".account-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&path)?;
        file.write_all(&key.to_bytes())?;
        file.sync_all()?;
        std::fs::rename(&path, dir.join("account.key"))
    })();
    let _ = std::fs::remove_file(path);
    result.map_err(|_: std::io::Error| "cannot save account key".into())
}
fn account(dir: &Path) -> Result<iroh::SecretKey, String> {
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    if let Some(key) = read_key(dir)? { return Ok(key); }
    let key = iroh::SecretKey::generate();
    write_key(dir, &key)?;
    Ok(key)
}
pub(crate) fn verify_account(public: &str, signature: &str, endpoint: &str) -> bool {
    let Ok(bytes) = hex::decode(public) else { return false; };
    let Ok(bytes) = <[u8; 32]>::try_from(bytes) else { return false; };
    let Ok(key) = iroh::PublicKey::from_bytes(&bytes) else { return false; };
    let Ok(sig) = hex::decode(signature) else { return false; };
    let Ok(sig) = <[u8; 64]>::try_from(sig) else { return false; };
    key.verify(endpoint.as_bytes(), &iroh::Signature::from_bytes(&sig)).is_ok()
}
pub(crate) fn profile(state: &IrohState, endpoint: &str) -> Value {
    let Some(st) = state.app.get().and_then(|a| a.try_state::<Arc<AppState>>()) else { return json!({}); };
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    // Ordinary hello does not create an account; only explicit account commands do.
    let key = read_key(&st.config_dir).ok().flatten();
    let kind = st.settings.lock().unwrap().device_kind.clone();
    let mut out = json!({"device_kind": kind, "device_os": std::env::consts::OS,
        "device_name": crate::account::device_name(&kind),
        "account_pub": key.as_ref().map(|k| hex::encode(k.public().as_bytes())),
        "account_sig": key.map(|k| hex::encode(k.sign(endpoint.as_bytes()).to_bytes()))});
    drop(_guard);
    let left = crate::account::left_notice(&st.config_dir);
    if !left.is_null() { out["left_accounts"] = left; }
    out
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct LinkResult { endpoint_id: String, name: String, device_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] device_os: Option<String>,
    /// What came over (or went over) in the link, for the success screen.
    #[serde(default, skip_serializing_if = "Option::is_none")] friends: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")] messages: Option<usize> }
#[derive(Serialize)]
pub struct DeviceInfo { #[serde(flatten)] device: LinkResult, account_pub: String, linked_devices: usize,
    device_os: &'static str, devices: Vec<crate::account::DeviceView>,
    /// The person's name (the same on every device in the account).
    display_name: String }
fn device(st: &AppState, net: &IrohState) -> Result<LinkResult, String> {
    let endpoint_id = net.get().ok_or(NOT_READY)?.id().to_string();
    let s = st.settings.lock().unwrap();
    Ok(LinkResult { endpoint_id, name: crate::account::device_name(&s.device_kind), device_kind: s.device_kind.clone(),
        device_os: Some(std::env::consts::OS.into()), friends: None, messages: None })
}
/// This device's account and how many OTHER devices share it right now.
fn account_state(dir: &Path) -> (Option<String>, u32) {
    (account_pub(dir), crate::account::own_devices(dir).len() as u32)
}
#[tauri::command]
pub fn my_device_info(state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<DeviceInfo, String> {
    let device = device(&state, &iroh)?;
    // Never mint a key just to answer a query: a fresh device must stay unlinked
    // until it either links a new device (link_device_send) or receives an offer,
    // otherwise its own throwaway key would block adopting the real account.
    let account_pub = { let _guard = ACCOUNT_LOCK.lock().unwrap(); read_key(&state.config_dir)?.map(|k| hex::encode(k.public().as_bytes())).unwrap_or_default() };
    let devices = if account_pub.is_empty() { vec![] } else { crate::account::device_views(&state, &device.endpoint_id) };
    let linked_devices = devices.len().saturating_sub(1);
    let display_name = state.settings.lock().unwrap().display_name.clone();
    Ok(DeviceInfo { device, account_pub, linked_devices, device_os: std::env::consts::OS, devices, display_name })
}
fn new_code(st: &AppState, net: &IrohState) -> Result<(LinkCode, [u8; 16]), String> {
    let me = device(st, net)?;
    let token: [u8; 16] = rand::random();
    let (acct, devices) = account_state(&st.config_dir);
    Ok((LinkCode { v: 1, eid: me.endpoint_id, name: me.name, token: hex::encode(token), acct, devices: Some(devices) }, token))
}
#[tauri::command]
pub fn link_device_begin(state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<String, String> {
    let (code, token) = new_code(&state, &iroh)?;
    *state.pending_link.lock().unwrap() = Some(PendingLink { token, created_at: Instant::now() });
    Ok(encode(&code))
}
#[tauri::command]
pub fn link_device_cancel(state: State<'_, Arc<AppState>>) { *state.pending_link.lock().unwrap() = None; }

fn export_thread(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    messages.sort_by_key(|m| (m.seq, m.ts, m.id.clone()));
    if messages.len() > 500 { messages.drain(..messages.len() - 500); }
    for m in &mut messages { m.path = None; }
    messages
}
/// Friends + chats count in an offer (for progress and the success screen).
fn offer_counts(offer: &Value) -> (usize, usize) {
    let friends = offer["friends"].as_array().map_or(0, |a| a.len());
    let messages = offer["chats"].as_object().map_or(0, |m| m.values().map(|t| t.as_array().map_or(0, |a| a.len())).sum());
    (friends, messages)
}
fn offer(st: &AppState, net: &IrohState, code: &LinkCode, key: &iroh::SecretKey) -> Result<Value, String> {
    let mut chats = HashMap::new();
    let records: Vec<Value> = friends::load(&st.config_dir).into_iter()
        // The device being linked doesn't need itself as a contact.
        .filter(|f| f.endpoint_id.as_deref() != Some(code.eid.as_str()))
        .map(|mut f| {
        let thread = export_thread(chat::messages(&st.config_dir, &f.id));
        if !thread.is_empty() { chats.insert(f.id.clone(), thread); }
        let avatar = f.avatar.take().and_then(|p| {
            let md = std::fs::metadata(&p).ok()?;
            if md.len() > MAX_AVATAR as u64 { return None; }
            let bytes = std::fs::read(p).ok()?;
            (bytes.len() <= MAX_AVATAR).then(|| STANDARD.encode(bytes))
        });
        f.secret.clear();
        let mut value = serde_json::to_value(f).unwrap();
        value.as_object_mut().unwrap().remove("avatar");
        value["avatar_b64"] = json!(avatar);
        value
    }).collect();
    let sender = device(st, net)?;
    Ok(json!({"kind":"link-offer", "v":1, "token":code.token,
        "account_seed_hex":hex::encode(key.to_bytes()), "account_pub":hex::encode(key.public().as_bytes()),
        "account_sig":hex::encode(key.sign(sender.endpoint_id.as_bytes()).to_bytes()),
        "sender":sender, "friends":records, "chats":chats}))
}

/// Progress for the linking screens ("Bringing over 7 friends and 309 messages…").
fn progress(app: Option<&AppHandle>, stage: &str, counts: (usize, usize)) {
    if let Some(app) = app {
        let _ = app.emit("link://progress", json!({"stage": stage, "friends": counts.0, "messages": counts.1}));
    }
}

/// Which way this link runs, given who already shares an account.
#[derive(Debug, PartialEq)]
enum Direction { Give, Take }
fn direction(kind: CodeKind, code: &LinkCode, mine: &Option<String>, my_devices: u32) -> Result<Direction, &'static str> {
    let by_code = if kind == CodeKind::Link { Direction::Give } else { Direction::Take };
    // An older build's code doesn't say, and only accepts its own direction.
    let Some(theirs) = code.devices else { return Ok(by_code) };
    let theirs_shared = theirs > 0;
    let same = code.acct.is_some() && code.acct == *mine;
    if my_devices > 0 && theirs_shared && !same { return Err(BOTH_ACCOUNTS); }
    // The account that already spans devices is the one both end up in; with
    // none (or the same one), the code decides.
    Ok(if my_devices > 0 && !theirs_shared { Direction::Give }
        else if theirs_shared && my_devices == 0 { Direction::Take }
        else { by_code })
}

/// Link with a scanned device code of either kind. The Tauri commands for both
/// codes land here, so a code scanned on the "wrong" screen still works.
async fn link_with_code(app: &AppHandle, st: &Arc<AppState>, iroh: &Arc<IrohState>, code: &str) -> Result<LinkResult, String> {
    let me = device(st, iroh)?;
    let (code, way) = plan_link(&st.config_dir, &me.endpoint_id, code).inspect_err(|e| {
        if e == ALREADY { crate::account::account_sync_now(); }
    })?;
    match way {
        Direction::Give => give(app, st, iroh, &code).await,
        Direction::Take => take(app, st, iroh, me, &code).await,
    }
}
/// Everything decided before dialing: is it a device code, is it ours, are the
/// devices already linked or in two different accounts, and which way to go.
fn plan_link(dir: &Path, me: &str, code: &str) -> Result<(LinkCode, Direction), String> {
    let (code, kind) = parse_any(code)?;
    if code.eid == me { return Err(SELF_CODE.into()); }
    let (mine, my_devices) = account_state(dir);
    if mine.is_some() && code.acct == mine && crate::account::is_own_device(dir, &code.eid) {
        return Err(ALREADY.into());
    }
    let way = direction(kind, &code, &mine, my_devices)?;
    Ok((code, way))
}
#[tauri::command]
pub async fn link_device_send(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>, code: String) -> Result<LinkResult, String> {
    link_with_code(&app, state.inner(), iroh.inner(), &code).await
}

/// This device gives its account to the device whose code was scanned: push
/// the offer (the shower checks its one-time token and adopts it).
async fn give(app: &AppHandle, st: &Arc<AppState>, iroh: &Arc<IrohState>, code: &LinkCode) -> Result<LinkResult, String> {
    let ep = iroh.get().ok_or(NOT_READY)?.clone();
    let key = account(&st.config_dir)?;
    let req = offer(st, iroh, code, &key)?;
    if serde_json::to_vec(&req).map_err(|_| "cannot encode link")?.len() > MAX_OFFER { return Err("Your chat history is too large to link in one go (over 64 MB).".into()); }
    let counts = offer_counts(&req);
    progress(Some(app), "sending", counts);
    let dialed = tokio::time::timeout(Duration::from_secs(20), async {
        let conn = ep.connect(iroh_net::dial_addr(code.eid.parse()?), iroh_net::ALPN).await?;
        let (send, recv) = conn.open_bi().await?;
        anyhow::Ok((conn, send, recv))
    }).await;
    let (_conn, mut send, mut recv) = match dialed { Ok(Ok(v)) => v, _ => return Err(UNREACHABLE.into()) };
    // Generous: the offer carries the recent history (up to 64 MB over a relay).
    let reply = tokio::time::timeout(Duration::from_secs(240), async {
        iroh_net::write_frame(&mut send, &req).await?;
        send.finish()?;
        iroh_net::read_frame(&mut recv).await
    }).await.map_err(|_| DROPPED)?.map_err(|_| DROPPED)?;
    if reply["kind"] != "link-ok" { return Err(explain(reply["reason"].as_str().unwrap_or(""))); }
    let mut result: LinkResult = serde_json::from_value(reply.clone()).map_err(|_| "invalid link reply")?;
    if result.endpoint_id != code.eid || !verify_account(&hex::encode(key.public().as_bytes()), reply["account_sig"].as_str().unwrap_or(""), &result.endpoint_id) { return Err("invalid link identity".into()); }
    (result.friends, result.messages) = (Some(counts.0), Some(counts.1));
    record_new_device(app, st, iroh, &key, &result);
    Ok(result)
}

/// Both link flows end here on the device that already had the account: the
/// new device is an own device from now on, and every other own device hears
/// about it on the next account sync.
fn record_new_device(app: &AppHandle, st: &AppState, iroh: &Arc<IrohState>, key: &iroh::SecretKey, result: &LinkResult) {
    record_new_device_data(st, iroh, key, result);
    iroh_net::broadcast_profile(app.clone(), iroh.clone());
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("link://linked", result);
    crate::account::account_sync_now();
}

fn record_new_device_data(st: &AppState, iroh: &IrohState, key: &iroh::SecretKey, result: &LinkResult) {
    let account = hex::encode(key.public().as_bytes());
    if let Some(me) = iroh.get().map(|e| e.id().to_string()) { crate::account::mark_linked(&st.config_dir, &me); }
    crate::account::mark_linked(&st.config_dir, &result.endpoint_id);
    // A former plain contact (the user once added their own device as a friend)
    // becomes one of their devices; its name comes from the device itself.
    friends::upsert_own_device(&st.config_dir, &result.endpoint_id, &result.name, Some(&result.device_kind),
        result.device_os.as_deref(), &account, crate::chat::now_ms(), true);
}

// ── Reverse flow: the device that HAS the account shows a code, the new one scans it.

/// The "join me" code on screen, per config dir (one app has one; tests many).
static HOST_PENDING: Mutex<Option<HashMap<std::path::PathBuf, PendingLink>>> = Mutex::new(None);
fn with_host<T>(dir: &Path, f: impl FnOnce(&mut Option<PendingLink>) -> T) -> T {
    let mut all = HOST_PENDING.lock().unwrap();
    let map = all.get_or_insert_with(HashMap::new);
    let mut slot = map.remove(dir);
    let out = f(&mut slot);
    if let Some(p) = slot { map.insert(dir.to_path_buf(), p); }
    out
}

/// Show a code another (new) device can scan to join this device's account.
#[tauri::command]
pub fn link_host_begin(state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<String, String> {
    let (code, token) = new_code(&state, &iroh)?;
    with_host(&state.config_dir, |p| *p = Some(PendingLink { token, created_at: Instant::now() }));
    Ok(encode_join(&code))
}
#[tauri::command]
pub fn link_host_cancel(state: State<'_, Arc<AppState>>) { with_host(&state.config_dir, |p| *p = None); }

/// Link with a scanned device code (either kind — see `link_with_code`).
#[tauri::command]
pub async fn link_device_join(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>, code: String) -> Result<LinkResult, String> {
    link_with_code(&app, state.inner(), iroh.inner(), &code).await
}

/// This device takes the account of the device whose code was scanned: it
/// receives the same offer `give` would push, over one stream.
async fn take(app: &AppHandle, st: &Arc<AppState>, iroh: &Arc<IrohState>, me: LinkResult, code: &LinkCode) -> Result<LinkResult, String> {
    let ep = iroh.get().ok_or(NOT_READY)?.clone();
    let dialed = tokio::time::timeout(Duration::from_secs(20), async {
        let conn = ep.connect(iroh_net::dial_addr(code.eid.parse()?), iroh_net::ALPN).await?;
        let (send, recv) = conn.open_bi().await?;
        anyhow::Ok((conn, send, recv))
    }).await;
    let (_conn, mut send, mut recv) = match dialed { Ok(Ok(v)) => v, _ => return Err(UNREACHABLE.into()) };
    let host_info = join_over(st, Some(app), me, code, &mut send, &mut recv).await?;
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("chat://changed", ());
    iroh_net::broadcast_profile(app.clone(), iroh.clone());
    crate::account::account_sync_now();
    Ok(host_info)
}

/// New-device half of the join flow over an open stream to the host.
async fn join_over(st: &AppState, app: Option<&AppHandle>, me: LinkResult, code: &LinkCode, send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream) -> Result<LinkResult, String> {
    let token: [u8; 16] = hex::decode(&code.token).ok().and_then(|v| v.try_into().ok()).ok_or(NOT_A_CODE)?;
    iroh_net::write_frame(send, &json!({"kind":"link-join", "v":1, "token":code.token, "device":me}))
        .await.map_err(|_| UNREACHABLE)?;
    progress(app, "waiting", (0, 0));
    let offer = tokio::time::timeout(Duration::from_secs(240), iroh_net::read_frame_cap(recv, MAX_OFFER)).await
        .map_err(|_| DROPPED)?.map_err(|_| DROPPED)?;
    if offer["kind"] != "link-offer" {
        return Err(explain(offer["reason"].as_str().unwrap_or("")));
    }
    // The offer must answer THIS dial (same one-time token, from the scanned host);
    // it's bound to the connection, so no other peer can push an account here.
    let echoed: Option<[u8; 16]> = offer["token"].as_str().and_then(|t| hex::decode(t).ok()).and_then(|v| v.try_into().ok());
    #[allow(deprecated)]
    let same = echoed.is_some_and(|e| ring::constant_time::verify_slices_are_equal(&e, &token).is_ok());
    if !same { return Err("invalid link offer".into()); }
    let mut host_info: LinkResult = serde_json::from_value(offer["sender"].clone()).map_err(|_| "invalid link offer")?;
    let counts = offer_counts(&offer);
    progress(app, "importing", counts);
    let result = adopt_offer(st, me, &code.eid, &offer);
    let reply = result.as_ref().cloned().unwrap_or_else(|e| json!({"kind":"link-error", "reason":e}));
    let _ = iroh_net::write_frame(send, &reply).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
    result.map_err(|e| explain(&e))?;
    (host_info.friends, host_info.messages) = (Some(counts.0), Some(counts.1));
    Ok(host_info)
}

/// Host side of the join flow: check the one-time code, send the offer, then
/// record the new device once it proves it adopted the account.
pub(crate) async fn serve_join(net: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<()> {
    let app = net.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.clone();
    let st = app.state::<Arc<AppState>>();
    let iroh = app.state::<Arc<IrohState>>().inner().clone();
    match host_join_over(&st, Some(&app), net, who, req, send, recv).await {
        Ok(Ok((key, result))) => {
            record_new_device_data(&st, net, &key, &result);
            iroh_net::broadcast_profile(app.clone(), iroh.clone());
            let _ = app.emit("friends://changed", ());
            let _ = app.emit("link://linked", &result);
            crate::account::account_sync_now();
        }
        Ok(Err(reason)) => { if !token_error(&reason) { let _ = app.emit("link://failed", explain(&reason)); } }
        Err(e) => { let _ = app.emit("link://failed", DROPPED); return Err(e); }
    }
    Ok(())
}

/// Host half of the join flow over an open stream (the first frame is `req`).
/// Ok(Err(reason)) = the join was refused or failed on the new device.
async fn host_join_over(st: &AppState, app: Option<&AppHandle>, net: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<Result<(iroh::SecretKey, LinkResult), String>> {
    let token = req["token"].as_str().unwrap_or("").to_owned();
    let prepared = (|| -> Result<(iroh::SecretKey, Value), String> {
        consume_any(st, &token)?;
        if req["v"] != 1 { return Err("unsupported link version".into()); }
        let newcomer: LinkResult = serde_json::from_value(req["device"].clone()).map_err(|_| "invalid device")?;
        if newcomer.endpoint_id != who { return Err("invalid device identity".into()); }
        let key = account(&st.config_dir)?;
        let code = LinkCode { v: 1, eid: who.to_owned(), name: newcomer.name, token: token.clone(), acct: None, devices: None };
        let offer = offer(st, net, &code, &key)?;
        if serde_json::to_vec(&offer).map_err(|_| "cannot encode link")?.len() > MAX_OFFER { return Err("link history exceeds 64 MiB".into()); }
        Ok((key, offer))
    })();
    let (key, offer) = match prepared {
        Ok(v) => v,
        Err(e) => {
            iroh_net::write_frame(send, &json!({"kind":"link-error", "reason":e})).await?;
            send.finish()?;
            return Ok(Err(e));
        }
    };
    let counts = offer_counts(&offer);
    progress(app, "sending", counts);
    iroh_net::write_frame(send, &offer).await?;
    send.finish()?;
    let reply = tokio::time::timeout(Duration::from_secs(240), iroh_net::read_frame(recv)).await??;
    if reply["kind"] != "link-ok" {
        return Ok(Err(reply["reason"].as_str().unwrap_or("link rejected").to_owned()));
    }
    let mut result: LinkResult = serde_json::from_value(reply.clone())?;
    if result.endpoint_id != who || !verify_account(&hex::encode(key.public().as_bytes()), reply["account_sig"].as_str().unwrap_or(""), who) {
        anyhow::bail!("invalid link identity");
    }
    (result.friends, result.messages) = (Some(counts.0), Some(counts.1));
    Ok(Ok((key, result)))
}

fn receive(st: &AppState, me: LinkResult, who: &str, req: &Value) -> Result<Value, String> {
    consume_any(st, req["token"].as_str().unwrap_or(""))?;
    adopt_offer(st, me, who, req)
}
/// Apply an authenticated link offer (the caller has already checked it is the
/// one this device asked for). Order matters for a link cut short: the key and
/// the sending device are recorded FIRST, so even if the import below is
/// interrupted, the regular account sync with that device brings the rest.
fn adopt_offer(st: &AppState, me: LinkResult, who: &str, req: &Value) -> Result<Value, String> {
    if req["v"] != 1 { return Err("unsupported link version".into()); }
    let sender: LinkResult = serde_json::from_value(req["sender"].clone()).map_err(|_| "invalid sender")?;
    if sender.endpoint_id != who || who == me.endpoint_id { return Err("invalid sender identity".into()); }
    let seed: [u8; 32] = hex::decode(req["account_seed_hex"].as_str().unwrap_or("")).ok().and_then(|v| v.try_into().ok()).ok_or("invalid account key")?;
    let key = iroh::SecretKey::from_bytes(&seed);
    let public = hex::encode(key.public().as_bytes());
    if req["account_pub"].as_str() != Some(&public) || !verify_account(&public, req["account_sig"].as_str().unwrap_or(""), who) { return Err("invalid account identity".into()); }
    let records = req["friends"].as_array().ok_or("invalid friends")?;
    let mut validated = Vec::new();
    for value in records {
        let f: crate::models::Friend = serde_json::from_value(value.clone()).map_err(|_| "invalid friend")?;
        if let Some(eid) = f.endpoint_id.as_deref() {
            let parsed = eid.parse::<iroh::EndpointId>().map_err(|_| "invalid friend endpoint")?;
            if parsed.to_string() != eid { return Err("invalid friend endpoint".into()); }
        }
        let avatar = value["avatar_b64"].as_str().map(|s| {
            if s.len() > MAX_AVATAR * 4 / 3 + 4 { return Err("avatar too large"); }
            let bytes = STANDARD.decode(s).map_err(|_| "invalid avatar")?;
            if bytes.len() > MAX_AVATAR { return Err("avatar too large"); }
            Ok(bytes)
        }).transpose()?;
        validated.push((f, avatar));
    }
    let chats: HashMap<String, Vec<ChatMessage>> = serde_json::from_value(req["chats"].clone()).map_err(|_| "invalid chats")?;
    // Refuse only when this device already belongs to an account that other
    // devices share; a key nobody else references is simply replaced. Checked
    // (and the key written) under the lock, with the friend list read first.
    let others = crate::account::own_devices(&st.config_dir);
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    if let Some(old) = read_key(&st.config_dir)? {
        if old.to_bytes() != seed {
            let old_pub = hex::encode(old.public().as_bytes());
            if others.iter().any(|f| f.account_pub.as_deref() == Some(&old_pub)) { return Err("already linked to another account".into()); }
        }
    }
    write_key(&st.config_dir, &key)?;
    drop(_guard);
    crate::account::forget_left(&st.config_dir, &public);
    crate::account::mark_linked(&st.config_dir, &me.endpoint_id);
    crate::account::mark_linked(&st.config_dir, who);
    friends::upsert_own_device(&st.config_dir, who, &sender.name, Some(&sender.device_kind), sender.device_os.as_deref(),
        &public, crate::chat::now_ms(), true);
    for (f, avatar) in validated {
        let eid = f.endpoint_id.as_deref();
        if eid.is_some_and(|id| id == me.endpoint_id || id == who) { continue; }
        // The account's other devices: the sender holds the key, so it speaks
        // for them (exactly as its roster will on the first sync).
        if let (Some(eid), Some(true)) = (eid, f.account_pub.as_deref().map(|a| a == public)) {
            friends::upsert_own_device(&st.config_dir, eid, &f.name, f.device_kind.as_deref(), f.device_os.as_deref(), &public, f.created_at, true);
            continue;
        }
        let local = friends::import_link_friend(&st.config_dir, &f);
        // An imported claim has no signature. Preserve any locally verified claim.
        if let Some(eid) = eid { friends::set_device_info(&st.config_dir, eid, f.device_kind.as_deref(), None); }
        if let Some(bytes) = avatar {
            let avatar_id = eid.map(str::to_owned).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let path = st.config_dir.join(format!("friend-avatar-{avatar_id}.jpg"));
            if std::fs::write(&path, bytes).is_ok() {
                friends::set_link_avatar(&st.config_dir, &local.id, path.to_string_lossy().into_owned());
            }
        }
        if let Some(messages) = chats.get(&f.id) {
            chat::import_link_thread(&st.config_dir, &local.id, export_thread(messages.clone()));
        }
    }
    friends::fold_person_threads(&st.config_dir);
    Ok(json!({"kind":"link-ok", "endpoint_id":me.endpoint_id, "name":me.name, "device_kind":me.device_kind, "device_os":me.device_os,
        "account_sig":hex::encode(key.sign(me.endpoint_id.as_bytes()).to_bytes())}))
}
pub(crate) async fn serve(net: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream) -> anyhow::Result<()> {
    let app = net.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?;
    let st = app.state::<Arc<AppState>>();
    let counts = offer_counts(req);
    if pending_active(&st) { progress(Some(app), "importing", counts); }
    let result = device(&st, net).and_then(|me| receive(&st, me, who, req));
    let reply = result.as_ref().cloned().unwrap_or_else(|e| json!({"kind":"link-error", "reason":e}));
    let written = iroh_net::write_frame(send, &reply).await;
    match &result {
        Ok(_) => {
            let _ = app.emit("friends://changed", ());
            let _ = app.emit("chat://changed", ());
            if let Ok(mut sender) = serde_json::from_value::<LinkResult>(req["sender"].clone()) {
                (sender.friends, sender.messages) = (Some(counts.0), Some(counts.1));
                let _ = app.emit("link://linked", &sender);
            }
            if let Some(net) = app.try_state::<Arc<IrohState>>() { iroh_net::broadcast_profile(app.clone(), net.inner().clone()); }
            crate::account::account_sync_now();
        }
        Err(e) if !token_error(e) => { let _ = app.emit("link://failed", explain(e)); }
        Err(_) => {}
    }
    written?;
    send.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("db-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap(); p
    }
    #[test]
    fn code_roundtrip_and_case_insensitive_prefix() {
        let c = LinkCode { v: 1, eid: iroh::SecretKey::generate().public().to_string(), name: "Ashton's iPhone 📱".into(), token: hex::encode([3;16]), acct: None, devices: None };
        let encoded = encode(&c);
        for value in [encoded.clone(), encoded.replacen(PREFIX, "DROPBEAMLINK1:", 1)] {
            let decoded = parse(&format!(" {value}\n")).unwrap();
            assert_eq!(decoded.eid, c.eid); assert_eq!(decoded.name, c.name); assert_eq!(decoded.token, c.token);
        }
        assert!(parse("dropbeamlink1:garbage").is_err());
        assert!(parse(&encoded.replace(PREFIX, "dropbeam:")).is_err());
    }
    #[test]
    fn pending_token_is_single_use_and_expires() {
        let mut p = Some(PendingLink { token: [4;16], created_at: Instant::now() });
        assert!(consume(&mut p, &hex::encode([5;16])).is_err()); assert!(p.is_some());
        assert!(consume(&mut p, &hex::encode([4;16])).is_ok());
        assert!(consume(&mut p, &hex::encode([4;16])).is_err());
        p = Some(PendingLink { token: [4;16], created_at: Instant::now() - TTL });
        assert!(consume(&mut p, &hex::encode([4;16])).is_err()); assert!(p.is_none());
    }
    #[test]
    fn account_seed_is_stable_and_signature_binds_endpoint() {
        let d = dir(); let key = account(&d).unwrap();
        assert_eq!(account(&d).unwrap().to_bytes(), key.to_bytes());
        assert_eq!(std::fs::read(d.join("account.key")).unwrap().len(), 32);
        let public = hex::encode(key.public().as_bytes());
        let sig = hex::encode(key.sign(b"endpoint").to_bytes());
        assert!(verify_account(&public, &sig, "endpoint"));
        assert!(!verify_account(&public, &sig, "other"));
        assert!(!verify_account(&public, "broken", "endpoint"));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn chat_snapshot_caps_strips_paths_and_merges_without_duplicates() {
        let d = dir();
        let messages: Vec<ChatMessage> = (0..2100).map(|i| serde_json::from_value(json!({
            "id":i.to_string(), "peerId":"offered", "fromMe":true, "kind":"file", "text":"", "files":["a.jpg"],
            "bytes":1, "path":"/private/a.jpg", "status":"read", "ts":i, "seq":i
        })).unwrap()).collect();
        let exported = export_thread(messages.clone());
        assert_eq!(exported.len(), 500); assert_eq!(exported[0].id, "1600");
        assert!(exported.iter().all(|m| m.path.is_none() && m.status.as_deref() == Some("read")));
        chat::import_link_thread(&d, "local", exported.clone());
        chat::import_link_thread(&d, "local", exported);
        assert_eq!(chat::messages(&d, "local").len(), 500);
        chat::import_link_thread(&d, "local", messages);
        let merged = chat::messages(&d, "local");
        assert_eq!(merged.len(), 2000); assert!(merged.iter().all(|m| m.path.is_none() && m.peer_id == "local"));
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[cfg(test)]
mod receive_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    fn state() -> AppState {
        let config_dir = std::env::temp_dir().join(format!("db-link-receive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config_dir).unwrap();
        AppState { config_dir, settings: Mutex::new(Default::default()),
            pending_link: Mutex::new(Some(PendingLink { token: [7;16], created_at: Instant::now() })),
            transfers: Mutex::new(HashMap::new()), offers: Mutex::new(HashMap::new()),
            force_quit: AtomicBool::new(false), main_focused: AtomicBool::new(false), active_chat: Mutex::new(None) }
    }
    fn me() -> LinkResult { LinkResult { endpoint_id: iroh::SecretKey::generate().public().to_string(), name: "Phone".into(), device_kind: "phone".into(), device_os: Some("ios".into()), friends: None, messages: None } }
    fn offer(key: &iroh::SecretKey, who: &str) -> Value {
        json!({"kind":"link-offer", "v":1, "token":hex::encode([7;16]),
            "account_seed_hex":hex::encode(key.to_bytes()), "account_pub":hex::encode(key.public().as_bytes()),
            "account_sig":hex::encode(key.sign(who.as_bytes()).to_bytes()),
            "sender":{"endpoint_id":who,"name":"Laptop","device_kind":"laptop"}, "friends":[], "chats":{}})
    }
    #[test]
    fn receiver_adopts_key_adds_sender_and_authenticates_reply() {
        let st = state(); let key = iroh::SecretKey::generate(); let who = iroh::SecretKey::generate().public().to_string();
        let req = offer(&key, &who);
        let reply = receive(&st, me(), &who, &req).unwrap();
        assert_eq!(reply["kind"], "link-ok");
        assert!(verify_account(req["account_pub"].as_str().unwrap(), reply["account_sig"].as_str().unwrap(), reply["endpoint_id"].as_str().unwrap()));
        assert_eq!(read_key(&st.config_dir).unwrap().unwrap().to_bytes(), key.to_bytes());
        let f = friends::load(&st.config_dir).remove(0);
        assert_eq!(f.endpoint_id.as_deref(), Some(who.as_str())); assert_eq!(f.device_kind.as_deref(), Some("laptop"));
        assert_eq!(f.account_pub.as_deref(), req["account_pub"].as_str());
        assert!(receive(&st, me(), &who, &req).is_err());
        std::fs::remove_dir_all(st.config_dir).unwrap();
    }
    #[test]
    fn receiver_refuses_other_account_and_wrong_sender_without_overwrite() {
        let st = state(); let old = account(&st.config_dir).unwrap();
        friends::upsert_by_endpoint(&st.config_dir, "existing", "Existing");
        // "Existing" is a device that shares this account, so the account is in use.
        friends::set_device_info(&st.config_dir, "existing", Some("laptop"), Some(&hex::encode(old.public().as_bytes())));
        let key = iroh::SecretKey::generate(); let who = iroh::SecretKey::generate().public().to_string();
        let req = offer(&key, &who);
        assert_eq!(receive(&st, me(), &who, &req).unwrap_err(), "already linked to another account");
        assert_eq!(read_key(&st.config_dir).unwrap().unwrap().to_bytes(), old.to_bytes());
        *st.pending_link.lock().unwrap() = Some(PendingLink { token: [7;16], created_at: Instant::now() });
        assert_eq!(receive(&st, me(), "imposter", &req).unwrap_err(), "invalid sender identity");
        assert_eq!(friends::load(&st.config_dir).len(), 1);
        std::fs::remove_dir_all(st.config_dir).unwrap();
    }
    #[test]
    fn receiver_maps_existing_thread_and_does_not_trust_imported_account_claim() {
        let st = state(); let key = iroh::SecretKey::generate(); let who = iroh::SecretKey::generate().public().to_string();
        let peer = iroh::SecretKey::generate().public().to_string();
        let existing = friends::upsert_by_endpoint(&st.config_dir, &peer, "Friend");
        let mut req = offer(&key, &who);
        let mut f = serde_json::to_value(&existing).unwrap();
        // A claim that the friend belongs to some OTHER account is unsigned: not trusted.
        f["id"] = json!("offered-thread"); f["accountPub"] = json!(hex::encode(iroh::SecretKey::generate().public().as_bytes()));
        f["deviceKind"] = json!("tablet"); f["secret"] = json!("");
        req["friends"] = json!([f]);
        req["chats"] = json!({"offered-thread":[{"id":"m", "peerId":"offered-thread", "fromMe":true,
            "kind":"text", "text":"hello", "files":[], "bytes":0, "path":"/secret", "status":"read", "ts":1}]});
        receive(&st, me(), &who, &req).unwrap();
        let f = friends::load(&st.config_dir).into_iter().find(|f| f.id == existing.id).unwrap();
        assert!(f.account_pub.is_none()); assert_eq!(f.device_kind.as_deref(), Some("tablet"));
        let msgs = chat::messages(&st.config_dir, &existing.id);
        assert_eq!(msgs.len(), 1); assert!(msgs[0].path.is_none()); assert_eq!(msgs[0].status.as_deref(), Some("read"));
        std::fs::remove_dir_all(st.config_dir).unwrap();
    }
    #[test]
    fn receiver_adopts_offer_when_its_own_key_is_unshared() {
        // A device that minted a key nobody references (e.g. by opening Settings)
        // must still be linkable: the offered account replaces the throwaway key.
        let st = state(); let old = account(&st.config_dir).unwrap();
        friends::upsert_by_endpoint(&st.config_dir, "someone", "Someone");
        let key = iroh::SecretKey::generate(); let who = iroh::SecretKey::generate().public().to_string();
        let req = offer(&key, &who);
        assert_eq!(receive(&st, me(), &who, &req).unwrap()["kind"], "link-ok");
        let now = read_key(&st.config_dir).unwrap().unwrap();
        assert_eq!(now.to_bytes(), key.to_bytes()); assert_ne!(now.to_bytes(), old.to_bytes());
        std::fs::remove_dir_all(st.config_dir).unwrap();
    }

    /// The reverse (scan-the-host) flow end to end over a real loopback connection.
    #[tokio::test]
    async fn join_flow_over_loopback_links_both_ways_and_rejects_bad_tokens() {
        use iroh::endpoint::presets;
        let host_ep = iroh::Endpoint::builder(presets::N0).alpns(vec![iroh_net::ALPN.to_vec()]).bind().await.unwrap();
        let new_ep = iroh::Endpoint::bind(presets::N0).await.unwrap();
        let (h, n) = (Arc::new(AppState::for_tests(state().config_dir)), Arc::new(AppState::for_tests(state().config_dir)));
        let host_net = Arc::new(IrohState::default());
        let _ = host_net.endpoint.set(host_ep.clone());
        // The host has a friend and a conversation to hand over.
        let mong = iroh::SecretKey::generate().public().to_string();
        let f = friends::upsert_by_endpoint(&h.config_dir, &mong, "Mong");
        chat::append(&h.config_dir, &serde_json::from_value(json!({"id":"m1","peerId":f.id,"fromMe":true,"kind":"text",
            "text":"hi","files":[],"bytes":0,"status":"read","ts":1})).unwrap());
        let run = |token: [u8; 16], shown: [u8; 16]| {
            let (h, n, host_net, host_ep, new_ep) = (h.clone(), n.clone(), host_net.clone(), host_ep.clone(), new_ep.clone());
            async move {
                with_host(&h.config_dir, |p| *p = Some(PendingLink { token, created_at: Instant::now() }));
                let addr = host_ep.addr();
                let host_id = host_ep.id().to_string();
                let server = tokio::spawn(async move {
                    let conn = host_ep.accept().await.unwrap().await.unwrap();
                    let who = conn.remote_id().to_string();
                    let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                    let req = iroh_net::read_frame(&mut recv).await.unwrap();
                    assert_eq!(req["kind"], "link-join");
                    let out = host_join_over(&h, None, &host_net, &who, &req, &mut send, &mut recv).await.unwrap();
                    if let Ok((key, result)) = &out { record_new_device_data(&h, &host_net, key, result); }
                    out.map(|(_, r)| r.endpoint_id)
                });
                let conn = new_ep.connect(addr, iroh_net::ALPN).await.unwrap();
                let (mut send, mut recv) = conn.open_bi().await.unwrap();
                let me = LinkResult { endpoint_id: new_ep.id().to_string(), name: "Phone".into(), device_kind: "phone".into(), device_os: Some("ios".into()), friends: None, messages: None };
                let code = LinkCode { v: 1, eid: host_id, name: "Mac".into(), token: hex::encode(shown), acct: None, devices: None };
                let joined = join_over(&n, None, me, &code, &mut send, &mut recv).await;
                (joined, server.await.unwrap())
            }
        };
        // A code whose token the host never issued is refused; nothing is adopted.
        let (joined, hosted) = run([1; 16], [2; 16]).await;
        assert!(joined.is_err() && hosted.is_err());
        assert!(account_pub(&n.config_dir).is_none());
        // The real code links: same account, friends + chats moved, each side
        // lists the other as its own device.
        let (joined, hosted) = run([3; 16], [3; 16]).await;
        joined.unwrap();
        assert_eq!(hosted.unwrap(), new_ep.id().to_string());
        let account = account_pub(&h.config_dir).unwrap();
        assert_eq!(account_pub(&n.config_dir).as_deref(), Some(account.as_str()));
        let mine = friends::load(&n.config_dir);
        let m = mine.iter().find(|x| x.endpoint_id.as_deref() == Some(mong.as_str())).unwrap();
        assert_eq!(chat::messages(&n.config_dir, &m.id).len(), 1);
        assert!(mine.iter().any(|x| x.endpoint_id.as_deref() == Some(host_ep.id().to_string().as_str()) && x.account_pub.as_deref() == Some(account.as_str())));
        let theirs = friends::load(&h.config_dir);
        let phone = theirs.iter().find(|x| x.endpoint_id.as_deref() == Some(new_ep.id().to_string().as_str())).unwrap();
        assert_eq!(phone.account_pub.as_deref(), Some(account.as_str()));
        assert_eq!(phone.device_os.as_deref(), Some("ios"));
        // One-time: the same code can't be replayed.
        let (joined, _) = run_replay(&h, &host_net, &host_ep, &new_ep).await;
        assert!(joined.is_err());
        for d in [&h.config_dir, &n.config_dir] { let _ = std::fs::remove_dir_all(d); }
    }

    async fn run_replay(h: &Arc<AppState>, host_net: &Arc<IrohState>, host_ep: &iroh::Endpoint, new_ep: &iroh::Endpoint) -> (Result<LinkResult, String>, ()) {
        let (h2, net2, hep) = (h.clone(), host_net.clone(), host_ep.clone());
        let server = tokio::spawn(async move {
            let conn = hep.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            let _ = host_join_over(&h2, None, &net2, &who, &req, &mut send, &mut recv).await;
        });
        let n = AppState::for_tests(state().config_dir);
        let conn = new_ep.connect(host_ep.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let me = LinkResult { endpoint_id: new_ep.id().to_string(), name: "Phone".into(), device_kind: "phone".into(), device_os: None, friends: None, messages: None };
        let code = LinkCode { v: 1, eid: host_ep.id().to_string(), name: "Mac".into(), token: hex::encode([3u8; 16]), acct: None, devices: None };
        let r = join_over(&n, None, me, &code, &mut send, &mut recv).await;
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(&n.config_dir);
        (r, ())
    }
}

/// Linking edge cases: wrong codes, direction, refusals, drops, merges.
#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::account::testkit::{sync, text, Dev};

    fn eid() -> String { iroh::SecretKey::generate().public().to_string() }
    fn code(kind: CodeKind, eid: &str, acct: Option<String>, devices: Option<u32>) -> String {
        let c = LinkCode { v: 1, eid: eid.into(), name: "Other".into(), token: hex::encode([9u8; 16]), acct, devices };
        if kind == CodeKind::Link { encode(&c) } else { encode_join(&c) }
    }
    fn net(d: &Dev) -> Arc<IrohState> {
        let n = Arc::new(IrohState::default());
        let _ = n.endpoint.set(d.ep.clone());
        n
    }
    fn me(d: &Dev) -> LinkResult {
        LinkResult { endpoint_id: d.eid(), name: d.label.clone(), device_kind: "phone".into(), device_os: Some("ios".into()), friends: None, messages: None }
    }
    /// Put `d` in an account with one other (made-up) device.
    fn shared(d: &Dev, key: &iroh::SecretKey) -> String {
        adopt_key_for_tests(&d.dir, key);
        let account = hex::encode(key.public().as_bytes());
        friends::upsert_own_device(&d.dir, &eid(), "Other device", Some("laptop"), Some("macos"), &account, 1, true);
        account
    }

    #[tokio::test]
    async fn wrong_codes_get_a_clear_word_before_anything_happens() {
        let d = Dev::new("Mac", "laptop", None).await;
        let me = d.eid();
        assert_eq!(plan_link(&d.dir, &me, "hello there").unwrap_err(), NOT_A_CODE);
        assert_eq!(plan_link(&d.dir, &me, "dropbeamlink1:%%%").unwrap_err(), NOT_A_CODE);
        assert_eq!(plan_link(&d.dir, &me, &friends::my_code("Mong", &eid())).unwrap_err(), FRIEND_CODE);
        assert_eq!(plan_link(&d.dir, &me, "DropBeamF1:abc").unwrap_err(), FRIEND_CODE);
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &me, None, Some(0))).unwrap_err(), SELF_CODE);
        // …and a device code in the friend scanner points to Settings → Devices.
        assert_eq!(friends::add_by_code(&d.dir, &code(CodeKind::Link, &eid(), None, None)).unwrap_err(), DEVICE_CODE_AS_FRIEND);
        assert!(friends::load(&d.dir).is_empty());
        assert!(account_pub(&d.dir).is_none(), "nothing minted");
        // Refusals from the other device (older builds send terse reasons) read as people-words.
        assert_eq!(explain("no pending link"), EXPIRED);
        assert_eq!(explain("link expired"), EXPIRED);
        assert_eq!(explain("already linked to another account"), BOTH_ACCOUNTS);
    }

    #[tokio::test]
    async fn the_account_with_devices_wins_whichever_code_was_scanned() {
        let d = Dev::new("Mac", "laptop", None).await;
        let me = d.eid();
        let (other, mine_key, theirs_key) = (eid(), iroh::SecretKey::generate(), iroh::SecretKey::generate());
        let theirs = Some(hex::encode(theirs_key.public().as_bytes()));
        // Neither has devices: the code decides.
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Link, &other, None, Some(0))).unwrap().1, Direction::Give);
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &other, None, Some(0))).unwrap().1, Direction::Take);
        // They have devices, we don't: we join them even from their "link me" code.
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Link, &other, theirs.clone(), Some(2))).unwrap().1, Direction::Take);
        // We have devices, they don't: they join us even from their "join me" code.
        let mine = shared(&d, &mine_key);
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &other, None, Some(0))).unwrap().1, Direction::Give);
        // Both have devices in different accounts: refused before dialing.
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &other, theirs.clone(), Some(1))).unwrap_err(), BOTH_ACCOUNTS);
        // An older code (no device count) keeps its own direction.
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &other, None, None)).unwrap().1, Direction::Take);
        // Already one of our devices: nothing to do.
        let ours = own_eid(&d);
        assert_eq!(plan_link(&d.dir, &me, &code(CodeKind::Join, &ours, Some(mine.clone()), Some(1))).unwrap_err(), ALREADY);
        assert_eq!(account_pub(&d.dir), Some(mine), "untouched");
    }
    fn own_eid(d: &Dev) -> String {
        crate::account::own_devices(&d.dir)[0].endpoint_id.clone().unwrap()
    }

    /// A "join me" code works for a device that GIVES its account too (the
    /// scanner had the devices), and a "link me" code for one that takes.
    #[tokio::test]
    async fn either_code_accepts_either_direction_once() {
        let (giver, shower) = (Dev::new("iPhone", "phone", None).await, Dev::new("Mac", "laptop", None).await);
        let key = iroh::SecretKey::generate();
        shared(&giver, &key);
        let mong = eid();
        friends::upsert_by_endpoint(&giver.dir, &mong, "Mong");
        // The Mac shows a JOIN code; the iPhone (with devices) pushes its account instead.
        with_host(&shower.dir, |p| *p = Some(PendingLink { token: [5; 16], created_at: Instant::now() }));
        let c = LinkCode { v: 1, eid: shower.eid(), name: "Mac".into(), token: hex::encode([5u8; 16]), acct: None, devices: Some(0) };
        let o = offer(&giver.st, &net(&giver), &c, &key).unwrap();
        receive(&shower.st, me(&shower), &giver.eid(), &o).unwrap();
        assert_eq!(account_pub(&shower.dir), account_pub(&giver.dir));
        assert!(shower.friend(&mong).is_some());
        assert!(shower.friend(&giver.eid()).unwrap().account_pub.is_some(), "the giver is its own device now");
        // One use only, whichever slot it came from.
        assert_eq!(receive(&shower.st, me(&shower), &giver.eid(), &o).unwrap_err(), "no pending link");
    }

    /// The new device already had friends and chats: nothing lost, nothing
    /// doubled, and its own friends reach the account's other devices.
    #[tokio::test]
    async fn linking_a_device_with_its_own_friends_and_chats_merges_them() {
        let (host, newbie) = (Dev::new("Mac", "laptop", None).await, Dev::new("iPhone", "phone", None).await);
        let (mong, ethan) = (eid(), eid());
        let hm = friends::upsert_by_endpoint(&host.dir, &mong, "Mong");
        chat::append(&host.dir, &text("h1", &hm.id, true, 10));
        let mut pending = text("h2", &hm.id, true, 30); pending.status = Some("sending".into());
        chat::append(&host.dir, &pending);
        let nm = friends::upsert_by_endpoint(&newbie.dir, &mong, "Mong (phone)");
        chat::append(&newbie.dir, &text("n1", &nm.id, false, 20));
        chat::append(&newbie.dir, &text("h1", &nm.id, true, 10)); // the same message, already here
        let ne = friends::upsert_by_endpoint(&newbie.dir, &ethan, "Ethan");
        chat::append(&newbie.dir, &text("e1", &ne.id, false, 5));
        let (joined, hosted) = join(&host, &newbie, [3; 16], [3; 16], true).await;
        let info = joined.unwrap();
        assert_eq!((info.friends, info.messages), (Some(1), Some(2)));
        hosted.unwrap();
        // One Mong, one conversation in time order, the pending copy not the phone's to send.
        let mongs: Vec<_> = friends::load(&newbie.dir).into_iter().filter(|f| f.endpoint_id.as_deref() == Some(mong.as_str())).collect();
        assert_eq!(mongs.len(), 1);
        let ids: Vec<String> = newbie.thread(&mong).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["h1", "n1", "h2"]);
        assert!(chat::outbox(&newbie.dir).is_empty());
        assert!(newbie.friend(&ethan).is_some(), "its own friend stays");
        // First sync: the account's other device gets the phone's friend and history.
        assert!(sync(&newbie, &host).await.client.is_ok());
        assert!(host.friend(&ethan).is_some());
        let ids: Vec<String> = host.thread(&mong).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["h1", "n1", "h2"]);
        assert_eq!(host.thread(&ethan).len(), 1);
    }

    /// Both devices already in (different) multi-device accounts, with an older
    /// code that can't say so up front: the new device refuses, nothing moves.
    #[tokio::test]
    async fn two_multi_device_accounts_are_never_merged() {
        let (host, newbie) = (Dev::new("Mac", "laptop", None).await, Dev::new("iPhone", "phone", None).await);
        let (hk, nk) = (iroh::SecretKey::generate(), iroh::SecretKey::generate());
        shared(&host, &hk);
        shared(&newbie, &nk);
        let before = friends::load(&newbie.dir).len();
        let (joined, hosted) = join(&host, &newbie, [4; 16], [4; 16], true).await;
        assert_eq!(joined.unwrap_err(), BOTH_ACCOUNTS);
        assert!(hosted.is_err());
        assert_eq!(account_pub(&newbie.dir), Some(hex::encode(nk.public().as_bytes())));
        assert_eq!(friends::load(&newbie.dir).len(), before);
        assert!(host.friend(&newbie.eid()).is_none(), "the host recorded nothing");
    }

    /// The other device vanishes mid-link: a clean error, no half-linked state,
    /// and a fresh code links fine afterwards.
    #[tokio::test]
    async fn a_link_cut_short_leaves_nothing_behind_and_retry_works() {
        let (host, newbie) = (Dev::new("Mac", "laptop", None).await, Dev::new("iPhone", "phone", None).await);
        // The host goes away right after the join request (no offer ever comes).
        let srv = host.ep.clone();
        let server = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let (_send, mut recv) = conn.accept_bi().await.unwrap();
            let _ = iroh_net::read_frame(&mut recv).await;
            conn.close(0u32.into(), b"quit");
        });
        let conn = newbie.ep.connect(host.ep.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let c = LinkCode { v: 1, eid: host.eid(), name: "Mac".into(), token: hex::encode([6u8; 16]), acct: None, devices: Some(0) };
        let err = join_over(&newbie.st, None, me(&newbie), &c, &mut send, &mut recv).await.unwrap_err();
        server.await.unwrap();
        assert!(err == DROPPED || err == UNREACHABLE, "{err}");
        assert!(account_pub(&newbie.dir).is_none() && friends::load(&newbie.dir).is_empty());
        // The new device goes away after the offer: the host records nothing.
        let srv_st = host.st.clone();
        let host_net = net(&host);
        with_host(&host.dir, |p| *p = Some(PendingLink { token: [7; 16], created_at: Instant::now() }));
        let srv = host.ep.clone();
        let server = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            host_join_over(&srv_st, None, &host_net, &who, &req, &mut send, &mut recv).await
        });
        let conn = newbie.ep.connect(host.ep.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        iroh_net::write_frame(&mut send, &json!({"kind":"link-join", "v":1, "token":hex::encode([7u8; 16]), "device":me(&newbie)})).await.unwrap();
        let _offer = iroh_net::read_frame_cap(&mut recv, MAX_OFFER).await.unwrap();
        conn.close(0u32.into(), b"quit");
        assert!(!matches!(server.await.unwrap(), Ok(Ok(_))));
        assert!(host.friend(&newbie.eid()).is_none());
        assert!(account_pub(&newbie.dir).is_none());
        // Retry with a fresh code: linked both ways.
        let (joined, hosted) = join(&host, &newbie, [8; 16], [8; 16], true).await;
        joined.unwrap();
        hosted.unwrap();
        assert_eq!(account_pub(&newbie.dir), account_pub(&host.dir));
        assert!(host.friend(&newbie.eid()).unwrap().account_pub.is_some());
    }

    /// One join over loopback: the host shows a code (`host_slot`: a "join me"
    /// code, else a "link me" one) with `token`; the new device scans `shown`.
    async fn join(host: &Dev, newbie: &Dev, token: [u8; 16], shown: [u8; 16], host_slot: bool)
        -> (Result<LinkResult, String>, Result<String, String>) {
        let pending = PendingLink { token, created_at: Instant::now() };
        if host_slot { with_host(&host.dir, |p| *p = Some(pending)); } else { *host.st.pending_link.lock().unwrap() = Some(pending); }
        let (srv, st, host_net) = (host.ep.clone(), host.st.clone(), net(host));
        let server = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            let out = host_join_over(&st, None, &host_net, &who, &req, &mut send, &mut recv).await.map_err(|e| e.to_string())?;
            let (key, result) = out?;
            record_new_device_data(&st, &host_net, &key, &result);
            Ok(result.endpoint_id)
        });
        let conn = newbie.ep.connect(host.ep.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let c = LinkCode { v: 1, eid: host.eid(), name: host.label.clone(), token: hex::encode(shown), acct: None, devices: None };
        let joined = join_over(&newbie.st, None, me(newbie), &c, &mut send, &mut recv).await;
        (joined, server.await.unwrap())
    }

    /// A "link me" code on a device that already has the account: the scanner
    /// joins it instead (its pending "link me" token answers a join request).
    #[tokio::test]
    async fn a_link_me_code_can_be_joined() {
        let (host, newbie) = (Dev::new("Mac", "laptop", None).await, Dev::new("iPhone", "phone", None).await);
        shared(&host, &iroh::SecretKey::generate());
        let (joined, hosted) = join(&host, &newbie, [2; 16], [2; 16], false).await;
        joined.unwrap();
        hosted.unwrap();
        assert_eq!(account_pub(&newbie.dir), account_pub(&host.dir));
        // The account's other device came over as an own device, not a contact.
        assert_eq!(crate::account::own_devices(&newbie.dir).len(), 2);
    }
}
