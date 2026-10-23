//! Waking an iPhone so a call can ring while DropBeam is in the background or
//! not running: an Apple VoIP push (PushKit), through the owner's push relay
//! (push-worker, route `/call`). Only a wake-up travels that way — the call
//! itself (signaling and media) always goes over the iroh connection.
//!
//! - The PHONE gets a VoIP token from PushKit (`push_set_voip_token`). In each
//!   hello to a friend or own device it includes that token SEALED to the relay's
//!   key and bound to that one device (`allowed_server` = the device we greet).
//!   Nobody but the relay can read the token, and only that device can use it.
//! - The CALLER, when dialing a device fails, signs a `/call` request with its
//!   endpoint key: sealed token + call id + a payload whose caller id is sealed
//!   to the phone's notification key when known. The relay checks the signature
//!   and the binding, rate-limits per phone, and asks APNs for a VoIP push.
//! - The phone reports the call to CallKit at once (iOS requires it), wakes the
//!   engine and gets dialed again (calls/mod.rs redials every 2 s while it rings).
//!   A caller who isn't a friend is refused by the engine as usual, and the
//!   CallKit call ends.
//! Without a configured relay (or VoIP token) calls ring only while the app runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::iroh_net::IrohState;
use crate::mailbox::{push, seal};

/// The relay's call route (beside push::DEFAULT_URL's /push).
pub const CALL_URL: &str = "https://dropbeam-push.ashton-mcp-worker.workers.dev/call";
const BUNDLE: &str = "com.ashtonmiller.dropbeam";
const TOKEN_TTL_MS: u64 = 90 * 24 * 3600 * 1000;
const MAX_SEALED: usize = 2048;

static LOCK: Mutex<()> = Mutex::new(());

fn now() -> u64 { crate::chat::now_ms() }

// ── phone side ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Voip {
    /// PushKit VoIP token, hex.
    pub token: String,
    /// "prod" | "sandbox".
    pub env: String,
    #[serde(default)]
    pub updated_ms: u64,
}

fn voip_path(config: &Path) -> PathBuf { config.join("push-voip.json") }

pub fn my_voip(config: &Path) -> Option<Voip> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let v: Voip = serde_json::from_slice(&std::fs::read(voip_path(config)).ok()?).ok()?;
    (!v.token.is_empty()).then_some(v)
}

/// PushKit gave (or refreshed, or with an empty token withdrew) this phone's
/// VoIP token. True when it changed (friends then need a fresh hello).
pub fn set_voip_token(config: &Path, token: &str, env: &str) -> Result<bool> {
    let token = token.trim().to_ascii_lowercase();
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if token.is_empty() {
        return Ok(std::fs::remove_file(voip_path(config)).is_ok());
    }
    anyhow::ensure!(token.len() >= 32 && token.len() <= 200 && token.chars().all(|c| c.is_ascii_hexdigit()), "bad VoIP token");
    let env = if env == "sandbox" { "sandbox" } else { "prod" };
    let old: Option<Voip> = std::fs::read(voip_path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let changed = old.as_ref().is_none_or(|o| o.token != token || o.env != env);
    let v = Voip { token, env: env.into(), updated_ms: now() };
    crate::settings::write_atomic(&voip_path(config), &serde_json::to_vec(&v)?)?;
    Ok(changed)
}

/// Seal `body` for the relay: `eph(32) || nonce(12) || AES-256-GCM`, key =
/// HKDF-SHA256(X25519(eph, W), eph || W, "dropbeam-push-token-v1") — the same
/// format as push::seal_token, so the relay opens both the same way.
fn seal_for_worker(body: &Value, worker_pub: &str) -> Result<String> {
    let w = seal::key32(worker_pub)?;
    let eph: [u8; 32] = rand::random();
    let epk = seal::x25519_public(&eph);
    let ss = seal::x25519(&eph, &w).context("bad worker key")?;
    let mut salt = epk.to_vec();
    salt.extend_from_slice(&w);
    let k = seal::hkdf32_pub(&ss, &salt, b"dropbeam-push-token-v1");
    let nonce: [u8; 12] = rand::random();
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).map_err(|_| anyhow::anyhow!("aes key"))?);
    let mut buf = serde_json::to_vec(body)?;
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| anyhow::anyhow!("seal failed"))?;
    let mut out = epk.to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(seal::b64(&out))
}

/// Our VoIP token sealed for the relay, usable only by `to`.
pub fn seal_voip(v: &Voip, me: &str, to: &str, worker_pub: &str) -> Result<String> {
    seal_for_worker(&json!({
        "kind": "voip", "token": v.token, "env": v.env, "bundle": BUNDLE,
        "allowed_server": to, "device": me, "exp": now() + TOKEN_TTL_MS,
    }), worker_pub)
}

