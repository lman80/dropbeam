//! Blocking (App Store guideline 1.2). Blocking a person removes them as a
//! friend on every own device AND keeps them out: their hello no longer adds
//! them back, and the engine answers their chat, typing/read signals, file
//! pushes, folder invites and Location requests exactly as it answers a
//! stranger (see `iroh_net::serve_blocked`) — they are never told they're
//! blocked.
//!
//! One record per endpoint id in `blocked.json`, last writer wins by `at`
//! (Lamport-style: a change is stamped one past the newest stamp it saw), so an
//! unblock is a tombstone (`blocked: false`) that syncs like a block. The list
//! rides account sync (`account::Meta::blocked`) to the user's own devices only;
//! it is never sent to anyone else. A person is blocked by every device of theirs
//! we know (grouped by their verified account), and the account is remembered so
//! a device of theirs we meet later is blocked the moment it proves the account.

use std::{collections::HashMap, path::Path, sync::Mutex};

use serde::{Deserialize, Serialize};

use crate::models::Friend;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub(crate) struct BlockRec {
    /// Their name when blocked (for the Blocked list; never sent to them).
    #[serde(default)]
    pub name: String,
    /// Their verified account key, when known: blocks their other devices too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// False = unblocked (a tombstone, so the unblock reaches the other devices).
    #[serde(default)]
    pub blocked: bool,
    /// When this was decided (ms, Lamport-stamped).
    #[serde(default)]
    pub at: u64,
}

/// One blocked person as the UI lists them (their devices folded together).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlockedPerson {
    /// Stable key for unblocking: the smallest endpoint id of the person.
    pub id: String,
    pub name: String,
    pub at: u64,
    pub endpoint_ids: Vec<String>,
}

fn path(dir: &Path) -> std::path::PathBuf {
    dir.join("blocked.json")
}

pub(crate) fn snapshot(dir: &Path) -> HashMap<String, BlockRec> {
    std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write(dir: &Path, all: &HashMap<String, BlockRec>) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(dir);
    crate::settings::write_atomic(&path(dir), &bytes).map_err(|e| e.to_string())
}

fn with<T>(dir: &Path, f: impl FnOnce(&mut HashMap<String, BlockRec>) -> T) -> Result<T, String> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut all = snapshot(dir);
    let before = all.clone();
    let out = f(&mut all);
    if all != before {
        write(dir, &all)?;
        crate::account::note_change();
    }
    Ok(out)
}

fn stamp(prev: Option<&BlockRec>) -> u64 {
    crate::chat::now_ms().max(prev.map_or(0, |r| r.at.saturating_add(1)))
}

/// Is the peer at `eid` blocked (directly, or as a device of a blocked account)?
pub(crate) fn is_blocked(dir: &Path, eid: &str) -> bool {
    if eid.is_empty() {
        return false;
    }
    let all = snapshot(dir);
    if all.get(eid).is_some_and(|r| r.blocked) {
        return true;
    }
    // A device we still hold a record for, whose verified account is blocked.
    let accounts: Vec<&str> = all.values().filter(|r| r.blocked).filter_map(|r| r.account.as_deref()).collect();
    !accounts.is_empty()
        && crate::friends::load_raw(dir).iter().any(|f| {
            f.endpoint_id.as_deref() == Some(eid) && f.account_pub.as_deref().is_some_and(|a| accounts.contains(&a))
        })
}

/// A verified account key belongs to a blocked person.
pub(crate) fn account_blocked(dir: &Path, account: &str) -> bool {
    snapshot(dir).values().any(|r| r.blocked && r.account.as_deref() == Some(account))
}

/// A hello from a device we don't know yet: if it proves (signature-verified)
/// the account of a blocked person, block this device too and say so.
pub(crate) fn adopt_from_hello(dir: &Path, eid: &str, req: &serde_json::Value) -> bool {
    let Some(key) = req["account_pub"].as_str().filter(|k| {
        crate::link::verify_account(k, req["account_sig"].as_str().unwrap_or(""), eid)
    }) else {
        return false;
    };
    if crate::account::my_pub(dir).as_deref() == Some(key) || !account_blocked(dir, key) {
        return false;
    }
    let name = snapshot(dir).values().find(|r| r.blocked && r.account.as_deref() == Some(key))
        .map(|r| r.name.clone()).unwrap_or_default();
    let _ = with(dir, |all| {
        let at = stamp(all.get(eid));
        all.insert(eid.to_owned(), BlockRec { name, account: Some(key.to_owned()), blocked: true, at });
    });
    remove_friend_records(dir, &[eid.to_owned()]);
    true
}

