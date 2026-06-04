//! Shared Drop Folder pairing: persistence, invite encode/decode, and the
//! per-channel transfer codes derived from the pair secret.
//!
//! Recurring prompt-free transfers work without any signaling channel: each
//! direction gets a FIXED code derived from the shared secret. The receiving
//! side runs a persistent `croc <code>` listen loop; the sending side runs
//! `croc send --code <code>` on demand. They rendezvous on the relay via the
//! shared code. A sends on "a2b" / listens on "b2a"; B is the mirror, so the
//! two directions never collide.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::friends;
use crate::models::{DeleteMode, Pair, PairRole};
use crate::settings::write_atomic;

static LOCK: Mutex<()> = Mutex::new(());
const INVITE_PREFIX: &str = "dropbeam1:";

pub fn pairs_path(config_dir: &Path) -> PathBuf {
    config_dir.join("pairs.json")
}

pub fn load(config_dir: &Path) -> Vec<Pair> {
    match fs::read_to_string(pairs_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save(config_dir: &Path, pairs: &[Pair]) -> Result<(), String> {
    let _ = fs::create_dir_all(config_dir);
    let txt = serde_json::to_string_pretty(pairs).map_err(|e| e.to_string())?;
    write_atomic(&pairs_path(config_dir), txt.as_bytes()).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
struct Invite {
    v: u8,
    id: String,
    secret: String,
    name: String,
    tw: bool,
    /// Whether accepting this invite should also link the two as friends. Set
    /// only when the creator named the peer, so both sides end up with a
    /// matching friend record (never a one-sided, non-working one).
    #[serde(default)]
    frn: bool,
}

/// Create a new pair (this device is A). Returns the pair + a shareable invite.
/// `peer_name` is the creator's optional label for the friend; when present,
/// both sides auto-link as friends on accept.
pub fn create(
    config_dir: &Path,
    folder: String,
    my_name: String,
    two_way: bool,
    peer_name: String,
) -> Result<(Pair, String), String> {
    validate_folder(&folder)?;
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    if pairs.iter().any(|p| same_path(&p.folder, &folder)) {
        return Err("That folder is already a Shared Drop Folder.".into());
    }

    let id = uuid::Uuid::new_v4().to_string();
    let secret = random_secret();
    let peer_label = peer_name.trim().to_string();
    let auto_friend = !peer_label.is_empty();
    let pair = Pair {
        id: id.clone(),
        role: PairRole::A,
        peer_name: peer_label.clone(),
        secret: secret.clone(),
        folder,
        two_way,
        auto_delete: false,
        delete_mode: DeleteMode::Trash,
        created_at: now_ms(),
    };

    let invite = Invite {
        v: 1,
        id,
        secret,
        name: my_name,
        tw: two_way,
        frn: auto_friend,
    };
    let json = serde_json::to_string(&invite).map_err(|e| e.to_string())?;
    let encoded = format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json));

    pairs.push(pair.clone());
    save(config_dir, &pairs)?;
    // Creator (role A) labels the peer, so we can register the friend right away.
    if auto_friend {
        friends::upsert_from_pairing(config_dir, &peer_label, &pair.secret, PairRole::A);
    }
    Ok((pair, encoded))
}

/// Accept an invite (this device is B).
pub fn accept(config_dir: &Path, invite_str: &str, folder: String) -> Result<Pair, String> {
    validate_folder(&folder)?;
    let body = invite_str
        .trim()
        .strip_prefix(INVITE_PREFIX)
        .ok_or("That doesn't look like a DropBeam invite code.")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body.trim())
        .map_err(|_| "The invite code is malformed.".to_string())?;
    let invite: Invite =
        serde_json::from_slice(&bytes).map_err(|_| "The invite code is malformed.".to_string())?;

    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    if pairs.iter().any(|p| p.id == invite.id) {
        return Err("You're already paired with this invite.".into());
    }
    if pairs.iter().any(|p| same_path(&p.folder, &folder)) {
        return Err("That folder is already a Shared Drop Folder.".into());
    }

    let auto_friend = invite.frn;
    let pair = Pair {
        id: invite.id,
        role: PairRole::B,
        peer_name: if invite.name.trim().is_empty() {
            "Peer".into()
        } else {
            invite.name
        },
        secret: invite.secret,
        folder,
        two_way: invite.tw,
        auto_delete: false,
        delete_mode: DeleteMode::Trash,
        created_at: now_ms(),
    };
    pairs.push(pair.clone());
    save(config_dir, &pairs)?;
    // Mirror the creator: if they linked us as a friend, link them back (role B),
    // named after the inviter. Both sides derive the same friend channels.
    if auto_friend {
        friends::upsert_from_pairing(config_dir, &pair.peer_name, &pair.secret, PairRole::B);
    }
    Ok(pair)
}