/// The hello field for `to` (a friend or own device), if this phone has a VoIP token.
pub fn hello_field(config: &Path, signer: &iroh::SecretKey, to: &str) -> Option<String> {
    let trusted = crate::account::is_own_device(config, to) || crate::friends::chat_sender(config, to).is_some();
    if !trusted { return None; }
    let v = my_voip(config)?;
    seal_voip(&v, &signer.public().to_string(), to, push::WORKER_SEAL_PUB).ok()
}

// ── caller side ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Sealed { sealed: String, at: u64 }

fn peers_path(config: &Path) -> PathBuf { config.join("voip-peers.json") }

fn load_peers(config: &Path) -> HashMap<String, Sealed> {
    std::fs::read(peers_path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// A trusted peer's hello (`mailbox.voip`): remember how to wake it. Called
/// from mailbox::on_hello after its friend/own-device check; `None` forgets.
pub fn learn(config: &Path, who: &str, mailbox: &Value) {
    let sealed = mailbox.get("voip").and_then(|v| v.as_str())
        .filter(|s| s.len() <= MAX_SEALED && seal::unb64(s).is_ok_and(|b| b.len() > 44 + 16));
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut peers = load_peers(config);
    let changed = match sealed {
        Some(s) => peers.insert(who.to_string(), Sealed { sealed: s.to_string(), at: now() }).is_none_or(|old| old.sealed != s),
        None => peers.remove(who).is_some(),
    };
    if changed {
        if let Ok(bytes) = serde_json::to_vec(&peers) { let _ = crate::settings::write_atomic(&peers_path(config), &bytes); }
    }
}

pub fn sealed_for(config: &Path, endpoint: &str) -> Option<String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load_peers(config).get(endpoint).map(|s| s.sealed.clone())
}

/// Canonical bytes the relay verifies for `/call` (push-worker/worker.js).
pub fn call_message(caller: &str, sealed: &str, call_id: &str, payload: &str, ts: u64) -> Vec<u8> {
    format!("dropbeam-call-v1\n1\n{caller}\n{sealed}\n{call_id}\n{payload}\n{ts}").into_bytes()
}

pub fn signed_call(signer: &iroh::SecretKey, sealed: &str, call_id: &str, payload: &str) -> Value {
    let caller = signer.public().to_string();
    let ts = now();
    let sig = signer.sign(&call_message(&caller, sealed, call_id, payload, ts));
    json!({"v": 1, "caller": caller, "sealed_token": sealed, "call": call_id, "payload": payload, "ts": ts,
        "sig": seal::b64(&sig.to_bytes())})
}

/// What APNs and the relay see: the call id, audio/video and — sealed to the
/// phone's notification key when we know it — who is calling.
pub fn wake_payload(me: &str, call_id: &str, video: bool, push_key: Option<&[u8; 32]>) -> String {
    let who = json!({"f": me}).to_string();
    let mut p = json!({"c": call_id, "v": video});
    match push_key.and_then(|k| seal::seal_small(k, who.as_bytes()).ok()) {
        Some(s) => p["s"] = json!(s),
        None => p["f"] = json!(me),
    }
    p.to_string()
}

/// Ask `endpoint` to wake up for call `call_id`. No push path → false.
pub async fn wake(state: &IrohState, endpoint: &str, call_id: &str, video: bool) -> bool {
    let (Ok(config), Some(ep)) = (crate::iroh_net::location_config(state), state.get().cloned()) else { return false };
    let Some(sealed) = sealed_for(&config, endpoint) else { return false };
    let push_key = crate::mailbox::keys::peers(&config).get(endpoint)
        .and_then(|p| p.push_key.clone()).and_then(|k| seal::key32(&k).ok());
    let payload = wake_payload(&ep.id().to_string(), call_id, video, push_key.as_ref());
    let body = signed_call(ep.secret_key(), &sealed, call_id, &payload);
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(8)).build() else { return false };
    match client.post(CALL_URL).json(&body).send().await {
        Ok(r) if r.status().is_success() => { log::info!("call: woke a sleeping device with a VoIP push"); true }
        Ok(r) => {
            if r.status().as_u16() == 410 {
                // That phone's token is gone: stop trying it until its next hello.
                learn(&config, endpoint, &Value::Null);
            }
            log::info!("call: VoIP wake-up refused ({})", r.status());
            false
        }
        Err(e) => { log::info!("call: VoIP wake-up failed: {e}"); false }
    }
}