/// Block the person behind friend `friend_id`: every device of theirs we know
/// (their verified account groups them) is blocked and removed as a friend on
/// all own devices. Returns the endpoint ids blocked. Own devices can't be blocked.
pub(crate) fn block_friend(dir: &Path, friend_id: &str) -> Result<Vec<String>, String> {
    let friend = crate::friends::get(dir, friend_id).ok_or("That friend no longer exists.")?;
    let owner = crate::friends::thread_owner(dir, &friend.id).unwrap_or_else(|| friend.clone());
    let mine = crate::account::my_pub(dir);
    if friend.account_pub.is_some() && friend.account_pub == mine {
        return Err("That's one of your own devices. Remove it from Devices instead.".into());
    }
    let mut eids = crate::friends::person_endpoints(dir, &owner.id);
    if let Some(e) = friend.endpoint_id.clone() {
        if !eids.contains(&e) { eids.push(e); }
    }
    eids.retain(|e| !crate::account::is_own_device(dir, e));
    let account = owner.account_pub.clone().or(friend.account_pub.clone()).filter(|a| Some(a) != mine.as_ref());
    let name = owner.name.clone();
    if !eids.is_empty() {
        with(dir, |all| {
            for e in &eids {
                let at = stamp(all.get(e));
                all.insert(e.clone(), BlockRec { name: name.clone(), account: account.clone(), blocked: true, at });
            }
        })?;
    }
    // Remove every record of the person (an invite-only record without an
    // endpoint id is removed too — it's the same person).
    let records: Vec<Friend> = crate::friends::load_raw(dir).into_iter()
        .filter(|f| f.id == friend.id || f.endpoint_id.as_ref().is_some_and(|e| eids.contains(e)))
        .collect();
    for f in records {
        crate::account::record_friend_removed(dir, &f);
        crate::friends::remove(dir, &f.id)?;
    }
    Ok(eids)
}

fn remove_friend_records(dir: &Path, eids: &[String]) {
    for f in crate::friends::load_raw(dir) {
        if f.endpoint_id.as_ref().is_some_and(|e| eids.contains(e)) {
            crate::account::record_friend_removed(dir, &f);
            let _ = crate::friends::remove(dir, &f.id);
        }
    }
}

/// Unblock the person listed under `id` (any of their endpoint ids): every
/// device of theirs becomes a stranger again. They are NOT re-added as a
/// friend; the user adds them again with their code if they want to.
pub(crate) fn unblock(dir: &Path, id: &str) -> Result<(), String> {
    with(dir, |all| {
        let account = all.get(id).and_then(|r| r.account.clone());
        let keys: Vec<String> = all.iter()
            .filter(|(e, r)| r.blocked && (e.as_str() == id || (account.is_some() && r.account == account)))
            .map(|(e, _)| e.clone()).collect();
        for e in keys {
            let at = stamp(all.get(&e));
            if let Some(r) = all.get_mut(&e) {
                r.blocked = false;
                r.at = at;
            }
        }
    })
}

/// The Blocked list, one row per person, newest first.
pub fn list(dir: &Path) -> Vec<BlockedPerson> {
    let mut people: HashMap<String, BlockedPerson> = HashMap::new();
    for (eid, r) in snapshot(dir).into_iter().filter(|(_, r)| r.blocked) {
        let key = r.account.clone().unwrap_or_else(|| eid.clone());
        let p = people.entry(key).or_insert_with(|| BlockedPerson { id: eid.clone(), name: r.name.clone(), at: r.at, endpoint_ids: vec![] });
        p.endpoint_ids.push(eid.clone());
        p.endpoint_ids.sort();
        p.id = p.endpoint_ids[0].clone();
        if p.name.trim().is_empty() { p.name = r.name.clone(); }
        p.at = p.at.max(r.at);
    }
    let mut out: Vec<BlockedPerson> = people.into_values().map(|mut p| {
        if p.name.trim().is_empty() { p.name = "Unknown".into(); }
        p
    }).collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
    out
}

/// Merge another own device's list (newest decision per endpoint wins; a tie
/// keeps the block). True if anything changed here.
pub(crate) fn merge(dir: &Path, theirs: &HashMap<String, BlockRec>, clamp: impl Fn(u64) -> u64) -> bool {
    if theirs.is_empty() {
        return false;
    }
    with(dir, |all| {
        let mut changed = false;
        for (eid, r) in theirs {
            if eid.is_empty() || eid.len() > 128 {
                continue;
            }
            let mut r = r.clone();
            r.at = clamp(r.at);
            r.name = r.name.chars().take(200).collect();
            let wins = match all.get(eid) {
                None => true,
                Some(mine) => r.at > mine.at || (r.at == mine.at && r.blocked && !mine.blocked),
            };
            if wins && all.get(eid) != Some(&r) {
                all.insert(eid.clone(), r);
                changed = true;
            }
        }
        changed
    }).unwrap_or(false)
}

