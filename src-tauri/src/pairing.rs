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
    /// Total-sync / mirror mode — both sides must agree, so it rides the invite.
    #[serde(default)]
    mir: bool,
    /// The inviter's iroh EndpointId, so the accepter can sync this folder
    /// directly over iroh (dial-by-key) instead of the croc relay. The accepter
    /// hands back their own id via a "folder-hello" right after accepting.
    #[serde(default)]
    eid: Option<String>,
    /// Multi-person folders: the group all the folder's pairwise links share. The
    /// rest of the roster propagates over the control beacon, so the invite only
    /// needs to carry the group id. None on a classic 1:1 invite.
    #[serde(default)]
    gid: Option<String>,
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
    mirror: bool,
    my_endpoint_id: Option<String>,
) -> Result<(Pair, String), String> {
    validate_folder(&folder)?;
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    if pairs.iter().any(|p| same_path(&p.folder, &folder)) {
        return Err("That folder is already a Shared Drop Folder.".into());
    }

    let id = uuid::Uuid::new_v4().to_string();
    let secret = random_secret();
    // Every new folder gets a group id so it can grow to 3+ people later; a 1:1
    // folder is just a group of two (the roster reconciliation is a no-op there).
    let group_id = uuid::Uuid::new_v4().to_string();
    let peer_label = peer_name.trim().to_string();
    let auto_friend = !peer_label.is_empty();
    // Mirror is inherently two-way.
    let two_way = two_way || mirror;
    let pair = Pair {
        id: id.clone(),
        role: PairRole::A,
        peer_name: peer_label.clone(),
        secret: secret.clone(),
        folder,
        two_way,
        mirror,
        auto_delete: false,
        delete_mode: DeleteMode::Trash,
        created_at: now_ms(),
        // The accepter's id arrives later via their folder-hello.
        endpoint_id: None,
        group_id: Some(group_id.clone()),
    };

    let invite = Invite {
        v: 1,
        id,
        secret,
        name: my_name,
        tw: two_way,
        frn: auto_friend,
        mir: mirror,
        eid: my_endpoint_id,
        gid: Some(group_id),
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
        two_way: invite.tw || invite.mir,
        mirror: invite.mir,
        auto_delete: false,
        delete_mode: DeleteMode::Trash,
        created_at: now_ms(),
        // Learned straight from the invite — lets us sync directly over iroh.
        endpoint_id: invite.eid.clone(),
        group_id: invite.gid.clone(),
    };
    pairs.push(pair.clone());
    save(config_dir, &pairs)?;
    // Folder partners are always friends — that's how you see who's in a folder
    // and beam to them by name. We know the inviter's name from the invite; they
    // learn ours over the control channel's hello.
    friends::upsert_from_pairing(config_dir, &pair.peer_name, &pair.secret, PairRole::B);
    Ok(pair)
}

/// Record (or update) a pair peer's iroh EndpointId — called when the accepter's
/// "folder-hello" reaches the creator, so the creator can also push directly over
/// iroh. Returns true if a pair was found and changed.
pub fn set_endpoint_id(config_dir: &Path, pair_id: &str, endpoint_id: String) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let mut changed = false;
    for p in pairs.iter_mut() {
        if p.id == pair_id && p.endpoint_id.as_deref() != Some(endpoint_id.as_str()) {
            p.endpoint_id = Some(endpoint_id.clone());
            changed = true;
        }
    }
    if changed {
        let _ = save(config_dir, &pairs);
    }
    changed
}

