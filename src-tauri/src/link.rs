//! Serverless account bootstrap over the authenticated iroh connection.
use std::{collections::HashMap, path::Path, sync::{Arc, Mutex}, time::{Duration, Instant}};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use crate::{chat::{self, ChatMessage}, friends, iroh_net::{self, IrohState}, AppState};

const PREFIX: &str = "dropbeamlink1:";
const TTL: Duration = Duration::from_secs(600);
pub(crate) const MAX_OFFER: usize = 64 << 20;
const MAX_AVATAR: usize = 2 << 20;
static ACCOUNT_LOCK: Mutex<()> = Mutex::new(());

pub struct PendingLink { token: [u8; 16], created_at: Instant }
pub(crate) fn pending_active(st: &AppState) -> bool {
    let mut pending = st.pending_link.lock().unwrap();
    if pending.as_ref().is_some_and(|p| p.created_at.elapsed() >= TTL) { *pending = None; }
    pending.is_some()
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

#[derive(Serialize, Deserialize)]
struct LinkCode { v: u8, eid: String, name: String, token: String }
fn encode(code: &LinkCode) -> String { format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(code).unwrap())) }
fn parse(code: &str) -> Result<LinkCode, String> {
    if code.len() > 8192 { return Err("invalid link code".into()); }
    let payload = crate::codes::strip_prefix(code, PREFIX).ok_or("invalid link prefix")?;
    let c: LinkCode = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| "invalid link code")?).map_err(|_| "invalid link code")?;
    if c.v != 1 || hex::decode(&c.token).map_or(true, |v| v.len() != 16) || c.eid.parse::<iroh::EndpointId>().is_err() { return Err("invalid link code".into()); }
    Ok(c)
}
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
    json!({"device_kind": st.settings.lock().unwrap().device_kind,
        "account_pub": key.as_ref().map(|k| hex::encode(k.public().as_bytes())),
        "account_sig": key.map(|k| hex::encode(k.sign(endpoint.as_bytes()).to_bytes()))})
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LinkResult { endpoint_id: String, name: String, device_kind: String }
#[derive(Serialize)]
pub struct DeviceInfo { #[serde(flatten)] device: LinkResult, account_pub: String, linked_devices: usize }
fn device(st: &AppState, net: &IrohState) -> Result<LinkResult, String> {
    let endpoint_id = net.get().ok_or("network not ready")?.id().to_string();
    let s = st.settings.lock().unwrap();
    Ok(LinkResult { endpoint_id, name: s.display_name.clone(), device_kind: s.device_kind.clone() })
}
#[tauri::command]
pub fn my_device_info(state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<DeviceInfo, String> {
    let device = device(&state, &iroh)?;
    // Never mint a key just to answer a query: a fresh device must stay unlinked
    // until it either links a new device (link_device_send) or receives an offer,
    // otherwise its own throwaway key would block adopting the real account.
    let account_pub = { let _guard = ACCOUNT_LOCK.lock().unwrap(); read_key(&state.config_dir)?.map(|k| hex::encode(k.public().as_bytes())).unwrap_or_default() };
    let linked_devices = if account_pub.is_empty() { 0 } else { friends::load(&state.config_dir).iter().filter(|f| f.account_pub.as_deref() == Some(&account_pub)).count() };
    Ok(DeviceInfo { device, account_pub, linked_devices })
}
#[tauri::command]
pub fn link_device_begin(state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<String, String> {
    let me = device(&state, &iroh)?;
    let token: [u8; 16] = rand::random();
    *state.pending_link.lock().unwrap() = Some(PendingLink { token, created_at: Instant::now() });
    Ok(encode(&LinkCode { v: 1, eid: me.endpoint_id, name: me.name, token: hex::encode(token) }))
}
#[tauri::command]
pub fn link_device_cancel(state: State<'_, Arc<AppState>>) { *state.pending_link.lock().unwrap() = None; }

fn export_thread(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    messages.sort_by_key(|m| (m.seq, m.ts, m.id.clone()));
    if messages.len() > 500 { messages.drain(..messages.len() - 500); }
    for m in &mut messages { m.path = None; }
    messages
}
fn offer(st: &AppState, net: &IrohState, code: &LinkCode, key: &iroh::SecretKey) -> Result<Value, String> {
    let mut chats = HashMap::new();
    let records: Vec<Value> = friends::load(&st.config_dir).into_iter().map(|mut f| {
        chats.insert(f.id.clone(), export_thread(chat::messages(&st.config_dir, &f.id)));
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
#[tauri::command]
pub async fn link_device_send(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>, code: String) -> Result<LinkResult, String> {
    let code = parse(&code)?;
    let key = account(&state.config_dir)?;
    let req = offer(&state, &iroh, &code, &key)?;
    if serde_json::to_vec(&req).map_err(|_| "cannot encode link")?.len() > MAX_OFFER { return Err("link history exceeds 64 MiB".into()); }
    let ep = iroh.get().ok_or("network not ready")?.clone();
    if code.eid == ep.id().to_string() { return Err("cannot link this device to itself".into()); }
    let reply = tokio::time::timeout(Duration::from_secs(20), async {
        let conn = ep.connect(iroh_net::dial_addr(code.eid.parse()?), iroh_net::ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        iroh_net::write_frame(&mut send, &req).await?;
        send.finish()?;
        iroh_net::read_frame(&mut recv).await
    }).await.map_err(|_| "link timed out")?.map_err(|_| "link connection failed")?;
    if reply["kind"] != "link-ok" { return Err(reply["reason"].as_str().unwrap_or("link rejected").to_owned()); }
    let result: LinkResult = serde_json::from_value(reply.clone()).map_err(|_| "invalid link reply")?;
    if result.endpoint_id != code.eid || !verify_account(&hex::encode(key.public().as_bytes()), reply["account_sig"].as_str().unwrap_or(""), &result.endpoint_id) { return Err("invalid link identity".into()); }
    friends::upsert_by_endpoint(&state.config_dir, &result.endpoint_id, &result.name);
    friends::set_device_info(&state.config_dir, &result.endpoint_id, Some(&result.device_kind), Some(&hex::encode(key.public().as_bytes())));
    iroh_net::broadcast_profile(app.clone(), iroh.inner().clone());
    let _ = app.emit("friends://changed", ());
    Ok(result)
}

fn receive(st: &AppState, me: LinkResult, who: &str, req: &Value) -> Result<Value, String> {
    consume(&mut st.pending_link.lock().unwrap(), req["token"].as_str().unwrap_or(""))?;
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
    let _guard = ACCOUNT_LOCK.lock().unwrap();
    // Refuse only when this device already belongs to an account that other
    // devices share; a key nobody else references is simply replaced.
    if let Some(old) = read_key(&st.config_dir)? {
        if old.to_bytes() != seed {
            let old_pub = hex::encode(old.public().as_bytes());
            if friends::load(&st.config_dir).iter().any(|f| f.account_pub.as_deref() == Some(&old_pub)) { return Err("already linked to another account".into()); }
        }
    }
    write_key(&st.config_dir, &key)?;
    for (f, avatar) in validated {
        let eid = f.endpoint_id.as_deref();
        if eid.is_some_and(|id| id == me.endpoint_id || id == who) { continue; }
        let local = friends::import_link_friend(&st.config_dir, &f);
        // An imported claim has no signature. Preserve any locally verified claim.
        if let Some(eid) = eid { friends::set_device_info(&st.config_dir, eid, f.device_kind.as_deref(), None); }
        if let Some(bytes) = avatar {
            let avatar_id = eid.map(str::to_owned).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let path = st.config_dir.join(format!("friend-avatar-{avatar_id}.jpg"));
            std::fs::write(&path, bytes).map_err(|_| "cannot save friend avatar")?;
            friends::set_link_avatar(&st.config_dir, &local.id, path.to_string_lossy().into_owned());
        }
        if let Some(messages) = chats.get(&f.id) {
            chat::import_link_thread(&st.config_dir, &local.id, export_thread(messages.clone()));
        }
    }
    friends::upsert_by_endpoint(&st.config_dir, who, &sender.name);
    friends::set_device_info(&st.config_dir, who, Some(&sender.device_kind), Some(&public));
    Ok(json!({"kind":"link-ok", "endpoint_id":me.endpoint_id, "name":me.name, "device_kind":me.device_kind,
        "account_sig":hex::encode(key.sign(me.endpoint_id.as_bytes()).to_bytes())}))
}
pub(crate) async fn serve(net: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream) -> anyhow::Result<()> {
    let app = net.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?;
    let st = app.state::<Arc<AppState>>();
    let result = device(&st, net).and_then(|me| receive(&st, me, who, req));
    let reply = result.as_ref().cloned().unwrap_or_else(|e| json!({"kind":"link-error", "reason":e}));
    let written = iroh_net::write_frame(send, &reply).await;
    if result.is_ok() {
        let _ = app.emit("friends://changed", ());
        let _ = app.emit("chat://changed", ());
        if let Some(net) = app.try_state::<Arc<IrohState>>() { iroh_net::broadcast_profile(app.clone(), net.inner().clone()); }
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
        let c = LinkCode { v: 1, eid: iroh::SecretKey::generate().public().to_string(), name: "Ashton's iPhone 📱".into(), token: hex::encode([3;16]) };
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
    fn me() -> LinkResult { LinkResult { endpoint_id: iroh::SecretKey::generate().public().to_string(), name: "Phone".into(), device_kind: "phone".into() } }
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
        f["id"] = json!("offered-thread"); f["accountPub"] = req["account_pub"].clone();
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
}
