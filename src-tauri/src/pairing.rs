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
    /// Role authority: the folder OWNER's endpoint_id + the current role epoch, so
    /// the accepter knows whose role assignments to trust and from which version.
    #[serde(default)]
    own: Option<String>,
    #[serde(default)]
    eph: u64,
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
        i_am_viewer: false,
        peer_is_viewer: false,
        owner_eid: my_endpoint_id.clone(),
        role_epoch: 0,
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
        own: pair.owner_eid.clone(),
        eph: pair.role_epoch,
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
        i_am_viewer: false,
        peer_is_viewer: false,
        owner_eid: invite.own.clone(),
        role_epoch: invite.eph,
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
        own: pair.owner_eid.clone(),
        eph: pair.role_epoch,
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
        i_am_viewer: false,
        peer_is_viewer: false,
        owner_eid: template.owner_eid.clone(),
        role_epoch: template.role_epoch,
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
        i_am_viewer: false,
        peer_is_viewer: false,
        owner_eid: source.owner_eid.clone(),
        role_epoch: source.role_epoch,
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
        own: new.owner_eid.clone(),
        eph: new.role_epoch,
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
    // A VIEWER never sends — read-only copy, no watcher/sender, no pushes.
    if p.i_am_viewer {
        return false;
    }
    p.mirror || p.two_way || p.role == PairRole::A
}

pub fn runs_listener(p: &Pair) -> bool {
    // We never receive FROM a viewer peer (they don't change the folder).
    if p.peer_is_viewer {
        return false;
    }
    p.mirror || p.two_way || p.role == PairRole::B
}

/// Owner action: set whether the PEER on a given link is a viewer (read-only),
/// and BUMP the group's role epoch so this assignment wins across the mesh (only
/// the owner originates new epochs; everyone else applies the newest one they see
/// from the owner). The change rides the next roster beacon. Returns true if it
/// changed.
pub fn set_peer_viewer(
    config_dir: &Path,
    pair_id: &str,
    viewer: bool,
    my_eid: Option<&str>,
) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let gid = pairs
        .iter()
        .find(|p| p.id == pair_id)
        .and_then(|p| p.group_id.clone());
    // OWNER-ONLY: refuse unless THIS device is the group's owner. The whole mesh
    // trusts the owner_eid we relay, so a non-owner must never originate a role
    // change / epoch bump (else any member could silence an editor).
    let is_owner = match (&gid, my_eid) {
        (Some(g), Some(me)) => pairs.iter().any(|p| {
            p.group_id.as_deref() == Some(g.as_str()) && p.owner_eid.as_deref() == Some(me)
        }),
        _ => false,
    };
    if !is_owner {
        return false;
    }
    let mut changed = false;
    for p in pairs.iter_mut() {
        if p.id == pair_id && p.peer_is_viewer != viewer {
            p.peer_is_viewer = viewer;
            changed = true;
        }
    }
    if changed {
        if let Some(gid) = gid {
            let next = pairs
                .iter()
                .filter(|p| p.group_id.as_deref() == Some(gid.as_str()))
                .map(|p| p.role_epoch)
                .max()
                .unwrap_or(0)
                + 1;
            for p in pairs.iter_mut() {
                if p.group_id.as_deref() == Some(gid.as_str()) {
                    p.role_epoch = next;
                }
            }
        }
        let _ = save(config_dir, &pairs);
    }
    changed
}

