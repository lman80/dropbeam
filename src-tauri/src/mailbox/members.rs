//! "All my friends" for a Transfer Server = the OWNER's friends, on any of the
//! owner's devices — not just the few people the server device itself met.
//!
//! The server device (say a Linux box) usually isn't friends with everyone its
//! owner talks to. So:
//!
//! 1. The server knows its owner: the account it belongs to (when it is linked),
//!    or the account the owner picked on the server (`ServerConfig::owner_account`).
//!    A device proves that account with the signature in its friend-hello
//!    (`friends::apply_device_hello`), never by saying so.
//! 2. Each owner device that agreed to share the server ("Share with friends")
//!    sends it the owner's current friends — `mailbox.members`, on an
//!    authenticated connection, refreshed while things change. The server keeps
//!    one list per owner device and trusts only lists from owner devices.
//! 3. Owner devices tell their friends about the server in their hello
//!    (`shared`); a friend's device then asks the server what it may do there
//!    (`mailbox.hello`) and only shows the offer when the server says yes.
//!
//! Removal is as sticky as it is on the owner's devices: every list carries the
//! owner's removals and blocks (endpoint id → when), and a person listed on one
//! device but removed on another after they were added is out.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::server::{ServerConfig, DAY_MS};

/// A list nobody refreshed for this long no longer counts (the device that sent
/// it may be gone for good).
const LIST_TTL_MS: u64 = 60 * DAY_MS;
const MAX_PEOPLE: usize = 4000;
const MAX_DEVICES: usize = 16;
const MAX_REMOVED: usize = 8000;
/// At most this many owner devices keep a list here.
const MAX_LISTS: usize = 16;

/// One device of one of the owner's friends.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub eid: String,
    /// When the owner added this device as a friend (ms, the owner's clock).
    #[serde(default)]
    pub since: u64,
}

/// One of the owner's friends (all their devices).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Person {
    /// Stable across the owner's devices: the smallest endpoint id of the person.
    pub key: String,
    pub name: String,
    pub devices: Vec<Device>,
}

/// What one owner device last said.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerList {
    /// When this server received it (its own clock).
    pub at: u64,
    pub people: Vec<Person>,
    /// Friends the owner removed or blocked: endpoint id → when.
    #[serde(default)]
    pub removed: HashMap<String, u64>,
    /// The owner's account roster (link / removal times), so a device the owner
    /// removed from the account stops counting as the owner here too.
    #[serde(default)]
    pub linked: HashMap<String, u64>,
    #[serde(default)]
    pub unlinked: HashMap<String, u64>,
}

type Lists = HashMap<String, OwnerList>;

static CACHE: Mutex<Option<HashMap<PathBuf, Lists>>> = Mutex::new(None);

fn path(config: &Path) -> PathBuf {
    config.join("transfer-server-members.json")
}

fn load(config: &Path) -> Lists {
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if let Some(l) = m.get(config) {
        return l.clone();
    }
    let l: Lists = match crate::settings::read_json_store(&path(config)) {
        crate::settings::StoreRead::Loaded(l) => l,
        _ => Lists::new(),
    };
    m.insert(config.to_path_buf(), l.clone());
    l
}

fn save(config: &Path, lists: &Lists) {
    if let Ok(bytes) = serde_json::to_vec(lists) {
        if let Err(e) = crate::settings::write_atomic(&path(config), &bytes) {
            log::warn!("transfer-server: cannot save the member lists: {e}");
        }
    }
    CACHE.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(config.to_path_buf(), lists.clone());
    forget_view(config);
}

/// Forget every list (the owner changed, or the server was switched off).
pub fn clear(config: &Path) {
    save(config, &Lists::new());
}

// ── who owns this server ────────────────────────────────────────────────────

/// The account that owns this server: its own account when it is linked to one,
/// else the one picked on this device.
pub fn owner_account(config: &Path, c: &ServerConfig) -> Option<String> {
    crate::account::my_pub(config).or_else(|| Some(c.owner_account.trim().to_owned()).filter(|a| !a.is_empty()))
}

/// Devices that proved `account` in their hello (the signed `account_sig`) and
/// aren't blocked here.
fn proving(config: &Path, account: &str) -> HashSet<String> {
    crate::friends::load(config).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account))
        .filter_map(|f| f.endpoint_id)
        .filter(|e| !crate::block::is_blocked(config, e))
        .collect()
}

