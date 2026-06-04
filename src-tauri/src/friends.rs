//! Friends — named peers you can send files to directly, no per-transfer code.
//!
//! Same shared-secret/derived-channel model as pairing: A sends to B on "fa2b"
//! and listens on "fb2a"; B is the mirror. The person who creates the invite (or
//! the folder) labels the peer's name, and the invite carries their own name — so
//! both sides end up with a name without any extra handshake.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{Friend, PairRole};
use crate::settings::write_atomic;

static LOCK: Mutex<()> = Mutex::new(());
const INVITE_PREFIX: &str = "dropbeamf1:";

pub fn friends_path(config_dir: &Path) -> PathBuf {
    config_dir.join("friends.json")
}

pub fn load(config_dir: &Path) -> Vec<Friend> {
    match fs::read_to_string(friends_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save(config_dir: &Path, friends: &[Friend]) -> Result<(), String> {
    let _ = fs::create_dir_all(config_dir);
    let txt = serde_json::to_string_pretty(friends).map_err(|e| e.to_string())?;
    write_atomic(&friends_path(config_dir), txt.as_bytes()).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
struct Invite {
    v: u8,
    id: String,
    secret: String,
    /// The inviter's own name, so the accepter sees who added them.
    name: String,
}

/// Create a friend invite (this device is A). `friend_name` is your label for them.
pub fn create(
    config_dir: &Path,
    my_name: String,
    friend_name: String,
) -> Result<(Friend, String), String> {
    let _guard = LOCK.lock().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let secret = random_secret();
    let friend = Friend {
        id: id.clone(),
        role: PairRole::A,
        name: clean_name(&friend_name, "New friend"),
        secret: secret.clone(),
        created_at: now_ms(),
    };
    let invite = Invite {
        v: 1,
        id,
        secret,
        name: my_name,
    };
    let json = serde_json::to_string(&invite).map_err(|e| e.to_string())?;
    let encoded = format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json));

    let mut friends = load(config_dir);
    friends.push(friend.clone());
    save(config_dir, &friends)?;
    Ok((friend, encoded))
}

/// Accept a friend invite (this device is B). The friend is named after the inviter.
pub fn accept(config_dir: &Path, invite_str: &str) -> Result<Friend, String> {
    let body = invite_str
        .trim()
        .strip_prefix(INVITE_PREFIX)
        .ok_or("That doesn't look like a friend invite.")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body.trim())
        .map_err(|_| "The friend invite is malformed.".to_string())?;
    let invite: Invite =
        serde_json::from_slice(&bytes).map_err(|_| "The friend invite is malformed.".to_string())?;

    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    if friends.iter().any(|f| f.id == invite.id) {
        return Err("You're already friends with this person.".into());
    }
    let friend = Friend {
        id: invite.id,
        role: PairRole::B,
        name: clean_name(&invite.name, "Friend"),
        secret: invite.secret,
        created_at: now_ms(),
    };
    friends.push(friend.clone());
    save(config_dir, &friends)?;
    Ok(friend)
}

/// Rebuild a friend's invite (so the inviter can show it again).
pub fn invite_for(friend: &Friend, my_name: &str) -> String {
    let invite = Invite {
        v: 1,
        id: friend.id.clone(),
        secret: friend.secret.clone(),
        name: my_name.to_string(),
    };
    let json = serde_json::to_string(&invite).unwrap_or_default();
    format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

/// Add a friend derived from a folder pairing (dedup by name). The friend secret
/// is derived from the pair secret so the friend channels never collide with the
/// folder channels. Both sides derive the same friend secret + matching roles.
pub fn upsert_from_pairing(config_dir: &Path, name: &str, pair_secret: &str, role: PairRole) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    if friends.iter().any(|f| f.name.eq_ignore_ascii_case(name)) {
        return; // already a friend by that name
    }
    friends.push(Friend {
        id: uuid::Uuid::new_v4().to_string(),
        role,
        name: name.to_string(),
        secret: derive_friend_secret(pair_secret),
        created_at: now_ms(),
    });
    let _ = save(config_dir, &friends);
}

pub fn rename(config_dir: &Path, id: &str, name: String) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    if let Some(f) = friends.iter_mut().find(|f| f.id == id) {
        if !name.trim().is_empty() {
            f.name = name.trim().to_string();
        }
    }
    save(config_dir, &friends)
}

pub fn remove(config_dir: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    friends.retain(|f| f.id != id);
    save(config_dir, &friends)
}

pub fn get(config_dir: &Path, id: &str) -> Option<Friend> {
    load(config_dir).into_iter().find(|f| f.id == id)
}

/// Channel I LISTEN on for files this friend sends me.
pub fn my_inbox_code(friend: &Friend) -> String {
    let channel = match friend.role {
        PairRole::A => "fb2a",
        PairRole::B => "fa2b",
    };
    derive_code(&friend.secret, channel)
}

/// Channel I SEND on to reach this friend.
pub fn friend_inbox_code(friend: &Friend) -> String {
    let channel = match friend.role {
        PairRole::A => "fa2b",
        PairRole::B => "fb2a",
    };
    derive_code(&friend.secret, channel)
}

fn derive_code(secret: &str, channel: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b":");
    h.update(channel.as_bytes());
    hex::encode(&h.finalize()[..12])
}

fn derive_friend_secret(pair_secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(pair_secret.as_bytes());
    h.update(b":friend");
    hex::encode(h.finalize())
}

fn clean_name(name: &str, fallback: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        fallback.to_string()
    } else {
        n.to_string()
    }
}

fn random_secret() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
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

    fn friend(role: PairRole) -> Friend {
        Friend {
            id: "x".into(),
            role,
            name: "n".into(),
            secret: "sharedsecret123".into(),
            created_at: 0,
        }
    }

    #[test]
    fn friend_channels_match_across_roles() {
        let a = friend(PairRole::A);
        let b = friend(PairRole::B);
        // What A sends on must equal what B listens on, and vice-versa.
        assert_eq!(friend_inbox_code(&a), my_inbox_code(&b));
        assert_eq!(friend_inbox_code(&b), my_inbox_code(&a));
        assert_ne!(my_inbox_code(&a), my_inbox_code(&b));
    }
}