/// Apply a group's role roster (endpoint_id → is_viewer) from a control beacon —
/// OWNER-AUTHORITATIVE + MONOTONIC, so roles never flap and a non-owner can't
/// reassign them. Applies ONLY when: the beacon's claimed owner matches the owner
/// we recorded for this group, AND the beacon's epoch is strictly newer than ours.
/// Members relay the owner's (owner, epoch, roles) verbatim, so an offline owner's
/// last assignment still reaches everyone, and the highest epoch always wins.
/// Sets each link's `peer_is_viewer` + our own `i_am_viewer`, and advances every
/// group link's `role_epoch`. Returns true if anything changed.
pub fn apply_group_roles(
    config_dir: &Path,
    group_id: &str,
    my_eid: &str,
    beacon_owner: Option<&str>,
    beacon_epoch: u64,
    roles: &std::collections::HashMap<String, bool>,
) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);

    // The owner + current epoch we recorded for THIS group (all its links agree).
    let my_owner = pairs
        .iter()
        .find(|p| p.group_id.as_deref() == Some(group_id))
        .and_then(|p| p.owner_eid.clone());
    let cur_epoch = pairs
        .iter()
        .filter(|p| p.group_id.as_deref() == Some(group_id))
        .map(|p| p.role_epoch)
        .max()
        .unwrap_or(0);

    // Trust only the owner, and only a strictly-newer assignment. A legacy folder
    // (owner_eid unknown) carries no roles, so there's nothing to apply.
    match (my_owner.as_deref(), beacon_owner) {
        (Some(mine), Some(claimed)) if mine == claimed => {}
        _ => return false,
    }
    if beacon_epoch <= cur_epoch {
        return false;
    }

    let my_role = roles.get(my_eid).copied();
    let mut changed = false;
    for p in pairs.iter_mut() {
        if p.group_id.as_deref() != Some(group_id) {
            continue;
        }
        if p.role_epoch != beacon_epoch {
            p.role_epoch = beacon_epoch;
            changed = true;
        }
        if let Some(v) = my_role {
            if p.i_am_viewer != v {
                p.i_am_viewer = v;
                changed = true;
            }
        }
        if let Some(peer_eid) = &p.endpoint_id {
            if let Some(&v) = roles.get(peer_eid) {
                if p.peer_is_viewer != v {
                    p.peer_is_viewer = v;
                    changed = true;
                }
            }
        }
    }
    if changed {
        let _ = save(config_dir, &pairs);
    }
    changed
}

/// The owner + role epoch recorded for a group (for the roster beacon). Returns
/// (owner_eid, epoch); owner is None on a legacy pre-roles folder.
pub fn group_role_authority(config_dir: &Path, group_id: &str) -> (Option<String>, u64) {
    let pairs = load(config_dir);
    let owner = pairs
        .iter()
        .find(|p| p.group_id.as_deref() == Some(group_id))
        .and_then(|p| p.owner_eid.clone());
    let epoch = pairs
        .iter()
        .filter(|p| p.group_id.as_deref() == Some(group_id))
        .map(|p| p.role_epoch)
        .max()
        .unwrap_or(0);
    (owner, epoch)
}

/// One-time repair: stamp our own endpoint id as `owner_eid` on folders WE created
/// while iroh wasn't up yet (so `owner_eid` was None at create time, leaving the
/// group permanently ownerless and roles inert). We identify "a folder we created"
/// as a group whose EARLIEST link is role A — the original `create()` link. An
/// accepter's earliest link is role B, so a member never wrongly claims ownership.
/// Idempotent: only touches groups where NO link has an owner yet. Returns whether
/// anything changed.
pub fn backfill_owner_eid(config_dir: &Path, my_eid: &str) -> bool {
    if my_eid.is_empty() {
        return false;
    }
    let _guard = LOCK.lock().unwrap();
    let mut pairs = load(config_dir);
    let mut groups: std::collections::HashSet<String> = std::collections::HashSet::new();
    for p in &pairs {
        if let Some(g) = &p.group_id {
            groups.insert(g.clone());
        }
    }
    let mut changed = false;
    for g in groups {
        // Skip groups that already have an owner (assigned correctly at create).
        if pairs
            .iter()
            .any(|p| p.group_id.as_deref() == Some(g.as_str()) && p.owner_eid.is_some())
        {
            continue;
        }
        // Am I the creator? My earliest-created link in this group is role A.
        let i_created = pairs
            .iter()
            .filter(|p| p.group_id.as_deref() == Some(g.as_str()))
            .min_by_key(|p| p.created_at)
            .map(|p| p.role == PairRole::A)
            .unwrap_or(false);
        if !i_created {
            continue;
        }
        for p in pairs
            .iter_mut()
            .filter(|p| p.group_id.as_deref() == Some(g.as_str()))
        {
            p.owner_eid = Some(my_eid.to_string());
            changed = true;
        }
    }
    if changed {
        let _ = save(config_dir, &pairs);
    }
    changed
}