pub fn update(
    config_dir: &Path,
    id: &str,
    two_way: Option<bool>,
    auto_delete: Option<bool>,
    delete_mode: Option<DeleteMode>,
    peer_name: Option<String>,
    mirror: Option<bool>,
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
    if let Some(v) = mirror {
        pair.mirror = v;
        if v {
            pair.two_way = true; // mirror is inherently two-way
            pair.auto_delete = false; // and mutually exclusive with auto-delete
        }
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
pub fn invite_for(pair: &Pair, my_name: &str, my_endpoint_id: Option<String>) -> String {
    let invite = Invite {
        v: 1,
        id: pair.id.clone(),
        secret: pair.secret.clone(),
        name: my_name.to_string(),
        tw: pair.two_way,
        // Re-offer the friend link iff this pair was created with a named peer.
        frn: !pair.peer_name.trim().is_empty(),
        mir: pair.mirror,
        eid: my_endpoint_id,
        gid: pair.group_id.clone(),
    };
    let json = serde_json::to_string(&invite).unwrap_or_default();
    format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

/// Pairs that share a folder group (all the pairwise links of one N-person folder).
pub fn members_of_group(config_dir: &Path, group_id: &str) -> Vec<Pair> {
    load(config_dir)
        .into_iter()
        .filter(|p| p.group_id.as_deref() == Some(group_id))
        .collect()
}

/// Ensure a pairwise link exists to `endpoint_id` within a folder group (used by
/// the control-beacon roster reconciliation to mesh everyone with everyone).
/// `template` is any existing pair in the group — we copy its folder + sync mode.
/// Returns the new pair if one was created, or None if it already existed / self.
pub fn ensure_member(
    config_dir: &Path,
    group_id: &str,
    template: &Pair,
    endpoint_id: &str,
    name: &str,
    my_endpoint_id: &str,
) -> Option<Pair> {
    if endpoint_id.is_empty() || endpoint_id == my_endpoint_id {
        return None; // never pair with ourselves
    }
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    if pairs
        .iter()
        .any(|p| p.group_id.as_deref() == Some(group_id) && p.endpoint_id.as_deref() == Some(endpoint_id))
    {
        return None; // already meshed with this member
    }
    let new = Pair {
        // CRITICAL: both ends must derive the SAME id, because the folder-files /
        // folder-ctrl handlers authorize by matching the wire pair_id to a local
        // pair. A random uuid here would never match the other side → the link
        // could never exchange anything. Derive it from the group + the two keys.
        id: derive_link_id(group_id, my_endpoint_id, endpoint_id),
        role: PairRole::B,
        peer_name: clean_name(name),
        // Deterministic so both ends of this new link derive the same channels.
        secret: derive_group_secret(group_id, my_endpoint_id, endpoint_id),
        folder: template.folder.clone(),
        two_way: template.two_way,
        mirror: template.mirror,
        auto_delete: template.auto_delete,
        delete_mode: template.delete_mode,
        created_at: now_ms(),
        endpoint_id: Some(endpoint_id.to_string()),
        group_id: Some(group_id.to_string()),
    };
    pairs.push(new.clone());
    let _ = save(config_dir, &pairs);
    // Folder partners are friends, so they show up by name + you can chat them.
    friends::upsert_from_pairing(config_dir, &new.peer_name, &new.secret, PairRole::B);
    Some(new)
}

fn clean_name(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        "Member".into()
    } else {
        n.to_string()
    }
}

/// A stable LINK id for a group mesh link, identical on both ends (so each side's
/// `pair_id` matches the other's and folder pushes/beacons are authorized).
fn derive_link_id(group_id: &str, a: &str, b: &str) -> String {
    use sha2::{Digest, Sha256};
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut h = Sha256::new();
    h.update(b"link|");
    h.update(group_id.as_bytes());
    h.update(b"|");
    h.update(lo.as_bytes());
    h.update(b"|");
    h.update(hi.as_bytes());
    format!("g{}", hex::encode(&h.finalize()[..16]))
}

/// A stable secret for a group link, identical on both ends (order-independent).
fn derive_group_secret(group_id: &str, a: &str, b: &str) -> String {
    use sha2::{Digest, Sha256};
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut h = Sha256::new();
    h.update(group_id.as_bytes());
    h.update(b"|");
    h.update(lo.as_bytes());
    h.update(b"|");
    h.update(hi.as_bytes());
    hex::encode(h.finalize())
}

/// Invite another person into an EXISTING folder's group. Creates a fresh
/// inviter→newcomer link (sharing the folder + group), and returns an invite the
/// newcomer accepts. A folder created before groups existed is upgraded in place:
/// every link on that folder is stamped with a new group id. The rest of the
/// roster reaches the newcomer over the control beacon once they're in.
pub fn group_invite(
    config_dir: &Path,
    source_pair_id: &str,
    my_name: String,
    my_endpoint_id: Option<String>,
) -> Result<String, String> {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let source = pairs
        .iter()
        .find(|p| p.id == source_pair_id)
        .cloned()
        .ok_or("That shared folder no longer exists.")?;
    // Resolve (or assign) the group id, upgrading a pre-groups folder in place.
    let group_id = match source.group_id.clone() {
        Some(g) => g,
        None => {
            let g = uuid::Uuid::new_v4().to_string();
            for p in pairs.iter_mut() {
                if same_path(&p.folder, &source.folder) {
                    p.group_id = Some(g.clone());
                }
            }
            g
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    let secret = random_secret();
    let new = Pair {
        id: id.clone(),
        role: PairRole::A,
        peer_name: String::new(),
        secret: secret.clone(),
        folder: source.folder.clone(),
        two_way: source.two_way,
        mirror: source.mirror,
        auto_delete: source.auto_delete,
        delete_mode: source.delete_mode,
        created_at: now_ms(),
        endpoint_id: None, // arrives via the newcomer's folder-hello
        group_id: Some(group_id.clone()),
    };
    let invite = Invite {
        v: 1,
        id,
        secret,
        name: my_name,
        tw: new.two_way,
        frn: true,
        mir: new.mirror,
        eid: my_endpoint_id,
        gid: Some(group_id),
    };
    let json = serde_json::to_string(&invite).map_err(|e| e.to_string())?;
    let encoded = format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json));
    pairs.push(new);
    save(config_dir, &pairs)?;
    Ok(encoded)
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

/// Control channel (presence/identity/sync events) this side SENDS on. Separate
/// from the file channels so a tiny hello never collides with a file transfer.
pub fn control_outbound_code(pair: &Pair) -> String {
    let channel = match pair.role {
        PairRole::A => "ctrl-a2b",
        PairRole::B => "ctrl-b2a",
    };
    derive_code(&pair.secret, channel)
}

/// Control channel this side RECEIVES on.
pub fn control_inbound_code(pair: &Pair) -> String {
    let channel = match pair.role {
        PairRole::A => "ctrl-b2a",
        PairRole::B => "ctrl-a2b",
    };
    derive_code(&pair.secret, channel)
}

/// Persist a peer name learned over the control channel. Returns true if it
/// actually changed (so the caller can refresh the UI + auto-friend once).
pub fn set_peer_name(config_dir: &Path, id: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let mut changed = false;
    if let Some(p) = pairs.iter_mut().find(|p| p.id == id) {
        if p.peer_name != name {
            p.peer_name = name.to_string();
            changed = true;
        }
    }
    if changed {
        let _ = save(config_dir, &pairs);
    }
    changed
}

fn derive_code(secret: &str, channel: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b":");
    h.update(channel.as_bytes());
    hex::encode(&h.finalize()[..12]) // 24 hex chars, well above croc's 6-char min
}

pub fn runs_sender(p: &Pair) -> bool {
    p.mirror || p.two_way || p.role == PairRole::A
}

pub fn runs_listener(p: &Pair) -> bool {
    p.mirror || p.two_way || p.role == PairRole::B
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
            mirror: false,
            auto_delete: false,
            delete_mode: DeleteMode::Trash,
            created_at: 0,
            endpoint_id: None,
            group_id: None,
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