pub fn update(
    config_dir: &Path,
    id: &str,
    two_way: Option<bool>,
    auto_delete: Option<bool>,
    delete_mode: Option<DeleteMode>,
    peer_name: Option<String>,
) -> Result<Pair, String> {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let pair = pairs
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("Pair not found.")?;
    if let Some(v) = two_way {
        pair.two_way = v;
    }
    if let Some(v) = auto_delete {
        pair.auto_delete = v;
    }
    if let Some(v) = delete_mode {
        pair.delete_mode = v;
    }
    if let Some(v) = peer_name {
        if !v.trim().is_empty() {
            pair.peer_name = v;
        }
    }
    let updated = pair.clone();
    save(config_dir, &pairs)?;
    Ok(updated)
}

/// Rebuild the shareable invite for an existing pair (so the creator can show
/// it again). Only meaningful for the inviter (role A).
pub fn invite_for(pair: &Pair, my_name: &str) -> String {
    let invite = Invite {
        v: 1,
        id: pair.id.clone(),
        secret: pair.secret.clone(),
        name: my_name.to_string(),
        tw: pair.two_way,
        // Re-offer the friend link iff this pair was created with a named peer.
        frn: !pair.peer_name.trim().is_empty(),
    };
    let json = serde_json::to_string(&invite).unwrap_or_default();
    format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

pub fn remove(config_dir: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    pairs.retain(|p| p.id != id);
    save(config_dir, &pairs)
}

/// The code phrase this side SENDS on.
pub fn outbound_code(pair: &Pair) -> String {
    let channel = match pair.role {
        PairRole::A => "a2b",
        PairRole::B => "b2a",
    };
    derive_code(&pair.secret, channel)
}

/// The code phrase this side RECEIVES on.
pub fn inbound_code(pair: &Pair) -> String {
    let channel = match pair.role {
        PairRole::A => "b2a",
        PairRole::B => "a2b",
    };
    derive_code(&pair.secret, channel)
}

fn derive_code(secret: &str, channel: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b":");
    h.update(channel.as_bytes());
    hex::encode(&h.finalize()[..12]) // 24 hex chars, well above croc's 6-char min
}

pub fn runs_sender(p: &Pair) -> bool {
    p.two_way || p.role == PairRole::A
}

pub fn runs_listener(p: &Pair) -> bool {
    p.two_way || p.role == PairRole::B
}

fn random_secret() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn validate_folder(folder: &str) -> Result<(), String> {
    let p = Path::new(folder);
    if folder.trim().is_empty() {
        return Err("Choose a folder first.".into());
    }
    if !p.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    Ok(())
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    norm(a) == norm(b)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(role: PairRole) -> Pair {
        Pair {
            id: "x".into(),
            role,
            peer_name: String::new(),
            secret: "deadbeefcafebabe".into(),
            folder: "/tmp".into(),
            two_way: true,
            auto_delete: false,
            delete_mode: DeleteMode::Trash,
            created_at: 0,
        }
    }

    #[test]
    fn channels_match_across_roles() {
        let a = pair(PairRole::A);
        let b = pair(PairRole::B);
        // What A sends on must equal what B listens on, and vice versa.
        assert_eq!(outbound_code(&a), inbound_code(&b));
        assert_eq!(inbound_code(&a), outbound_code(&b));
        // The two directions are distinct so senders never collide.
        assert_ne!(outbound_code(&a), outbound_code(&b));
        // Code is well above croc's 6-char minimum.
        assert!(outbound_code(&a).len() >= 6);
    }

    #[test]
    fn roles_decide_who_runs_what() {
        let mut a = pair(PairRole::A);
        a.two_way = false;
        let mut b = pair(PairRole::B);
        b.two_way = false;
        // One-way: A sends only, B listens only.
        assert!(runs_sender(&a) && !runs_listener(&a));
        assert!(!runs_sender(&b) && runs_listener(&b));
    }
}
