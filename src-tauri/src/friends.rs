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
/// Prefix for the permanent, reusable personal code (carries your stable
/// EndpointId + name). Share it once; it works forever and across updates.
const USER_PREFIX: &str = "dropbeam:";

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
    /// The inviter's iroh EndpointId, so the accepter can reach them directly.
    #[serde(default)]
    endpoint_id: Option<String>,
}

/// Create a friend invite (this device is A). `friend_name` is your label for them.
/// `my_endpoint_id` is our iroh id, embedded so the accepter can send to us directly.
pub fn create(
    config_dir: &Path,
    my_name: String,
    friend_name: String,
    my_endpoint_id: Option<String>,
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
        auto_accept: true,
        endpoint_id: None, // learned when they accept + say hello
    };
    let invite = Invite {
        v: 1,
        id,
        secret,
        name: my_name,
        endpoint_id: my_endpoint_id,
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
        auto_accept: true,
        endpoint_id: invite.endpoint_id, // the inviter's id, for direct sends
    };
    friends.push(friend.clone());
    save(config_dir, &friends)?;
    Ok(friend)
}

/// Rebuild a friend's invite (so the inviter can show it again).
pub fn invite_for(friend: &Friend, my_name: &str, my_endpoint_id: Option<String>) -> String {
    let invite = Invite {
        v: 1,
        id: friend.id.clone(),
        secret: friend.secret.clone(),
        name: my_name.to_string(),
        endpoint_id: my_endpoint_id,
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
        auto_accept: true,
        endpoint_id: None,
    });
    let _ = save(config_dir, &friends);
}

/// Record a friend's iroh EndpointId (learned when they say hello after pairing).
/// Returns true if a friend was updated.
pub fn set_endpoint_id(config_dir: &Path, id: &str, endpoint_id: String) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    let mut changed = false;
    if let Some(f) = friends.iter_mut().find(|f| f.id == id) {
        if f.endpoint_id.as_deref() != Some(endpoint_id.as_str()) {
            f.endpoint_id = Some(endpoint_id);
            changed = true;
        }
    }
    if changed {
        let _ = save(config_dir, &friends);
    }
    changed
}

// ---------------------------------------------------------------------------
// Permanent personal code + EndpointId-keyed friendships
//
// Croc is gone: friends are reached purely by their stable iroh EndpointId, so a
// friend is really just {name, endpoint_id}. That key never changes across app
// updates, which means a friendship — once established — is permanent. These
// helpers make that explicit: a reusable code you share once, dedup-by-key so
// re-adding never duplicates or wipes history, and name auto-exchange so neither
// side has to type the other's name.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct UserCode {
    v: u8,
    /// This device's stable iroh EndpointId — the permanent address peers dial.
    eid: String,
    /// The owner's display name, so the other side never has to type it.
    name: String,
}

/// A permanent, reusable personal invite: share it once and anyone can add you
/// and reach you forever. It carries your stable EndpointId + name — no secret,
/// because friends are reached purely by EndpointId now.
pub fn my_code(my_name: &str, my_endpoint_id: &str) -> String {
    let code = UserCode {
        v: 1,
        eid: my_endpoint_id.to_string(),
        name: my_name.trim().to_string(),
    };
    let json = serde_json::to_string(&code).unwrap_or_default();
    format!("{USER_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

fn decode_user_code(code: &str) -> Result<UserCode, String> {
    let body = code
        .trim()
        .strip_prefix(USER_PREFIX)
        .ok_or("That doesn't look like a DropBeam code.")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body.trim())
        .map_err(|_| "That DropBeam code is malformed.".to_string())?;
    let uc: UserCode =
        serde_json::from_slice(&bytes).map_err(|_| "That DropBeam code is malformed.".to_string())?;
    if uc.eid.trim().is_empty() {
        return Err("That code is missing a device id.".into());
    }
    Ok(uc)
}

fn is_placeholder(name: &str) -> bool {
    matches!(name.trim(), "" | "New friend" | "Friend")
}

/// Find a friend by their EndpointId and refresh their name, or add them new.
/// The stable EndpointId IS the identity, so this NEVER duplicates a friend (or
/// loses their chat history) across app updates or re-pairs. Returns the friend.
pub fn upsert_by_endpoint(config_dir: &Path, endpoint_id: &str, name: &str) -> Friend {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    let name = name.trim();
    if let Some(f) = friends
        .iter_mut()
        .find(|f| f.endpoint_id.as_deref() == Some(endpoint_id))
    {
        // Refresh the name only if ours is still a placeholder — never clobber a
        // name the user deliberately set.
        if !name.is_empty() && is_placeholder(&f.name) {
            f.name = name.to_string();
        }
        let out = f.clone();
        let _ = save(config_dir, &friends);
        return out;
    }
    let friend = Friend {
        id: uuid::Uuid::new_v4().to_string(),
        role: PairRole::B,
        name: clean_name(name, "Friend"),
        secret: random_secret(),
        created_at: now_ms(),
        auto_accept: true,
        endpoint_id: Some(endpoint_id.to_string()),
    };
    friends.push(friend.clone());
    let _ = save(config_dir, &friends);
    friend
}

/// Add a friend from their permanent personal code (dedup by EndpointId).
pub fn add_by_code(config_dir: &Path, code: &str) -> Result<Friend, String> {
    let uc = decode_user_code(code)?;
    Ok(upsert_by_endpoint(config_dir, &uc.eid, &uc.name))
}

/// Apply an incoming friend-hello. If `friend_id` matches an existing record
/// (classic invite flow), learn their EndpointId + name. Otherwise auto-add the
/// sender by their EndpointId (the permanent-code reverse direction) so one code
/// share makes the friendship two-way.
pub fn apply_hello(config_dir: &Path, friend_id: &str, endpoint_id: &str, name: &str) {
    if endpoint_id.trim().is_empty() {
        return;
    }
    if !friend_id.is_empty() {
        let matched = {
            let _guard = LOCK.lock().unwrap();
            let mut friends = load(config_dir);
            if let Some(f) = friends.iter_mut().find(|f| f.id == friend_id) {
                let mut changed = false;
                if f.endpoint_id.as_deref() != Some(endpoint_id) {
                    f.endpoint_id = Some(endpoint_id.to_string());
                    changed = true;
                }
                if !name.trim().is_empty() && is_placeholder(&f.name) {
                    f.name = name.trim().to_string();
                    changed = true;
                }
                if changed {
                    let _ = save(config_dir, &friends);
                }
                true
            } else {
                false
            }
        };
        if matched {
            return;
        }
    }
    let _ = upsert_by_endpoint(config_dir, endpoint_id, name);
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

pub fn set_auto_accept(config_dir: &Path, id: &str, auto_accept: bool) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    if let Some(f) = friends.iter_mut().find(|f| f.id == id) {
        f.auto_accept = auto_accept;
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
            auto_accept: true,
            endpoint_id: None,
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
