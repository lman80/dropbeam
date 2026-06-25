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

static LOCK: Mutex<()> = Mutex::new(());
const INVITE_PREFIX: &str = "dropbeamf1:";
/// Prefix for the permanent, reusable personal code (carries your stable
/// EndpointId + name). Share it once; it works forever and across updates.
const USER_PREFIX: &str = "dropbeam:";

pub fn friends_path(config_dir: &Path) -> PathBuf {
    config_dir.join("friends.json")
}

pub fn load(config_dir: &Path) -> Vec<Friend> {
    // Resilient read (retry transient failures + recover from .bak), then parse
    // element-wise so one corrupt/forward-incompatible friend record drops only
    // itself instead of wiping every friend.
    crate::settings::read_json_array_resilient(&friends_path(config_dir))
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Persist the friends list. `allow_empty` MUST be true only for paths that
/// legitimately empty the list (removing your last friend) — every other caller
/// passes false so a stray empty list (a logic/IO glitch) can never silently wipe
/// a populated friends.json. That clobber is exactly the "I lost my friend's
/// contact after update" failure, so we refuse it: if we're about to write `[]`
/// but the file on disk still holds real records, abort rather than overwrite.
fn save_inner(config_dir: &Path, friends: &[Friend], allow_empty: bool) -> Result<(), String> {
    let _ = fs::create_dir_all(config_dir);
    let txt = serde_json::to_string_pretty(friends).map_err(|e| e.to_string())?;
    if friends.is_empty() && !allow_empty {
        // Use the resilient reader so a transiently-unreadable primary (with a good
        // .bak) still blocks the empty write instead of clobbering it.
        let on_disk = crate::settings::read_json_array_resilient(&friends_path(config_dir)).len();
        if on_disk > 0 {
            log::warn!(
                "friends::save refused to overwrite {on_disk} existing friend(s) with an empty list"
            );
            return Err("refusing to clobber existing friends with an empty list".into());
        }
    }
    crate::settings::write_atomic_with_backup(
        &friends_path(config_dir),
        txt.as_bytes(),
        !friends.is_empty() || allow_empty,
    )
    .map_err(|e| e.to_string())
}

fn save(config_dir: &Path, friends: &[Friend]) -> Result<(), String> {
    save_inner(config_dir, friends, false)
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
        avatar: None,
        name_custom: false,
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
        avatar: None,
        name_custom: false,
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
        avatar: None,
        name_custom: false,
    });
    let _ = save(config_dir, &friends);
}

/// True for the auto-generated placeholder names we assign before a real name is
/// known — never treated as a user-chosen identity for name-based merging.
fn is_placeholder(name: &str) -> bool {
    matches!(name.trim(), "" | "New friend" | "Friend")
}