/// A person the owner vouched for, as the server shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vouched {
    /// "v:<key>" — the usage/denial bucket on this server.
    pub person: String,
    pub name: String,
}

pub fn person_id(key: &str) -> String {
    format!("v:{key}")
}

/// Everything membership questions need, computed once and cached briefly
/// (rights are asked per recipient, per item, per hello).
struct View {
    made: Instant,
    account: Option<String>,
    /// Owner devices (for a server that isn't linked; linked servers use the
    /// account roster directly).
    owners: HashSet<String>,
    /// Vouched device → who they are.
    index: HashMap<String, Vouched>,
    /// Lists that count (from owner devices, not stale).
    live: usize,
}

static VIEWS: Mutex<Option<HashMap<PathBuf, Arc<View>>>> = Mutex::new(None);
const VIEW_TTL: Duration = Duration::from_secs(3);

fn forget_view(config: &Path) {
    if let Some(m) = VIEWS.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        m.remove(config);
    }
}

fn view(config: &Path, c: &ServerConfig) -> Arc<View> {
    let account = owner_account(config, c);
    if let Some(v) = VIEWS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(config)) {
        if v.made.elapsed() < VIEW_TTL && v.account == account {
            return v.clone();
        }
    }
    let v = Arc::new(compute(config, account));
    VIEWS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(config.to_path_buf(), v.clone());
    v
}

fn compute(config: &Path, account: Option<String>) -> View {
    let empty = |account| View { made: Instant::now(), account, owners: HashSet::new(), index: HashMap::new(), live: 0 };
    let Some(acct) = account.clone() else { return empty(account) };
    let linked_here = crate::account::my_pub(config).is_some();
    let now = crate::chat::now_ms();
    let lists: Vec<(String, OwnerList)> = load(config).into_iter()
        .filter(|(_, l)| now.saturating_sub(l.at) < LIST_TTL_MS)
        .collect();
    let owners: HashSet<String> = if linked_here {
        crate::account::own_devices(config).into_iter().filter_map(|f| f.endpoint_id).collect()
    } else {
        // A device the owner removed from the account still holds the key, so
        // it still proves the account; the owner's other devices report the
        // removal. A report only counts from a device nobody reported in turn:
        // two devices reporting each other both stay (the server can't tell
        // which one is lying), and nobody can lock the real devices out.
        let prove = proving(config, &acct);
        let fresh: Vec<&(String, OwnerList)> = lists.iter().filter(|(from, _)| prove.contains(from)).collect();
        let linked_at = |d: &str| fresh.iter().filter_map(|(_, l)| l.linked.get(d).copied()).max().unwrap_or(0);
        let reporters = |d: &str| -> Vec<&str> {
            let at = linked_at(d);
            fresh.iter()
                .filter(|(from, l)| from != d && l.unlinked.get(d).is_some_and(|t| *t > 0 && *t >= at))
                .map(|(from, _)| from.as_str())
                .collect()
        };
        prove.iter()
            .filter(|d| !reporters(d).iter().any(|r| reporters(r).is_empty()))
            .cloned()
            .collect()
    };
    let live: Vec<(String, OwnerList)> = lists.into_iter().filter(|(from, _)| owners.contains(from)).collect();
    View { made: Instant::now(), account, index: index(&live), live: live.len(), owners }
}

/// One of the owner's devices: this device's own account devices (a linked
/// server), or — for a server that isn't linked — a device proving the owner's
/// account that no (undisputed) owner device reported removed from it.
pub fn is_owner_device(config: &Path, c: &ServerConfig, eid: &str) -> bool {
    if crate::account::is_own_device(config, eid) {
        return true;
    }
    if crate::account::my_pub(config).is_some() {
        return false;
    }
    view(config, c).owners.contains(eid)
}

/// Every vouched device → who they are, from the lists that count.
fn index(lists: &[(String, OwnerList)]) -> HashMap<String, Vouched> {
    let mut removed: HashMap<&str, u64> = HashMap::new();
    for (_, l) in lists {
        for (e, t) in &l.removed {
            let x = removed.entry(e.as_str()).or_default();
            *x = (*x).max(*t);
        }
    }
    let mut best: HashMap<&str, (u64, u64, &Person)> = HashMap::new(); // (since, list at, person)
    for (_, l) in lists {
        for p in &l.people {
            for d in &p.devices {
                let cur = best.get(d.eid.as_str());
                if cur.is_none_or(|(s, at, _)| (d.since, l.at) > (*s, *at)) {
                    best.insert(d.eid.as_str(), (d.since, l.at, p));
                }
            }
        }
    }
    best.into_iter()
        // Removed (or blocked) on some owner device after they were added.
        .filter(|(eid, (since, _, _))| removed.get(eid).is_none_or(|r| r < since))
        .map(|(eid, (_, _, p))| (eid.to_owned(), Vouched { person: person_id(&p.key), name: p.name.clone() }))
        .collect()
}