/// Drop every friend record of a blocked person (after a merge). True if any.
pub(crate) fn purge_blocked_friends(dir: &Path) -> bool {
    let blocked: Vec<String> = crate::friends::load_raw(dir).into_iter()
        .filter_map(|f| f.endpoint_id)
        .filter(|e| is_blocked(dir, e))
        .collect();
    if blocked.is_empty() {
        return false;
    }
    remove_friend_records(dir, &blocked);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("db-block-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn eid() -> String {
        iroh::SecretKey::generate().public().to_string()
    }

    #[test]
    fn blocking_removes_the_friend_and_their_hello_cannot_re_add_them() {
        let d = dir();
        let e = eid();
        let f = crate::friends::upsert_by_endpoint(&d, &e, "Spammer");
        assert_eq!(block_friend(&d, &f.id).unwrap(), vec![e.clone()]);
        assert!(is_blocked(&d, &e));
        assert!(crate::friends::load(&d).is_empty());
        // No account needed: a hello, a chat and a self-heal all stay out.
        crate::friends::apply_hello(&d, "", &e, "Spammer");
        crate::friends::apply_hello(&d, &f.id, &e, "Spammer");
        assert!(crate::friends::load(&d).is_empty());
        assert!(crate::friends::chat_sender(&d, &e).is_none());
        assert!(crate::friends::self_heal_chat_sender(&d, &e, "Spammer", Some(&f.id)).is_none());
        assert!(crate::friends::add_by_code(&d, &crate::friends::my_code("Spammer", &e)).is_err());
        let listed = list(&d);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].name.as_str(), listed[0].endpoint_ids.clone()), ("Spammer", vec![e.clone()]));
        // Unblocking makes them a stranger again (not a friend); adding works.
        unblock(&d, &e).unwrap();
        assert!(!is_blocked(&d, &e) && list(&d).is_empty());
        assert!(crate::friends::load(&d).is_empty());
        crate::friends::add_by_code(&d, &crate::friends::my_code("Spammer", &e)).unwrap();
        assert_eq!(crate::friends::load(&d).len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn blocking_a_person_blocks_all_their_known_devices_and_future_ones() {
        let d = dir();
        let (mac, phone) = (eid(), eid());
        let a = crate::friends::upsert_by_endpoint(&d, &mac, "Mong");
        crate::friends::upsert_by_endpoint(&d, &phone, "Mong's iPhone");
        crate::friends::set_device_info(&d, &mac, Some("laptop"), Some("acct-mong"));
        crate::friends::set_device_info(&d, &phone, Some("phone"), Some("acct-mong"));
        let mut blocked = block_friend(&d, &a.id).unwrap();
        blocked.sort();
        let mut want = vec![mac.clone(), phone.clone()];
        want.sort();
        assert_eq!(blocked, want);
        assert!(crate::friends::load(&d).is_empty());
        assert_eq!(list(&d).len(), 1, "one person, one row");
        assert!(account_blocked(&d, "acct-mong"));
        // Unblocking the person (by either id) unblocks every device.
        unblock(&d, &phone).unwrap();
        assert!(!is_blocked(&d, &mac) && !is_blocked(&d, &phone));
        let _ = std::fs::remove_dir_all(d);
    }

    /// A block made on one own device reaches the others over account sync
    /// (and removes the person there); an unblock travels back the same way.
    #[tokio::test]
    async fn a_block_syncs_to_the_users_other_devices_and_so_does_the_unblock() {
        use crate::account::testkit::*;
        let k = iroh::SecretKey::generate();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let spam = eid();
        let f = crate::friends::upsert_by_endpoint(&a.dir, &spam, "Spam");
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(b.friend(&spam).is_some(), "the friend synced first");
        std::thread::sleep(std::time::Duration::from_millis(5));
        block_friend(&a.dir, &f.id).unwrap();
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(is_blocked(&b.dir, &spam), "the block reached the iPhone");
        assert!(b.friend(&spam).is_none() && a.friend(&spam).is_none());
        assert_eq!(list(&b.dir).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Spam"]);
        // Their hello on the other device doesn't bring them back either.
        crate::friends::apply_hello(&b.dir, "", &spam, "Spam");
        assert!(b.friend(&spam).is_none());
        // Nothing left to move: the next round is quiet and changes nothing.
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(is_blocked(&a.dir, &spam) && is_blocked(&b.dir, &spam));
        // Unblock on the iPhone; the Mac learns it. Unblocking doesn't re-add them.
        std::thread::sleep(std::time::Duration::from_millis(5));
        unblock(&b.dir, &spam).unwrap();
        assert!(sync(&b, &a).await.client.is_ok());
        assert!(!is_blocked(&a.dir, &spam) && list(&a.dir).is_empty());
        assert!(a.friend(&spam).is_none() && b.friend(&spam).is_none());
    }

    #[test]
    fn merge_is_newest_wins_and_an_unblock_tombstone_travels() {
        let (a, b) = (dir(), dir());
        let e = eid();
        let f = crate::friends::upsert_by_endpoint(&a, &e, "X");
        crate::friends::upsert_by_endpoint(&b, &e, "X");
        block_friend(&a, &f.id).unwrap();
        assert!(merge(&b, &snapshot(&a), |t| t));
        assert!(purge_blocked_friends(&b));
        assert!(is_blocked(&b, &e) && crate::friends::load(&b).is_empty());
        std::thread::sleep(std::time::Duration::from_millis(3));
        unblock(&b, &e).unwrap();
        // A's older block doesn't undo B's newer unblock, and the unblock reaches A.
        assert!(!merge(&b, &snapshot(&a), |t| t));
        assert!(!is_blocked(&b, &e));
        merge(&a, &snapshot(&b), |t| t);
        assert!(!is_blocked(&a, &e));
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