/// Fresh-from-disk role flags for one link: `(peer_is_viewer, i_am_viewer)`.
/// Used right after `apply_group_roles` writes pairs.json, since the in-memory
/// worker handle isn't refreshed until the next reconcile.
pub fn pair_roles(config_dir: &Path, pair_id: &str) -> Option<(bool, bool)> {
    load(config_dir)
        .iter()
        .find(|p| p.id == pair_id)
        .map(|p| (p.peer_is_viewer, p.i_am_viewer))
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
            i_am_viewer: false,
            peer_is_viewer: false,
            owner_eid: None,
            role_epoch: 0,
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

    fn role_test_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("dropbeam-roletest-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Two links in one group: my own (role A) + a link to member B.
    fn group_pair(id: &str, group: &str, owner: Option<&str>, peer_eid: Option<&str>) -> Pair {
        let mut p = pair(PairRole::A);
        p.id = id.into();
        p.mirror = true;
        p.group_id = Some(group.into());
        p.owner_eid = owner.map(|s| s.into());
        p.endpoint_id = peer_eid.map(|s| s.into());
        p
    }

    #[test]
    fn set_peer_viewer_is_owner_only_and_bumps_epoch() {
        let dir = role_test_dir("ownergate");
        // I am the owner ("me"); my link to B carries B's eid.
        let link = group_pair("lb", "g1", Some("me"), Some("B"));
        save(&dir, &[link]).unwrap();

        // A non-owner caller (wrong eid) is refused — no change, no epoch bump.
        assert!(!set_peer_viewer(&dir, "lb", true, Some("not-me")));
        let after = load(&dir);
        assert!(!after[0].peer_is_viewer);
        assert_eq!(after[0].role_epoch, 0);

        // The owner succeeds: B becomes a viewer and the epoch advances.
        assert!(set_peer_viewer(&dir, "lb", true, Some("me")));
        let after = load(&dir);
        assert!(after[0].peer_is_viewer);
        assert_eq!(after[0].role_epoch, 1);
    }

    #[test]
    fn apply_group_roles_owner_authoritative_and_monotonic() {
        let dir = role_test_dir("apply");
        // We are member "me"; owner is "owner". One link to peer "B".
        let link = group_pair("lb", "g1", Some("owner"), Some("B"));
        save(&dir, &[link]).unwrap();

        let roles_b_viewer: std::collections::HashMap<String, bool> =
            [("B".to_string(), true)].into_iter().collect();

        // A NON-owner beacon is ignored even at a higher epoch.
        assert!(!apply_group_roles(&dir, "g1", "me", Some("imposter"), 9, &roles_b_viewer));
        assert!(!load(&dir)[0].peer_is_viewer);

        // The owner's newer-epoch beacon applies: B → viewer, epoch advances.
        assert!(apply_group_roles(&dir, "g1", "me", Some("owner"), 5, &roles_b_viewer));
        let after = load(&dir);
        assert!(after[0].peer_is_viewer);
        assert_eq!(after[0].role_epoch, 5);

        // A STALE re-broadcast (<= current epoch) is rejected → no flap.
        let roles_b_editor: std::collections::HashMap<String, bool> =
            [("B".to_string(), false)].into_iter().collect();
        assert!(!apply_group_roles(&dir, "g1", "me", Some("owner"), 5, &roles_b_editor));
        assert!(load(&dir)[0].peer_is_viewer); // unchanged

        // The owner promotes B back at a newer epoch → converges to editor.
        assert!(apply_group_roles(&dir, "g1", "me", Some("owner"), 6, &roles_b_editor));
        let after = load(&dir);
        assert!(!after[0].peer_is_viewer);
        assert_eq!(after[0].role_epoch, 6);
    }

    #[test]
    fn apply_group_roles_sets_my_own_viewer_flag() {
        let dir = role_test_dir("selfrole");
        let link = group_pair("lb", "g1", Some("owner"), Some("B"));
        save(&dir, &[link]).unwrap();
        // The owner marks ME ("me") a viewer.
        let roles: std::collections::HashMap<String, bool> =
            [("me".to_string(), true)].into_iter().collect();
        assert!(apply_group_roles(&dir, "g1", "me", Some("owner"), 2, &roles));
        assert!(load(&dir)[0].i_am_viewer);
    }

    #[test]
    fn backfill_claims_only_folders_i_created() {
        let dir = role_test_dir("backfill");
        // A folder I created (earliest link is role A), still ownerless.
        let mut created = group_pair("mine", "gc", None, Some("peer"));
        created.created_at = 100;
        // A folder I ACCEPTED (my earliest link is role B), ownerless.
        let mut accepted = group_pair("theirs", "ga", None, Some("owner-peer"));
        accepted.role = PairRole::B;
        accepted.created_at = 50;
        save(&dir, &[created, accepted]).unwrap();

        assert!(backfill_owner_eid(&dir, "me"));
        let after = load(&dir);
        let mine = after.iter().find(|p| p.id == "mine").unwrap();
        let theirs = after.iter().find(|p| p.id == "theirs").unwrap();
        assert_eq!(mine.owner_eid.as_deref(), Some("me")); // I claimed it
        assert_eq!(theirs.owner_eid, None); // I did NOT claim a folder I joined
    }
}
