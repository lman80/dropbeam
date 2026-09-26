//! Mailbox keys (this device's X25519 key) and the cache of what peers told us
//! in their hellos: their signed mailbox key and the Transfer Servers that hold
//! messages for them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::seal;

static LOCK: Mutex<()> = Mutex::new(());

fn key_path(config: &Path) -> PathBuf {
    config.join("mailbox-key.key")
}

/// This device's mailbox secret, created on first use (0600, written atomically
/// so a crash can never leave a torn key that would strand undelivered items).
/// None when the key exists but can't be read right now (e.g. a locked
/// device): never replace a key on a transient error — every item sealed to
/// the old one would become unopenable.
pub fn secret(config: &Path) -> Option<[u8; 32]> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    match std::fs::read(key_path(config)) {
        Ok(bytes) => match <[u8; 32]>::try_from(bytes.as_slice()) {
            Ok(k) => return Some(k),
            Err(_) => log::warn!("mailbox: key file is malformed; creating a new mailbox key"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!("mailbox: can't read the mailbox key right now: {e}");
            return None;
        }
    }
    let k: [u8; 32] = rand::random();
    if let Err(e) = write_private_atomic(&key_path(config), &k) {
        log::warn!("mailbox: cannot persist the mailbox key: {e}");
        return None;
    }
    Some(k)
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".mailbox-key-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

pub fn public(config: &Path) -> Option<[u8; 32]> {
    secret(config).map(|k| seal::x25519_public(&k))
}

/// A server a peer told us holds its messages ("deposit for me here").
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ServerRef {
    pub eid: String,
    pub name: String,
}

/// What we know about one peer endpoint's mailbox.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PeerInfo {
    /// Their X25519 mailbox public key (base64), accepted only with a valid
    /// signature from their endpoint key.
    #[serde(default)]
    pub key: String,
    /// Their notification preview key (iOS; base64), same signature rule.
    #[serde(default)]
    pub push_key: Option<String>,
    #[serde(default)]
    pub inbox: Vec<ServerRef>,
    /// Servers this peer sends through (so their items may reach us from there).
    #[serde(default)]
    pub sends: Vec<ServerRef>,
    #[serde(default)]
    pub updated_ms: u64,
}

fn peers_path(config: &Path) -> PathBuf {
    config.join("mailbox-peers.json")
}

pub fn peers(config: &Path) -> HashMap<String, PeerInfo> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_peers(config)
}

fn read_peers(config: &Path) -> HashMap<String, PeerInfo> {
    match crate::settings::read_json_store(&peers_path(config)) {
        crate::settings::StoreRead::Loaded(p) => p,
        _ => HashMap::new(),
    }
}

fn write_peers(config: &Path, all: &HashMap<String, PeerInfo>) {
    if let Ok(bytes) = serde_json::to_vec(all) {
        if let Err(e) = crate::settings::write_atomic(&peers_path(config), &bytes) {
            log::warn!("mailbox: cannot save peer keys: {e}");
        }
    }
}

/// Record the mailbox part of an authenticated hello from `eid`. Returns true
/// when anything changed. Keys that don't verify against `eid` are ignored.
pub fn learn(config: &Path, eid: &str, m: &serde_json::Value) -> bool {
    let key = m["key"].as_str().unwrap_or("");
    let sig = m["sig"].as_str().unwrap_or("");
    let verified = seal::key32(key).ok().filter(|k| seal::verify_mailbox_key(eid, k, sig));
    let push = m["push_key"].as_str().and_then(|pk| {
        let k = seal::key32(pk).ok()?;
        seal::verify_mailbox_key(eid, &k, m["push_sig"].as_str()?).then(|| pk.to_owned())
    });
    let refs = |v: &serde_json::Value| -> Vec<ServerRef> { v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let e = s["eid"].as_str()?;
                    e.parse::<iroh::PublicKey>().ok()?;
                    let name: String = s["name"].as_str().unwrap_or("").chars().take(64).collect();
                    Some(ServerRef { eid: e.to_owned(), name })
                })
                .take(8)
                .collect()
        })
        .unwrap_or_default() };
    let inbox = refs(&m["inbox"]);
    let sends = refs(&m["sends"]);
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut all = read_peers(config);
    let entry = all.entry(eid.to_owned()).or_default();
    let before = (entry.key.clone(), entry.push_key.clone(), entry.inbox.clone(), entry.sends.clone());
    if let Some(k) = verified {
        let k = seal::b64(&k);
        if !entry.key.is_empty() && entry.key != k {
            log::info!("mailbox: a peer rotated its mailbox key");
        }
        entry.key = k;
    }
    if m.get("push_key").is_some() {
        entry.push_key = push;
    }
    entry.inbox = inbox;
    entry.sends = sends;
    let changed = before != (entry.key.clone(), entry.push_key.clone(), entry.inbox.clone(), entry.sends.clone());
    let now = crate::chat::now_ms();
    // Hellos repeat often; only touch the disk when something changed (or daily,
    // to keep the "last heard" stamp roughly current).
    if changed || now.saturating_sub(entry.updated_ms) > 24 * 3600 * 1000 {
        entry.updated_ms = now;
        write_peers(config, &all);
    }
    changed
}

/// Recipient keys for the given endpoints (those we hold a verified key for).
pub fn recipients(config: &Path, eids: &[String]) -> Vec<seal::Recipient> {
    let all = peers(config);
    eids.iter()
        .filter_map(|e| {
            let k = seal::key32(&all.get(e)?.key).ok()?;
            Some(seal::Recipient { eid: e.clone(), key: k })
        })
        .collect()
}

/// Inbox servers advertised by any of these endpoints (one person's devices),
/// de-duplicated, in first-seen order.
pub fn inbox_servers(config: &Path, eids: &[String]) -> Vec<ServerRef> {
    let all = peers(config);
    let mut out: Vec<ServerRef> = Vec::new();
    for e in eids {
        for s in all.get(e).map(|p| p.inbox.clone()).unwrap_or_default() {
            if !out.iter().any(|o| o.eid == s.eid) {
                out.push(s);
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn set_peer_for_tests(config: &Path, eid: &str, info: PeerInfo) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut all = read_peers(config);
    all.insert(eid.to_owned(), info);
    write_peers(config, &all);
}