/// Is `eid` a device of one of the owner's friends (vouched by an owner device)?
pub fn vouched(config: &Path, c: &ServerConfig, eid: &str) -> Option<Vouched> {
    owner_account(config, c)?;
    if crate::block::is_blocked(config, eid) {
        return None;
    }
    view(config, c).index.get(eid).cloned()
}

/// Every vouched person (for the owner's management page): (person id, name, devices).
pub fn everyone(config: &Path, c: &ServerConfig) -> Vec<(String, String, usize)> {
    let mut out: HashMap<String, (String, usize)> = HashMap::new();
    for (eid, v) in &view(config, c).index {
        if crate::block::is_blocked(config, eid) {
            continue;
        }
        let e = out.entry(v.person.clone()).or_insert_with(|| (v.name.clone(), 0));
        e.1 += 1;
    }
    out.into_iter().map(|(id, (name, n))| (id, name, n)).collect()
}

/// The devices currently vouched as person `person` ("v:…").
pub fn devices_of(config: &Path, c: &ServerConfig, person: &str) -> Vec<String> {
    let mut out: Vec<String> = view(config, c).index.iter().filter(|(_, v)| v.person == person).map(|(e, _)| e.clone()).collect();
    out.sort();
    out
}

/// How many owner devices currently share this server with their friends.
pub fn sharing_devices(config: &Path, c: &ServerConfig) -> usize {
    if owner_account(config, c).is_none() {
        return 0;
    }
    view(config, c).live
}

/// The name of vouched person `person` ("v:…"), if listed.
pub fn name_of(config: &Path, c: &ServerConfig, person: &str) -> Option<String> {
    view(config, c).index.values().find(|v| v.person == person).map(|v| v.name.clone())
}

// ── server side: receiving a list ───────────────────────────────────────────

/// Times from another device's clock: never later than a day past ours (a far
/// "future" stamp would otherwise win every comparison for good).
fn clamp(t: u64, now: u64) -> u64 {
    t.min(now.saturating_add(DAY_MS))
}

fn clean_map(v: &Value, cap: usize, now: u64) -> HashMap<String, u64> {
    v.as_object().map(|o| {
        o.iter().filter(|(k, _)| k.parse::<iroh::PublicKey>().is_ok())
            .filter_map(|(k, t)| Some((k.clone(), clamp(t.as_u64()?, now))))
            .take(cap).collect()
    }).unwrap_or_default()
}

/// Parse + bound a `mailbox.members` request (None = malformed).
pub fn parse(req: &Value, now: u64) -> Option<OwnerList> {
    let people = req["people"].as_array()?;
    if people.len() > MAX_PEOPLE {
        return None;
    }
    let mut out = Vec::with_capacity(people.len());
    for p in people {
        let key = p["key"].as_str()?;
        key.parse::<iroh::PublicKey>().ok()?;
        let name: String = p["name"].as_str().unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(64).collect();
        let devices: Vec<Device> = p["devices"].as_array()?.iter().take(MAX_DEVICES)
            .filter_map(|d| {
                let eid = d["eid"].as_str()?;
                eid.parse::<iroh::PublicKey>().ok()?;
                Some(Device { eid: eid.to_owned(), since: clamp(d["since"].as_u64().unwrap_or(0), now) })
            })
            .collect();
        if devices.is_empty() {
            continue;
        }
        out.push(Person { key: key.to_owned(), name: if name.is_empty() { "A friend".into() } else { name }, devices });
    }
    Some(OwnerList {
        at: now,
        people: out,
        removed: clean_map(&req["removed"], MAX_REMOVED, now),
        linked: clean_map(&req["linked"], 256, now),
        unlinked: clean_map(&req["unlinked"], 256, now),
    })
}

