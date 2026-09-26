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
    let Some(d) = device(config) else { return };
    let Some(ep) = net.get().cloned() else { return };
    let me = ep.id().to_string();
    let t = now();
    for s in super::client::servers(config).into_iter().filter(|s| s.usable() && (s.own || s.hold_for_me)) {
        if d.registered.get(&s.eid).is_some_and(|at| t.saturating_sub(*at) < REREGISTER_MS) {
            continue;
        }
        let Ok(sealed) = seal_token(&d, &me, &s.eid, WORKER_SEAL_PUB) else { continue };
        let ok = super::client::push_register(&ep, &s.eid, &sealed).await;
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

fn seal_for(config: &Path, to: &[seal::Recipient], body: impl Fn(bool) -> String) -> Value {
    let peers = super::keys::peers(config);
    let title = my_name(config);
    let mut out = serde_json::Map::new();
    for r in to {
        let Some(p) = peers.get(&r.eid) else { continue };
        let Some(pk) = p.push_key.as_deref().and_then(|k| seal::key32(k).ok()) else { continue };
        let pt = json!({"t": title, "b": body(p.push_text), "th": ""}).to_string();
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
    seal_for(config, to, |show_text| match kind {
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
    seal_for(config, to, |show| if show && names.len() == 1 && !first.is_empty() { format!("Sent you {first}") } else { body.clone() })
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
    let rec = json!({"sealed_token": token, "at": now()});
    match crate::settings::write_atomic(&path, rec.to_string().as_bytes()) {
        Ok(()) => json!({"ok": true}),
        Err(_) => json!({"ok": false, "reason": "storage"}),
    }
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

/// Something new is held: wake each addressed phone that registered here and
/// isn't connected right now.
pub fn on_stored(config: &Path, item: &Item) {
    if item.kind == "op" {
        return;
    }
    let Some(root) = root_of(config) else { return };
    let Some(signer) = identity(config) else { return };
    let c = super::server::load_config(config);
    let url = if c.push_url.trim().is_empty() { DEFAULT_URL.to_owned() } else { c.push_url.trim().to_owned() };
    // Same sender → one notification thread on the phone.
    let collapse = {
        use sha2::{Digest, Sha256};
        hex::encode(&Sha256::digest(item.from.as_bytes())[..8])
    };
    for to in &item.to {
        if super::server::recently_seen_device(to) {
            continue;
        }
        let path = reg_dir(&root).join(format!("{to}.json"));
        let Some(token) = std::fs::read(&path).ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["sealed_token"].as_str().map(String::from)) else { continue };
        let payload = item.push.get(to).cloned().unwrap_or_default();
        let body = signed_request(&signer, &token, &collapse, &payload);
        let url = url.clone();
        tauri::async_runtime::spawn(async move {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build();
            let Ok(client) = client else { return };
            match client.post(&url).json(&body).send().await {
                Ok(res) => {
                    let v: Value = res.json().await.unwrap_or(Value::Null);
                    if v["gone"].as_bool() == Some(true) {
                        let _ = std::fs::remove_file(&path);
                        log::info!("push: a phone's registration expired; removed it");
                    } else if v["ok"].as_bool() != Some(true) {
                        log::info!("push: relay didn't send ({})", v["reason"].as_str().unwrap_or("?").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(24).collect::<String>());
                    }
                }
                Err(e) => log::info!("push: relay unreachable: {}", e.without_url()),
            }
        });
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
