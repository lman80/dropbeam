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
}