/// `mailbox.members` from `who`: accepted only from an owner device.
pub fn receive(config: &Path, c: &ServerConfig, me: Option<&str>, who: &str, req: &Value) -> Value {
    // Taken from any device proving the owner's account (so a real owner device
    // can always report one the owner removed); it only COUNTS while the sender
    // is an owner device (see `compute`).
    let accepted = is_owner_device(config, c, who)
        || (crate::account::my_pub(config).is_none() && owner_account(config, c).is_some_and(|a| proving(config, &a).contains(who)));
    if !accepted {
        return json!({"ok": false, "reason": "denied"});
    }
    let Some(mut list) = parse(req, crate::chat::now_ms()) else {
        return json!({"ok": false, "reason": "invalid"});
    };
    // The owner and this server are never "friends of the owner" here.
    let account = owner_account(config, c);
    let friends = crate::friends::load(config);
    let owner_dev = |e: &str| friends.iter().any(|f| f.endpoint_id.as_deref() == Some(e) && f.account_pub.is_some() && f.account_pub == account);
    for p in &mut list.people {
        p.devices.retain(|d| Some(d.eid.as_str()) != me && d.eid != who && !owner_dev(&d.eid));
    }
    list.people.retain(|p| !p.devices.is_empty());
    let n = list.people.len();
    let mut lists = load(config);
    lists.insert(who.to_owned(), list);
    if lists.len() > MAX_LISTS {
        // Keep the freshest.
        let mut by_age: Vec<(String, u64)> = lists.iter().map(|(k, l)| (k.clone(), l.at)).collect();
        by_age.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        let keep: std::collections::HashSet<String> = by_age.into_iter().take(MAX_LISTS).map(|(k, _)| k).collect();
        lists.retain(|k, _| keep.contains(k));
    }
    save(config, &lists);
    log::info!("transfer-server: the owner shared {n} friend(s) with this server");
    json!({"ok": true, "people": n})
}

// ── owner side: building the list ───────────────────────────────────────────

/// This device's friends (not its own devices), grouped by person, plus its
/// removals and blocks and its account roster. The same shape every owner
/// device builds, so the server can merge them.
pub fn build(config: &Path) -> Value {
    let mine = crate::account::my_pub(config);
    let friends = crate::friends::load(config);
    let mut people: HashMap<String, Person> = HashMap::new();
    for f in &friends {
        let Some(eid) = f.endpoint_id.as_deref() else { continue };
        if f.account_pub.is_some() && f.account_pub == mine {
            continue;
        }
        if crate::block::is_blocked(config, eid) {
            continue;
        }
        let owner = crate::friends::thread_owner(config, &f.id).unwrap_or_else(|| f.clone());
        let key = owner.endpoint_id.clone().unwrap_or_else(|| eid.to_owned());
        let p = people.entry(key.clone()).or_insert_with(|| Person { key: key.clone(), name: owner.name.clone(), devices: vec![] });
        if p.devices.len() < MAX_DEVICES && !p.devices.iter().any(|d| d.eid == eid) {
            p.devices.push(Device { eid: eid.to_owned(), since: f.created_at });
        }
    }
    let mut people: Vec<Person> = people.into_values().collect();
    people.sort_by(|a, b| a.key.cmp(&b.key));
    for p in &mut people {
        p.devices.sort_by(|a, b| a.eid.cmp(&b.eid));
    }
    people.truncate(MAX_PEOPLE);
    let (linked, unlinked, removed_friends) = crate::account::roster_times(config);
    let mut removed: HashMap<String, u64> = removed_friends;
    for (eid, rec) in crate::block::snapshot(config) {
        if rec.blocked {
            let e = removed.entry(eid).or_default();
            *e = (*e).max(rec.at.max(1));
        }
    }
    // A removal only matters if it is newer than the current friendship; keep
    // the newest ones when there are very many.
    let mut removed: Vec<(String, u64)> = removed.into_iter().collect();
    removed.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    removed.truncate(MAX_REMOVED);
    let removed: std::collections::BTreeMap<String, u64> = removed.into_iter().collect();
    let linked: std::collections::BTreeMap<String, u64> = linked.into_iter().collect();
    let unlinked: std::collections::BTreeMap<String, u64> = unlinked.into_iter().collect();
    json!({"kind": "mailbox.members", "v": super::VERSION, "people": people, "removed": removed, "linked": linked, "unlinked": unlinked})
}

#[cfg(test)]
pub(crate) fn lists_for_tests(config: &Path) -> Lists {
    load(config)
}