/// Pure planner behind [`reconcile`]: given the raw friend list, return the
/// collapsed survivor list plus the `(from_id → into_id)` chat migrations needed.
///
/// Identity rules (so we NEVER duplicate a person and NEVER lose one):
///   * two records with the **same endpoint id** are the same device → merge;
///   * a record with **no** endpoint id whose **name** matches another record
///     (case-insensitive, non-placeholder) is the same friend reached by a
///     different path (e.g. a folder pairing vs. their permanent code) → merge.
/// The endpoint-keyed record always wins (it's the reachable one); otherwise the
/// older record wins. Merging is non-destructive: we keep the user-chosen name,
/// OR the auto-accept flags, carry the endpoint id forward, and migrate chat.
/// `chat_ids` is the set of friend ids that currently have a conversation. We use
/// it as a guard: a NAME-based merge (which could in theory be two different people
/// who happen to share a custom name) is refused when BOTH records already hold
/// chat history, so we can never fuse two real conversations. An endpoint-id match
/// is always safe to merge (it's provably the same device) regardless of chat.
pub fn plan_reconcile(
    input: &[Friend],
    chat_ids: &std::collections::HashSet<String>,
) -> (Vec<Friend>, Vec<(String, String)>) {
    let mut kept: Vec<Friend> = Vec::new();
    let mut merges: Vec<(String, String)> = Vec::new();
    for f in input {
        let found = kept.iter().position(|s| {
            let same_endpoint = match (s.endpoint_id.as_deref(), f.endpoint_id.as_deref()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let either_unkeyed = s.endpoint_id.is_none() || f.endpoint_id.is_none();
            let both_have_chat = chat_ids.contains(&s.id) && chat_ids.contains(&f.id);
            let name_match = either_unkeyed
                && !is_placeholder(&s.name)
                && s.name.trim().eq_ignore_ascii_case(f.name.trim())
                && !both_have_chat;
            same_endpoint || name_match
        });
        let Some(i) = found else {
            kept.push(f.clone());
            continue;
        };
        let s = kept[i].clone();
        // Survivor: the reachable (endpoint-keyed) record wins; if both or neither
        // are keyed, the older one wins (stable, preserves the earliest identity).
        let f_keyed = f.endpoint_id.is_some();
        let s_keyed = s.endpoint_id.is_some();
        let f_wins = if f_keyed != s_keyed {
            f_keyed
        } else {
            f.created_at < s.created_at
        };
        let (mut survivor, loser) = if f_wins { (f.clone(), s) } else { (s, f.clone()) };
        if is_placeholder(&survivor.name) && !is_placeholder(&loser.name) {
            survivor.name = loser.name.clone();
        }
        // A name the user explicitly chose (custom + non-placeholder) outranks a
        // non-custom name like a broadcast device name, regardless of which record
        // survives — otherwise an older non-custom record silently reverts the
        // rename held on the newer one, and the OR below then locks it in so no
        // future broadcast can repair it.
        if loser.name_custom && !survivor.name_custom && !is_placeholder(&loser.name) {
            survivor.name = loser.name.clone();
        }
        survivor.auto_accept = survivor.auto_accept || loser.auto_accept;
        survivor.name_custom = survivor.name_custom || loser.name_custom;
        if survivor.endpoint_id.is_none() {
            survivor.endpoint_id = loser.endpoint_id.clone();
        }
        if survivor.avatar.is_none() {
            survivor.avatar = loser.avatar.clone();
        }
        if survivor.id != loser.id {
            merges.push((loser.id.clone(), survivor.id.clone()));
        }
        kept[i] = survivor;
    }
    (kept, merges)
}

/// Collapse duplicate friend records into one canonical entry per person and
/// migrate any chat history onto the survivor. This is what makes friendships
/// permanent: no matter how many ways the same person was added (folder pairing,
/// permanent code, classic invite) across app updates, they end up as a single
/// friend with their full conversation intact. Returns how many records collapsed.
pub fn reconcile(config_dir: &Path) -> usize {
    let _guard = LOCK.lock().unwrap();
    let friends = load(config_dir);
    // Which friends currently hold a conversation — the safety guard for name-based
    // merges (never fuse two records that both already have chat history).
    let chat_ids: std::collections::HashSet<String> = crate::chat::overview(config_dir)
        .into_iter()
        .map(|o| o.peer_id)
        .collect();
    let (kept, merges) = plan_reconcile(&friends, &chat_ids);
    let removed = friends.len().saturating_sub(kept.len());
    if merges.is_empty() && removed == 0 {
        return 0;
    }
    // Apply migrations IN ORDER: a survivor that is itself later superseded chains
    // its (already-merged) history forward correctly only if applied sequentially.
    for (from, into) in &merges {
        crate::chat::merge_threads(config_dir, from, into);
    }
    let _ = save(config_dir, &kept);
    removed
}

/// The display name the local user has for the friend at `endpoint_id`, if any.
/// This is the user's own LABEL (whatever they renamed the friend to, or the name
/// the friend broadcast) — so every surface that shows a peer can prefer it over a
/// raw device name like "MacBook Air".
pub fn label_for_endpoint(config_dir: &Path, endpoint_id: &str) -> Option<String> {
    load(config_dir)
        .into_iter()
        .find(|f| f.endpoint_id.as_deref() == Some(endpoint_id))
        .map(|f| f.name)
        .filter(|n| !n.trim().is_empty())
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
        // Adopt the broadcast name unless the user locally renamed this friend.
        if !name.is_empty() && !f.name_custom && f.name != name {
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
        avatar: None,
        name_custom: false,
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

/// Self-heal the contact behind an incoming chat message so the conversation is
/// always visible AND replyable, even if the friend record was lost across an
/// update (GitHub #18/#19). Given the sender's cryptographic `endpoint_id`
/// (`conn.remote_id()`), an optional display `name`, and the `claimed_id` the
/// messages may already be keyed under, return a reachable [`Friend`] keyed to
/// the right id — recreating it if missing.
///
/// Dedup invariants (mirror `reconcile`, so we NEVER duplicate a person):
///   * a record with this **endpoint id** already exists → reuse it (refresh the
///     name if we have a better one and the user hasn't locally renamed them),
///     even if it's filed under a *different* record id than `claimed_id`;
///   * else a record with `claimed_id` exists (an invite-friend whose endpoint
///     wasn't keyed yet) → key it to this endpoint id so future dials work;
///   * else create a minimal reachable record. We reuse `claimed_id` as the new
///     record's id when present so it lines up with the thread the messages were
///     already stored under (no orphaned conversation); otherwise a fresh uuid.
/// The name falls back to "Unknown contact" so the row is never blank.
pub fn self_heal_chat_sender(
    config_dir: &Path,
    endpoint_id: &str,
    name: &str,
    claimed_id: Option<&str>,
) -> Option<Friend> {
    if endpoint_id.trim().is_empty() {
        return None;
    }
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    let name = name.trim();

    // 1) Already reachable under this endpoint id (anywhere in the list) → reuse.
    if let Some(f) = friends
        .iter_mut()
        .find(|f| f.endpoint_id.as_deref() == Some(endpoint_id))
    {
        if !name.is_empty() && !f.name_custom && f.name != name {
            f.name = name.to_string();
        }
        let out = f.clone();
        let _ = save(config_dir, &friends);
        return Some(out);
    }

    // 2) An invite-friend filed under the claimed id but not yet keyed → key it.
    if let Some(id) = claimed_id.filter(|id| !id.is_empty()) {
        if let Some(f) = friends.iter_mut().find(|f| f.id == id) {
            f.endpoint_id = Some(endpoint_id.to_string());
            if !name.is_empty() && !f.name_custom && f.name != name {
                f.name = name.to_string();
            }
            let out = f.clone();
            let _ = save(config_dir, &friends);
            return Some(out);
        }
    }

    // 2b) A real record for this person already exists but is endpoint-UNKEYED and
    //     was filed under a DIFFERENT id than `claimed_id` (the classic phantom case:
    //     a permanent-code / folder-pairing friend whose wire `friendId` is the
    //     SENDER's foreign per-device uuid, so steps 1 & 2 both miss). Adopt it by
    //     NAME and key it to this endpoint id instead of spawning a phantom. Mirrors
    //     plan_reconcile's name-merge rule (the `either_unkeyed && !is_placeholder &&
    //     name-match && !both_have_chat` invariant) and carries the SAME guards so two
    //     distinct people can never be fused: a real non-placeholder name, the
    //     candidate must be endpoint-UNKEYED (a record keyed to a different eid is a
    //     provably different device — never touched), the UNIQUE name match only
    //     (ambiguous → fall through to a fresh record), and refuse when BOTH the
    //     candidate and the claimed-id thread already hold chat history.
    if !name.is_empty() && !is_placeholder(name) {
        let matches: Vec<usize> = friends
            .iter()
            .enumerate()
            .filter(|(_, f)| f.endpoint_id.is_none() && f.name.trim().eq_ignore_ascii_case(name))
            .map(|(i, _)| i)
            .collect();
        if matches.len() == 1 {
            let i = matches[0];
            let cand_id = friends[i].id.clone();
            let claimed_has_chat = claimed_id
                .filter(|id| !id.is_empty())
                .map(|id| !crate::chat::messages(config_dir, id).is_empty())
                .unwrap_or(false);
            let cand_has_chat = !crate::chat::messages(config_dir, &cand_id).is_empty();
            if !(claimed_has_chat && cand_has_chat) {
                // Key the existing record to the sender's cryptographic endpoint id so
                // replies dial back AND future messages resolve by eid (step 1).
                friends[i].endpoint_id = Some(endpoint_id.to_string());
                if !friends[i].name_custom && friends[i].name != name {
                    friends[i].name = name.to_string();
                }
                let out = friends[i].clone();
                let _ = save(config_dir, &friends);
                // Fold any messages a PRIOR buggy build already stored under the
                // foreign claimed id onto this canonical record (idempotent union;
                // no-op when the claimed thread is empty — the common live case,
                // since resolution runs BEFORE chat::append).
                if let Some(id) = claimed_id.filter(|id| !id.is_empty() && *id != out.id) {
                    crate::chat::merge_threads(config_dir, id, &out.id);
                }
                return Some(out);
            }
        }
    }

    // 3) No record at all — recreate a minimal reachable one. Reuse the claimed id
    //    when we have it so the new friend lines up with the already-stored thread.
    let friend = Friend {
        id: claimed_id
            .filter(|id| !id.is_empty())
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        role: PairRole::B,
        name: clean_name(name, "Unknown contact"),
        secret: random_secret(),
        created_at: now_ms(),
        auto_accept: true,
        endpoint_id: Some(endpoint_id.to_string()),
        avatar: None,
        name_custom: false,
    };
    friends.push(friend.clone());
    let _ = save(config_dir, &friends);
    Some(friend)
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
                // Adopt the broadcast name unless the user has locally renamed this
                // friend — so a friend changing their own name propagates to you.
                if !name.trim().is_empty() && !f.name_custom && f.name != name.trim() {
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

/// Store the friend-at-`endpoint_id`'s profile picture path (received over the
/// wire and already saved to disk by the caller). Returns true if a record changed.
pub fn set_avatar_by_endpoint(config_dir: &Path, endpoint_id: &str, path: String) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    let mut changed = false;
    for f in friends.iter_mut().filter(|f| f.endpoint_id.as_deref() == Some(endpoint_id)) {
        if f.avatar.as_deref() != Some(path.as_str()) {
            f.avatar = Some(path.clone());
            changed = true;
        }
    }
    if changed {
        let _ = save(config_dir, &friends);
    }
    changed
}

pub fn rename(config_dir: &Path, id: &str, name: String) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut friends = load(config_dir);
    if let Some(f) = friends.iter_mut().find(|f| f.id == id) {
        if !name.trim().is_empty() {
            f.name = name.trim().to_string();
            // Mark as user-chosen so an incoming profile broadcast won't override it.
            f.name_custom = true;
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
    // allow_empty: removing your last friend legitimately writes `[]`.
    save_inner(config_dir, &friends, true)
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
            avatar: None,
            name_custom: false,
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

    fn f(id: &str, name: &str, eid: Option<&str>, created: u64) -> Friend {
        Friend {
            id: id.into(),
            role: PairRole::A,
            name: name.into(),
            secret: "s".into(),
            created_at: created,
            auto_accept: false,
            endpoint_id: eid.map(String::from),
            avatar: None,
            name_custom: false,
        }
    }

    fn none() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }
    fn ids(list: &[&str]) -> std::collections::HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reconcile_merges_same_endpoint() {
        // Same device dialed in twice under different record ids → one friend.
        let input = vec![
            f("1", "Bob", Some("EID"), 10),
            f("2", "Bob", Some("EID"), 20),
        ];
        let (kept, merges) = plan_reconcile(&input, &none());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "1"); // older record survives
        assert_eq!(merges, vec![("2".to_string(), "1".to_string())]);
    }

    #[test]
    fn reconcile_folds_unkeyed_into_keyed_by_name() {
        // A folder pairing made an unkeyed "Bob"; later his permanent code made a
        // keyed "Bob". They're the same person → collapse onto the reachable one,
        // migrating the unkeyed record's chat history forward.
        let input = vec![
            f("folder", "Bob", None, 5),
            f("keyed", "bob", Some("EID"), 50),
        ];
        let (kept, merges) = plan_reconcile(&input, &none());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "keyed"); // endpoint-keyed (reachable) survives
        assert_eq!(kept[0].endpoint_id.as_deref(), Some("EID"));
        assert_eq!(merges, vec![("folder".to_string(), "keyed".to_string())]);
    }

    #[test]
    fn reconcile_keeps_user_chosen_name_and_auto_accept() {
        // Same device twice: the older record still carries the placeholder name,
        // the newer one learned the real name and had auto-accept on. The merged
        // friend keeps the real name and the enabled flag.
        let mut older = f("older", "Friend", Some("EID"), 5); // placeholder, survives
        older.auto_accept = false;
        let mut newer = f("newer", "Bob the Builder", Some("EID"), 50);
        newer.auto_accept = true;
        let (kept, merges) = plan_reconcile(&[older, newer], &none());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "older");
        assert_eq!(kept[0].name, "Bob the Builder"); // adopt the real name
        assert!(kept[0].auto_accept); // OR of the two
        assert_eq!(merges, vec![("newer".to_string(), "older".to_string())]);
    }

    #[test]
    fn reconcile_does_not_merge_distinct_placeholders() {
        // Two different unknown people, both still named the generic placeholder —
        // never merge on a placeholder name.
        let input = vec![f("a", "Friend", None, 1), f("b", "Friend", None, 2)];
        let (kept, merges) = plan_reconcile(&input, &none());
        assert_eq!(kept.len(), 2);
        assert!(merges.is_empty());
    }

    #[test]
    fn reconcile_noop_when_already_canonical() {
        let input = vec![
            f("1", "Bob", Some("E1"), 1),
            f("2", "Carol", Some("E2"), 2),
            f("3", "Dave", None, 3),
        ];
        let (kept, merges) = plan_reconcile(&input, &none());
        assert_eq!(kept.len(), 3);
        assert!(merges.is_empty());
    }

    #[test]
    fn reconcile_chains_superseded_survivor() {
        // unkeyed "Bob" (a) is folded into keyed "Bob" (b); both keyed records then
        // collapse. History must chain a→b then b→… correctly via ordered merges.
        let input = vec![
            f("a", "Bob", None, 1),
            f("b", "Bob", Some("EID"), 2),
            f("c", "Bob", Some("EID"), 3),
        ];
        let (kept, merges) = plan_reconcile(&input, &none());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "b");
        assert_eq!(
            merges,
            vec![
                ("a".to_string(), "b".to_string()),
                ("c".to_string(), "b".to_string()),
            ]
        );
    }

    #[test]
    fn reconcile_refuses_name_merge_when_both_have_chat() {
        // Two records that share a name but BOTH already hold a conversation — could
        // be two different people. Never fuse their chats: keep both.
        let input = vec![f("u", "Sam", None, 1), f("k", "Sam", Some("EID"), 2)];
        let (kept, merges) = plan_reconcile(&input, &ids(&["u", "k"]));
        assert_eq!(kept.len(), 2);
        assert!(merges.is_empty());
    }

    #[test]
    fn reconcile_same_endpoint_merges_even_with_chat_on_both() {
        // Same device (same endpoint id) is provably the same person — merge and
        // union the conversation even if both records have chat.
        let input = vec![f("a", "Sam", Some("EID"), 1), f("b", "Sam", Some("EID"), 2)];
        let (kept, merges) = plan_reconcile(&input, &ids(&["a", "b"]));
        assert_eq!(kept.len(), 1);
        assert_eq!(merges, vec![("b".to_string(), "a".to_string())]);
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("db-friends-test-{tag}-{}", now_ms()))
    }

    #[test]
    fn self_heal_recreates_lost_contact_keyed_to_thread() {
        // The "lost my friend's contact after update" case: friends.json is empty
        // but a chat thread exists under the claimed id "thread1". A message lands
        // → recreate a reachable friend filed under that same id so the stored
        // conversation isn't orphaned, with the sender's endpoint id + name.
        let dir = tmp("recreate");
        let f = self_heal_chat_sender(&dir, "EIDX", "Mong", Some("thread1")).unwrap();
        assert_eq!(f.id, "thread1"); // lines up with the existing thread
        assert_eq!(f.name, "Mong");
        assert_eq!(f.endpoint_id.as_deref(), Some("EIDX"));
        assert_eq!(load(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_blank_name_falls_back() {
        // No display name on the wire → a non-blank fallback so the chat row is
        // never empty/invisible.
        let dir = tmp("fallback");
        let f = self_heal_chat_sender(&dir, "EIDY", "", None).unwrap();
        assert_eq!(f.name, "Unknown contact");
        assert_eq!(f.endpoint_id.as_deref(), Some("EIDY"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_dedups_by_endpoint_no_duplicate() {
        // A record for this endpoint id already exists (under a DIFFERENT record id
        // than the claimed one) → reuse it, never add a second. This preserves the
        // reconcile invariant that endpoint id IS the identity.
        let dir = tmp("dedup");
        let _ = save(&dir, &[f("orig", "Mong", Some("EIDZ"), 5)]);
        let healed = self_heal_chat_sender(&dir, "EIDZ", "Mong", Some("different-claim")).unwrap();
        assert_eq!(healed.id, "orig"); // reused the existing record, not a new one
        assert_eq!(load(&dir).len(), 1); // NO duplicate
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_keys_unkeyed_invite_friend() {
        // An invite-friend exists under the claimed id but has no endpoint id yet
        // (so replies couldn't dial back) → learn the endpoint id in place.
        let dir = tmp("keyinvite");
        let _ = save(&dir, &[f("invite1", "Mong", None, 5)]);
        let healed = self_heal_chat_sender(&dir, "NEWEID", "Mong", Some("invite1")).unwrap();
        assert_eq!(healed.id, "invite1");
        assert_eq!(healed.endpoint_id.as_deref(), Some("NEWEID"));
        assert_eq!(load(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_keeps_user_renamed_label() {
        // The user locally renamed this friend (name_custom) → an incoming
        // broadcast name must NOT overwrite their chosen label.
        let dir = tmp("renamed");
        let mut custom = f("c", "My Bestie", Some("EIDR"), 5);
        custom.name_custom = true;
        let _ = save(&dir, &[custom]);
        let healed = self_heal_chat_sender(&dir, "EIDR", "RawDeviceName", None).unwrap();
        assert_eq!(healed.name, "My Bestie");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Seed a received chat message under `peer` so the both-have-chat guard can be
    // exercised (a real conversation already exists under that record id).
    fn seed_msg(dir: &Path, peer: &str) {
        let _ = crate::chat::append(
            dir,
            &crate::chat::ChatMessage {
                id: format!("m-{peer}"),
                peer_id: peer.into(),
                from_me: false,
                kind: "text".into(),
                text: "hi".into(),
                files: vec![],
                bytes: 0,
                path: None,
                status: None,
                ts: 1,
                seq: 1,
                reply_to: None,
                reply_preview: None,
                reactions: vec![],
                edited: false,
                deleted: false,
                gif: None,
            },
        );
    }

    #[test]
    fn self_heal_adopts_unkeyed_name_match_no_phantom() {
        // #19/#18: an inbound message from a permanent-code friend whose wire friendId
        // is a FOREIGN per-device uuid must ADOPT the existing endpoint-unkeyed record
        // (the one the open ChatView is subscribed to), NOT spawn a phantom.
        let dir = tmp("adopt");
        let _ = save(&dir, &[f("local-uuid", "Mong", None, 5)]);
        let healed = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        assert_eq!(healed.id, "local-uuid"); // reused the existing record
        assert_eq!(healed.endpoint_id.as_deref(), Some("EID")); // …now keyed
        assert_eq!(load(&dir).len(), 1); // NO phantom
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_does_not_adopt_when_two_share_name() {
        // Ambiguous: two endpoint-unkeyed records share the name → never guess; fall
        // through to a fresh record rather than fuse the wrong one.
        let dir = tmp("ambiguous");
        let _ = save(&dir, &[f("u1", "Mong", None, 5), f("u2", "Mong", None, 6)]);
        let healed = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        assert!(healed.id != "u1" && healed.id != "u2"); // a new record
        assert_eq!(load(&dir).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_does_not_adopt_record_keyed_to_other_eid() {
        // A record already keyed to a DIFFERENT endpoint id is a provably different
        // device — never adopt it by name.
        let dir = tmp("otherkey");
        let _ = save(&dir, &[f("x", "Mong", Some("OTHER_EID"), 5)]);
        let healed = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        assert!(healed.id != "x"); // a new record, the OTHER_EID one untouched
        assert_eq!(load(&dir).len(), 2);
        let other = load(&dir).into_iter().find(|fr| fr.id == "x").unwrap();
        assert_eq!(other.endpoint_id.as_deref(), Some("OTHER_EID"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_refuses_name_adopt_when_both_have_chat() {
        // Two REAL conversations (one under the unkeyed candidate, one under the
        // claimed id) must never be fused — mirrors reconcile_refuses_name_merge.
        let dir = tmp("bothchat");
        let _ = save(&dir, &[f("cand", "Mong", None, 5)]);
        seed_msg(&dir, "cand");
        seed_msg(&dir, "foreign-uuid");
        let healed = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        assert!(healed.id != "cand"); // refused adoption → fresh record
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_heal_name_adopt_is_idempotent() {
        // After the first adoption keys the record, a second inbound resolves by
        // endpoint id (step 1) — same record, no new writes, no duplicate.
        let dir = tmp("adopt-idem");
        let _ = save(&dir, &[f("local-uuid", "Mong", None, 5)]);
        let first = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        let second = self_heal_chat_sender(&dir, "EID", "Mong", Some("foreign-uuid")).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(load(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_refuses_to_clobber_with_empty_list() {
        // The persistence hardening: a stray empty list must never wipe a populated
        // friends.json (the durable form of "lost my contact"). save() refuses it…
        let dir = tmp("clobber");
        let _ = save(&dir, &[f("keep", "Mong", Some("EID1"), 1)]);
        assert!(save(&dir, &[]).is_err());
        assert_eq!(load(&dir).len(), 1); // still there
        // …but a legitimate "remove last friend" (allow_empty) does empty it.
        assert!(save_inner(&dir, &[], true).is_ok());
        assert_eq!(load(&dir).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_recovers_from_backup_when_primary_corrupt() {
        // The exact "lost my contacts after an update" wipe: a populated friends.json,
        // then a transient corrupt/partial primary read. load() must recover the real
        // records from the .bak that save keeps — NOT return [] (which would let a
        // friend-hello save a shrunken 1-record list and wipe the rest).
        let dir = tmp("bakrecover");
        let _ = std::fs::create_dir_all(&dir);
        save(
            &dir,
            &[
                f("a", "Ann", Some("EIDA"), 1),
                f("b", "Bob", Some("EIDB"), 2),
                f("c", "Cy", Some("EIDC"), 3),
            ],
        )
        .unwrap();
        // Corrupt the primary (truncated JSON). The .bak still holds the good 3.
        std::fs::write(friends_path(&dir), b"[{\"id\":\"a\",").unwrap();
        assert_eq!(load(&dir).len(), 3, "must recover all 3 from .bak, not wipe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_clobber_guard_holds_when_primary_unreadable() {
        // save(allow_empty=false) must still refuse to write [] when the .bak holds
        // real records even though the primary is momentarily corrupt.
        let dir = tmp("noclobber-corrupt");
        let _ = std::fs::create_dir_all(&dir);
        save(&dir, &[f("a", "Ann", Some("EIDA"), 1)]).unwrap();
        std::fs::write(friends_path(&dir), b"not json at all").unwrap();
        assert!(save(&dir, &[]).is_err(), "must refuse to clobber");
        assert_eq!(load(&dir).len(), 1, "the one real record survives via .bak");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_last_friend_is_not_resurrected_by_backup() {
        // A legitimate "remove my last friend" must stick even if the primary later
        // corrupts — the .bak tracks the legit empty, so no zombie contact returns.
        let dir = tmp("noresurrect");
        let _ = std::fs::create_dir_all(&dir);
        save(&dir, &[f("a", "Ann", Some("EIDA"), 1)]).unwrap();
        remove(&dir, "a").unwrap();
        assert_eq!(load(&dir).len(), 0, "removed friend stays gone");
        // Now corrupt the primary; recovery must NOT bring back the removed contact.
        std::fs::write(friends_path(&dir), b"garbage").unwrap();
        assert_eq!(load(&dir).len(), 0, "no resurrection from a stale .bak");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