/// The iOS shell hands over its PushKit VoIP token (empty = withdrawn).
#[tauri::command]
pub fn push_set_voip_token(app: AppHandle, state: State<'_, Arc<crate::AppState>>, net: State<'_, Arc<IrohState>>, token: String, env: String) -> Result<(), String> {
    if set_voip_token(&state.config_dir, &token, &env).map_err(|e| e.to_string())? {
        // Friends and own devices learn how to wake us on the next hello.
        crate::iroh_net::broadcast_profile(app, net.inner().clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dropbeam-voip-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Open a token the way push-worker/worker.js openToken does.
    fn open(worker_secret: &[u8; 32], sealed: &str) -> Value {
        let raw = seal::unb64(sealed).unwrap();
        let eph = <[u8; 32]>::try_from(&raw[..32]).unwrap();
        let ss = seal::x25519(worker_secret, &eph).unwrap();
        let mut salt = eph.to_vec();
        salt.extend_from_slice(&seal::x25519_public(worker_secret));
        let k = seal::hkdf32_pub(&ss, &salt, b"dropbeam-push-token-v1");
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).unwrap());
        let mut buf = raw[44..].to_vec();
        let pt = key.open_in_place(Nonce::assume_unique_for_key(raw[32..44].try_into().unwrap()), Aad::empty(), &mut buf).unwrap();
        serde_json::from_slice(pt).unwrap()
    }

    #[test]
    fn voip_token_is_validated_stored_and_withdrawn() {
        let d = dir();
        assert!(set_voip_token(&d, "not hex", "prod").is_err());
        assert!(set_voip_token(&d, &"ab".repeat(32), "sandbox").unwrap());
        assert!(!set_voip_token(&d, &"AB".repeat(32), "sandbox").unwrap(), "same token, any case: no change");
        assert_eq!(my_voip(&d).unwrap().env, "sandbox");
        assert!(set_voip_token(&d, &"ab".repeat(32), "weird").unwrap(), "env change counts");
        assert_eq!(my_voip(&d).unwrap().env, "prod");
        assert!(set_voip_token(&d, "", "prod").unwrap());
        assert!(my_voip(&d).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn sealed_token_is_bound_to_one_device_and_opens_like_the_relay() {
        let worker: [u8; 32] = rand::random();
        let wpub = seal::b64(&seal::x25519_public(&worker));
        let v = Voip { token: "cd".repeat(32), env: "prod".into(), updated_ms: 0 };
        let s = seal_voip(&v, "phone", "caller-device", &wpub).unwrap();
        let t = open(&worker, &s);
        assert_eq!(t["kind"], "voip");
        assert_eq!(t["allowed_server"], "caller-device");
        assert_eq!(t["token"], "cd".repeat(32));
        assert!(t["exp"].as_u64().unwrap() > now());
        assert!(!s.contains(&v.token));
    }

    #[test]
    fn hello_field_only_for_friends_and_learning_is_bounded() {
        let d = dir();
        let signer = iroh::SecretKey::generate();
        set_voip_token(&d, &"ab".repeat(32), "prod").unwrap();
        let stranger = iroh::SecretKey::generate().public().to_string();
        assert!(hello_field(&d, &signer, &stranger).is_none());
        let friend = iroh::SecretKey::generate().public().to_string();
        crate::friends::upsert_by_endpoint(&d, &friend, "Bea");
        let f = hello_field(&d, &signer, &friend).expect("friends get it");
        // The caller side remembers it, forgets on an empty hello, ignores junk.
        learn(&d, &friend, &json!({"voip": f}));
        assert_eq!(sealed_for(&d, &friend).as_deref(), Some(f.as_str()));
        learn(&d, &friend, &json!({"voip": "x".repeat(5000)}));
        assert!(sealed_for(&d, &friend).is_none());
        learn(&d, &friend, &json!({"voip": f}));
        learn(&d, &friend, &json!({}));
        assert!(sealed_for(&d, &friend).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn call_request_signature_and_payload() {
        let k = iroh::SecretKey::generate();
        let body = signed_call(&k, "SEALED", "call-1", "{}");
        let sig = seal::unb64(body["sig"].as_str().unwrap()).unwrap();
        let sig = iroh::Signature::from_bytes(&<[u8; 64]>::try_from(sig.as_slice()).unwrap());
        let msg = call_message(body["caller"].as_str().unwrap(), "SEALED", "call-1", "{}", body["ts"].as_u64().unwrap());
        assert!(k.public().verify(&msg, &sig).is_ok());
        // Not valid as a /push request (different domain string).
        let push_msg = push::request_message(body["caller"].as_str().unwrap(), "SEALED", "call-1", "{}", body["ts"].as_u64().unwrap());
        assert!(k.public().verify(&push_msg, &sig).is_err());
        // Caller id sealed when the phone's key is known, plain otherwise.
        let phone: [u8; 32] = rand::random();
        let p: Value = serde_json::from_str(&wake_payload("me", "c1", true, Some(&seal::x25519_public(&phone)))).unwrap();
        assert!(p.get("f").is_none());
        let opened: Value = serde_json::from_slice(&seal::open_small(&phone, p["s"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(opened["f"], "me");
        let p: Value = serde_json::from_str(&wake_payload("me", "c1", false, None)).unwrap();
        assert_eq!((p["f"].as_str(), p["v"].as_bool(), p["c"].as_str()), (Some("me"), Some(false), Some("c1")));
    }
}
