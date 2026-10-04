//! The user side of Transfer Servers: which servers we may use, depositing
//! chat/ops/files for offline friends, polling receipts, and fetching what
//! servers hold for us.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::{keys, seal};
use crate::iroh_net::{read_frame_cap, write_frame, IrohState};

static LOCK: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    crate::chat::now_ms()
}

fn read_store<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    match crate::settings::read_json_store(path) {
        crate::settings::StoreRead::Loaded(v) => v,
        _ => T::default(),
    }
}

fn write_store<T: Serialize>(path: &Path, v: &T) {
    match serde_json::to_vec(v) {
        Ok(bytes) => {
            if let Err(e) = crate::settings::write_atomic(path, &bytes) {
                log::warn!("mailbox: cannot save {}: {e}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
            }
        }
        Err(e) => log::warn!("mailbox: cannot serialize a store: {e}"),
    }
}

// ── servers we can use ──────────────────────────────────────────────────────

/// A Transfer Server this device knows it may use.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsableServer {
    pub eid: String,
    pub name: String,
    /// It belongs to this account (one of our own devices).
    #[serde(default)]
    pub own: bool,
    /// It lets us leave things for other people who use it.
    #[serde(default)]
    pub member: bool,
    /// It lets us leave things for anyone.
    #[serde(default)]
    pub through: bool,
    /// We opted in to sending through it ("Use it").
    #[serde(default)]
    pub use_it: bool,
    /// We opted in to having our messages held there (advertised to friends).
    #[serde(default)]
    pub hold_for_me: bool,
    /// "new" (show the offer card) | "seen" | "dismissed".
    #[serde(default)]
    pub offer: String,
    /// The owner took our access away.
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub learned_ms: u64,
    /// The server says it is ours (we're one of its owner's devices): it holds
    /// our messages and sends for us without asking.
    #[serde(default)]
    pub owner: bool,
    /// We (the owner) agreed to let our friends use it: we tell them about it
    /// and keep it up to date on who our friends are.
    #[serde(default)]
    pub share_friends: bool,
    /// Who the server lets use it: "me" | "chosen" | "all" ("" = unknown).
    #[serde(default)]
    pub access: String,
    /// Friends' devices that told us about it (it's their owner's server). Empty
    /// when the server itself told us (we're its friend).
    #[serde(default)]
    pub via: Vec<String>,
    /// For the UI (filled by `servers_view`, never stored): the thread of the
    /// person who shared it, and their name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_peer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_name: Option<String>,
}

impl UsableServer {
    pub fn usable(&self) -> bool {
        !self.revoked && !self.paused && (self.own || self.member)
    }
    /// We own it and agreed to share it with our friends.
    pub fn shared_by_me(&self) -> bool {
        self.owner && self.share_friends && !self.revoked && self.access != "me"
    }
    /// We may deposit here for anyone who uses it (not just the owner's people).
    fn sends_here(&self) -> bool {
        self.usable() && (self.own || self.use_it)
    }
}

fn servers_path(config: &Path) -> PathBuf {
    config.join("mailbox-servers.json")
}

pub fn servers(config: &Path) -> Vec<UsableServer> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let map: BTreeMap<String, UsableServer> = read_store(&servers_path(config));
    map.into_values().collect()
}

fn with_servers<T>(config: &Path, f: impl FnOnce(&mut BTreeMap<String, UsableServer>) -> T) -> T {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: BTreeMap<String, UsableServer> = read_store(&servers_path(config));
    let before = map.clone();
    let out = f(&mut map);
    if map != before {
        write_store(&servers_path(config), &map);
    }
    out
}

/// `servers`, with who shared each one (for the offer card and Settings).
pub fn servers_view(config: &Path) -> Vec<UsableServer> {
    servers(config).into_iter().map(|mut s| {
        if let Some(f) = s.via.iter().find_map(|e| crate::friends::chat_sender(config, e)) {
            s.via_peer = Some(f.id);
            s.via_name = Some(f.name);
        }
        s
    }).collect()
}

/// Our own servers we share with our friends (told to friends in hellos, so
/// their devices can ask the server to let them in).
pub fn my_shared(config: &Path) -> Vec<Value> {
    servers(config).into_iter()
        .filter(|s| s.shared_by_me() && !s.paused)
        .map(|s| json!({"eid": s.eid, "name": s.name}))
        .collect()
}

/// Servers that hold messages for us, as advertised to friends.
pub fn my_inbox(config: &Path) -> Vec<Value> {
    servers(config).into_iter()
        .filter(|s| s.usable() && (s.own || s.hold_for_me))
        .map(|s| json!({"eid": s.eid, "name": s.name}))
        .collect()
}

/// Servers we send through (friends may receive our items from these).
pub fn my_sends(config: &Path) -> Vec<Value> {
    servers(config).into_iter()
        .filter(|s| s.sends_here())
        .map(|s| json!({"eid": s.eid, "name": s.name}))
        .collect()
}

#[derive(Default, Debug)]
pub struct GrantChange {
    pub new_offer: bool,
    pub new_own: bool,
    pub revoked: bool,
    pub changed: bool,
}
impl GrantChange {
    pub fn any(&self) -> bool {
        self.changed
    }
}

/// Apply what server `who` says we may do (from its hello). `None`/null from a
/// peer that previously granted access = access removed.
pub fn learn_grant(config: &Path, who: &str, grant: Option<&Value>, verified_own: bool) -> GrantChange {
    let mut change = GrantChange::default();
    with_servers(config, |map| {
        match grant.filter(|g| g.is_object()) {
            Some(g) => {
                // "It's one of your own devices" is OUR call (account roster),
                // never the peer's claim.
                let own = verified_own && g["own"].as_bool().unwrap_or(false);
                // "You own me" is only the server's claim: it changes nothing by
                // itself. The user's yes ("share") is what makes it hold our
                // messages and go to our friends.
                let owner = own || g["owner"].as_bool().unwrap_or(false);
                let member = g["member"].as_bool().unwrap_or(false) || own;
                let access: String = g["access"].as_str().unwrap_or("").chars().take(8).collect();
                let entry = map.entry(who.to_owned()).or_insert_with(|| {
                    change.new_offer = !own;
                    change.new_own = own;
                    UsableServer { eid: who.to_owned(), learned_ms: now(), offer: if own { "seen".into() } else { "new".into() }, ..Default::default() }
                });
                let before = entry.clone();
                if own && !entry.own {
                    change.new_own = true;
                }
                entry.name = g["name"].as_str().unwrap_or("Transfer Server").chars().take(64).collect();
                entry.own = own;
                if owner && !entry.owner && !own {
                    // Newly ours: ask once whether our friends may use it.
                    entry.offer = if access == "me" || entry.share_friends { "seen".into() } else { "share".into() };
                    change.new_offer = entry.offer == "share";
                }
                if !owner && entry.owner && entry.offer == "share" {
                    entry.offer = "seen".into();
                }
                entry.owner = owner;
                entry.access = access;
                entry.member = member;
                entry.through = g["through"].as_bool().unwrap_or(false) || own;
                entry.paused = g["paused"].as_bool().unwrap_or(false);
                if own {
                    entry.use_it = true;
                    entry.hold_for_me = true;
                }
                if !owner {
                    entry.share_friends = false;
                }
                if entry.revoked {
                    entry.revoked = false;
                    if !own && !owner && entry.offer != "dismissed" {
                        entry.offer = "new".into();
                        change.new_offer = true;
                    }
                }
                change.changed = *entry != before;
            }
            None => {
                if let Some(entry) = map.get_mut(who) {
                    if !entry.revoked {
                        entry.revoked = true;
                        change.revoked = true;
                        change.changed = true;
                    }
                }
            }
        }
    });
    change
}

/// The user's choices about a server offered to them.
pub fn set_prefs(config: &Path, eid: &str, use_it: Option<bool>, hold_for_me: Option<bool>, offer: Option<&str>) -> Result<(), String> {
    set_prefs_full(config, eid, use_it, hold_for_me, offer, None)
}

/// `set_prefs`, plus (for a server we own) whether our friends may use it.
pub fn set_prefs_full(config: &Path, eid: &str, use_it: Option<bool>, hold_for_me: Option<bool>, offer: Option<&str>, share_friends: Option<bool>) -> Result<(), String> {
    with_servers(config, |map| {
        let s = map.get_mut(eid).ok_or("That Transfer Server isn't available any more.")?;
        if let Some(v) = share_friends {
            if v && !s.owner {
                return Err("Only the owner of this Transfer Server can share it.".to_owned());
            }
            s.share_friends = v;
            if v {
                // Saying yes = "it's mine": it holds our messages and sends for us.
                s.use_it = true;
                s.hold_for_me = true;
            }
        }
        if let Some(v) = use_it {
            s.use_it = v || s.own;
        }
        if let Some(v) = hold_for_me {
            s.hold_for_me = v || s.own;
        }
        if let Some(o) = offer {
            s.offer = o.to_owned();
        }
        Ok(())
    })
}

// ── servers our friends share (their owner's server) ───────────────────────

/// Servers our friends' (or own) devices told us they share, and who told us.
pub fn introduced(config: &Path, me: &str) -> Vec<(String, String, Vec<String>)> {
    let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut peers: Vec<(String, keys::PeerInfo)> = keys::peers(config).into_iter().collect();
    peers.sort_by(|a, b| a.0.cmp(&b.0));
    for (peer, p) in peers {
        if p.shared.is_empty() || crate::block::is_blocked(config, &peer) {
            continue;
        }
        if crate::friends::chat_sender(config, &peer).is_none() && !crate::account::is_own_device(config, &peer) {
            continue;
        }
        for sref in &p.shared {
            if sref.eid == me || crate::block::is_blocked(config, &sref.eid) {
                continue;
            }
            match out.iter_mut().find(|(e, _, _)| *e == sref.eid) {
                Some((_, _, via)) => {
                    if !via.contains(&peer) {
                        via.push(peer.clone());
                    }
                }
                None => out.push((sref.eid.clone(), sref.name.clone(), vec![peer.clone()])),
            }
        }
    }
    out.truncate(8);
    out
}

/// What a server's `mailbox.hello` answered.
pub enum HelloReply {
    Ok(Value),
    /// It turned us down ("denied", or it's no longer a Transfer Server).
    Refused,
    Unreachable,
}

pub async fn server_hello_reply(ep: &iroh::Endpoint, server: &str) -> HelloReply {
    let Some(conn) = connect(ep, server, Duration::from_secs(6)).await else { return HelloReply::Unreachable };
    let reply = rpc(&conn, &json!({"kind": "mailbox.hello", "v": super::VERSION})).await;
    conn.close(0u32.into(), b"done");
    match reply {
        Ok(r) if r["ok"].as_bool() == Some(true) => HelloReply::Ok(r),
        Ok(r) if matches!(r["reason"].as_str(), Some("denied" | "off")) => HelloReply::Refused,
        _ => HelloReply::Unreachable,
    }
}

/// Apply a server's `mailbox.hello` answer for a server friends told us about.
pub fn learn_intro(config: &Path, server: &str, reply: &HelloReply, via: &[String]) -> GrantChange {
    match reply {
        HelloReply::Ok(r) => {
            let rights = &r["rights"];
            // A server we heard about from a friend is never ours, whatever it
            // says (it stays ours only if it already told us so itself).
            let was_owner = servers(config).iter().any(|s| s.eid == server && s.owner);
            let g = json!({"name": r["name"], "own": rights["own"], "member": rights["member"], "through": rights["through"],
                "owner": was_owner, "paused": r["paused"], "access": r["access"]});
            let own = crate::account::is_own_device(config, server);
            let mut change = learn_grant(config, server, Some(&g), own);
            with_servers(config, |map| {
                if let Some(e) = map.get_mut(server) {
                    for v in via {
                        if !e.via.contains(v) && e.via.len() < 8 {
                            e.via.push(v.clone());
                            change.changed = true;
                        }
                    }
                }
            });
            change
        }
        HelloReply::Refused => {
            if servers(config).iter().any(|s| s.eid == server) {
                learn_grant(config, server, None, false)
            } else {
                GrantChange::default()
            }
        }
        HelloReply::Unreachable => GrantChange::default(),
    }
}

static INTRO_CHECKED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Servers we already gave one quick second look.
static INTRO_QUICK: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Ask each server our friends share whether we may use it (new ones soon and
/// often; known ones a few times a day, so a removal shows up).
pub async fn check_introduced(net: &IrohState, config: &Path) -> bool {
    let Some(ep) = net.get().cloned() else { return false };
    let me = ep.id().to_string();
    let known = servers(config);
    let mut any = false;
    for (server, _name, via) in introduced(config, &me) {
        let entry = known.iter().find(|s| s.eid == server);
        let every = match entry {
            None => Duration::from_secs(150),
            Some(e) if e.revoked => Duration::from_secs(1800),
            Some(e) if e.via.is_empty() => continue, // the server tells us itself (we're its friend)
            Some(_) => Duration::from_secs(6 * 3600),
        };
        {
            let mut g = INTRO_CHECKED.lock().unwrap_or_else(|p| p.into_inner());
            let m = g.get_or_insert_with(HashMap::new);
            if m.get(&server).is_some_and(|t| t.elapsed() < every) {
                continue;
            }
            m.insert(server.clone(), Instant::now());
        }
        let reply = server_hello_reply(&ep, &server).await;
        if entry.is_none() && !matches!(reply, HelloReply::Ok(_)) {
            // The owner's device may be telling the server about us right now:
            // look again shortly (once), then at the normal pace.
            let mut g = INTRO_CHECKED.lock().unwrap_or_else(|p| p.into_inner());
            let m = g.get_or_insert_with(HashMap::new);
            let quick = !INTRO_QUICK.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashSet::new).insert(server.clone());
            if !quick {
                if let Some(t) = Instant::now().checked_sub(Duration::from_secs(150 - 30)) {
                    m.insert(server.clone(), t);
                }
                tokio::spawn(async {
                    tokio::time::sleep(Duration::from_secs(32)).await;
                    wake();
                });
            }
        }
        let change = learn_intro(config, &server, &reply, &via);
        any |= change.any();
    }
    any | prune_introduced(config, &me)
}

/// Servers only friends told us about, that no current friend tells us about
/// any more (they stopped sharing, or aren't our friend now): drop those
/// friends, and show the server as gone once nobody vouches for it.
fn prune_introduced(config: &Path, me: &str) -> bool {
    let current = introduced(config, me);
    let direct = |eid: &str| crate::friends::chat_sender(config, eid).is_some() || crate::account::is_own_device(config, eid);
    with_servers(config, |map| {
        let mut changed = false;
        for (eid, e) in map.iter_mut().filter(|(_, e)| !e.via.is_empty()) {
            let now_via: Vec<String> = current.iter().find(|(s, _, _)| s == eid).map(|(_, _, v)| v.clone()).unwrap_or_default();
            let keep: Vec<String> = e.via.iter().filter(|v| now_via.contains(v)).cloned().collect();
            if keep != e.via {
                e.via = keep;
                changed = true;
            }
            if e.via.is_empty() && !e.revoked && !direct(eid) {
                e.revoked = true;
                changed = true;
            }
        }
        changed
    })
}

/// A friend just told us about a server they share: look at it on the next pass.
pub fn recheck_introduced(server: &str) {
    if let Some(m) = INTRO_CHECKED.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        m.remove(server);
    }
    if let Some(q) = INTRO_QUICK.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        q.remove(server);
    }
}

// ── owner side: keeping our shared servers up to date on our friends ────────

static MEMBERS_SENT: Mutex<Option<HashMap<String, (String, Instant, HashSet<String>)>>> = Mutex::new(None);

/// Send our friends list to each server we own and share (when it changed, or
/// every few hours). True when anything was sent.
pub async fn push_members(net: &IrohState, config: &Path) -> bool {
    let Some(ep) = net.get().cloned() else { return false };
    let mut sent = false;
    for s in servers(config).into_iter().filter(|s| s.shared_by_me()) {
        let body = super::members::build(config);
        let hash = hex::encode(Sha256::digest(serde_json::to_vec(&body).unwrap_or_default()));
        let fresh = MEMBERS_SENT.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
            .and_then(|m| m.get(&s.eid))
            .is_some_and(|(h, at, _)| *h == hash && at.elapsed() < Duration::from_secs(6 * 3600));
        if fresh {
            continue;
        }
        let Some(reply) = server_rpc(&ep, config, &s.eid, &body).await else { continue };
        if reply["ok"].as_bool() == Some(true) {
            let eids: HashSet<String> = body["people"].as_array().into_iter().flatten()
                .flat_map(|p| p["devices"].as_array().cloned().unwrap_or_default())
                .filter_map(|d| d["eid"].as_str().map(String::from)).collect();
            MEMBERS_SENT.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new)
                .insert(s.eid.clone(), (hash, Instant::now(), eids));
            log::info!("mailbox: told our Transfer Server who our friends are ({} people)", reply["people"].as_u64().unwrap_or(0));
            sent = true;
        } else {
            log::info!("mailbox: our Transfer Server didn't take the friends list ({})", reply["reason"].as_str().unwrap_or("?"));
        }
    }
    sent
}

/// A friend we haven't told our shared servers about yet said hello.
pub fn members_behind(config: &Path, friend_eid: &str) -> bool {
    let shared: Vec<String> = servers(config).into_iter().filter(|s| s.shared_by_me()).map(|s| s.eid).collect();
    if shared.is_empty() {
        return false;
    }
    let g = MEMBERS_SENT.lock().unwrap_or_else(|p| p.into_inner());
    shared.iter().any(|srv| !g.as_ref().and_then(|m| m.get(srv)).is_some_and(|(_, _, eids)| eids.contains(friend_eid)))
}

/// Stop using a server entirely (it may be offered again by a later hello).
pub fn forget(config: &Path, eid: &str) {
    with_servers(config, |map| {
        map.remove(eid);
    });
}

#[cfg(test)]
pub(crate) fn set_server_for_tests(config: &Path, s: UsableServer) {
    with_servers(config, |map| {
        map.insert(s.eid.clone(), s);
    });
}

// ── choosing a server ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub server: String,
    pub name: String,
}

/// Where to leave something for a person with devices `recipients`, best
/// first: servers that person told us hold their messages (email-MX style),
/// then servers we send through ourselves.
pub fn routes(config: &Path, me: &str, recipients: &[String]) -> Vec<Route> {
    let mine = servers(config);
    let mut out: Vec<Route> = Vec::new();
    let skip = |eid: &str| eid == me || recipients.iter().any(|r| r == eid);
    for s in keys::inbox_servers(config, recipients) {
        if skip(&s.eid) || out.iter().any(|r| r.server == s.eid) {
            continue;
        }
        if let Some(rec) = mine.iter().find(|m| m.eid == s.eid && m.usable()) {
            out.push(Route { server: rec.eid.clone(), name: rec.name.clone() });
        }
    }
    let mut own_first: Vec<&UsableServer> = mine.iter().filter(|s| s.sends_here()).collect();
    own_first.sort_by_key(|s| (!s.own, s.learned_ms));
    for s in own_first {
        if !skip(&s.eid) && !out.iter().any(|r| r.server == s.eid) {
            out.push(Route { server: s.eid.clone(), name: s.name.clone() });
        }
    }
    out
}

/// This device's own Transfer Server, when it is one: a message it sends can
/// wait right here (no network hop) for a friend's sleeping device.
fn self_route(config: &Path) -> Option<Route> {
    if !super::server::hosting_supported() {
        return None;
    }
    let c = super::server::load_config(config);
    (c.enabled && !c.paused).then(|| Route { server: SELF.into(), name: super::server::server_name(&c) })
}

/// The route id standing for "this device's own server".
pub const SELF: &str = "self";

/// `routes`, for chat: our own Transfer Server comes first when we are one
/// (chat items are small; files still go through another device).
pub fn chat_routes(config: &Path, me: &str, recipients: &[String]) -> Vec<Route> {
    let mut out: Vec<Route> = self_route(config).into_iter().collect();
    out.extend(routes(config, me, recipients));
    out
}

/// Whether a chat message to `peer_id` could be held right now.
pub fn can_hold_chat(config: &Path, me: &str, peer_id: &str) -> bool {
    let eids = person_devices(config, peer_id, me);
    !eids.is_empty() && !keys::recipients(config, &eids).is_empty() && !chat_routes(config, me, &eids).is_empty()
}

/// Which of these devices a chat item could be held for right now.
pub fn chat_holdable_devices(config: &Path, me: &str, eids: &[String]) -> Vec<String> {
    if eids.is_empty() || chat_routes(config, me, eids).is_empty() {
        return vec![];
    }
    keys::recipients(config, eids).into_iter().map(|r| r.eid).collect()
}

/// The devices of the person owning thread `peer_id`, excluding us.
fn person_devices(config: &Path, peer_id: &str, me: &str) -> Vec<String> {
    let owner = crate::friends::thread_owner(config, peer_id).map_or_else(|| peer_id.to_owned(), |o| o.id);
    crate::friends::person_endpoints(config, &owner).into_iter().filter(|e| e != me).collect()
}

/// Whether a message to `peer_id` could be held right now (a usable route AND
/// at least one of their device keys).
pub fn can_hold(config: &Path, me: &str, peer_id: &str) -> bool {
    let eids = person_devices(config, peer_id, me);
    !eids.is_empty() && !keys::recipients(config, &eids).is_empty() && !routes(config, me, &eids).is_empty()
}

/// Whether the first server we'd use keeps a file until EVERY device it was
/// sealed for has it (older servers hand it to whichever device fetches first).
pub async fn route_keeps_per_device(net: &IrohState, config: &Path, me: &str, eids: &[String]) -> bool {
    let Some(ep) = net.get().cloned() else { return false };
    let Some(route) = routes(config, me, eids).into_iter().next() else { return false };
    server_hello(&ep, &route.server).await.is_some_and(|h| h["per_device"].as_bool() == Some(true))
}

/// Which of these devices a server could hold something for right now (we have
/// their mailbox key and a route that takes them). Empty = none.
pub fn holdable_devices(config: &Path, me: &str, eids: &[String]) -> Vec<String> {
    if eids.is_empty() || routes(config, me, eids).is_empty() {
        return vec![];
    }
    keys::recipients(config, eids).into_iter().map(|r| r.eid).collect()
}

/// The server a message to `peer_id` would be held on (its name), if any.
pub fn hold_route(config: &Path, me: &str, peer_id: &str) -> Option<String> {
    let eids = person_devices(config, peer_id, me);
    if eids.is_empty() || keys::recipients(config, &eids).is_empty() {
        return None;
    }
    chat_routes(config, me, &eids).into_iter().next().map(|r| r.name)
}

/// Any of these devices showed life very recently (so a direct try is worth it).
pub fn any_live(eids: &[String]) -> bool {
    eids.iter().any(|e| super::seen_within(e, Duration::from_secs(20)))
}

// ── sent ledger ─────────────────────────────────────────────────────────────

/// Something we left on a server, until it reaches a final state.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub item_id: String,
    pub server: String,
    pub server_name: String,
    /// "chat" | "op" | "file".
    pub kind: String,
    /// Our thread id for the recipient.
    pub peer_id: String,
    #[serde(default)]
    pub msg_id: Option<String>,
    #[serde(default)]
    pub xfer_id: Option<String>,
    pub created_ms: u64,
    /// "uploading" | "held" | "delivered" | "expired" | "rejected" | "canceled" | "lost".
    pub state: String,
    #[serde(default)]
    pub updated_ms: u64,
    #[serde(default)]
    pub held_until: u64,
    /// Resume material for an interrupted file upload (dropped once held).
    #[serde(default)]
    pub header: Option<Value>,
    #[serde(default)]
    pub file_key: Option<String>,
    #[serde(default)]
    pub files_sig: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub names: Vec<String>,
    /// The transfer card this file send showed on (sender side).
    #[serde(default)]
    pub transfer_id: Option<String>,
    /// Recipient devices the server says already took it (a file item stays
    /// until every device it was sealed for has it).
    #[serde(default)]
    pub delivered_to: Vec<String>,
    /// The devices it was sealed for.
    #[serde(default)]
    pub to: Vec<String>,
    /// An extra per-device copy of a message another of their devices already
    /// got directly: its receipts never change the message's status.
    #[serde(default)]
    pub copy: bool,
}

fn sent_path(config: &Path) -> PathBuf {
    config.join("mailbox-sent.json")
}

pub fn sent_all(config: &Path) -> BTreeMap<String, Sent> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_store(&sent_path(config))
}

fn with_sent<T>(config: &Path, f: impl FnOnce(&mut BTreeMap<String, Sent>) -> T) -> T {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: BTreeMap<String, Sent> = read_store(&sent_path(config));
    let out = f(&mut map);
    // Final states are kept 30 days (for the UI's "expired" copy), then dropped.
    let t = now();
    map.retain(|_, s| matches!(s.state.as_str(), "uploading" | "held") || s.updated_ms + 30 * super::server::DAY_MS > t);
    write_store(&sent_path(config), &map);
    out
}

// ── seen ledger (recipient-side replay/duplicate guard) ─────────────────────

fn seen_path(config: &Path) -> PathBuf {
    config.join("mailbox-seen.json")
}
const SEEN_MS: u64 = 60 * super::server::DAY_MS;

pub fn already_seen(config: &Path, item_id: &str) -> bool {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let map: HashMap<String, u64> = read_store(&seen_path(config));
    map.contains_key(item_id)
}

fn mark_seen(config: &Path, item_id: &str) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: HashMap<String, u64> = read_store(&seen_path(config));
    let t = now();
    map.insert(item_id.to_owned(), t);
    map.retain(|_, at| *at + SEEN_MS > t);
    if map.len() > 50_000 {
        let mut v: Vec<(u64, String)> = map.iter().map(|(k, a)| (*a, k.clone())).collect();
        v.sort();
        for (_, k) in v.into_iter().take(map.len() - 50_000) {
            map.remove(&k);
        }
    }
    write_store(&seen_path(config), &map);
}

// ── depositing ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepositError {
    /// No server we may use for this person.
    NoRoute,
    /// We don't have any of their mailbox keys (old app / never met).
    NoKeys,
    /// A server said no ("full", "paused", "denied", "too_big", …).
    Refused { reason: String, server: String },
    /// Couldn't reach any server.
    Unreachable,
    /// The recipient came online mid-upload; send directly instead.
    GoDirect,
    Canceled,
    Failed(String),
}

impl DepositError {
    /// A reason code safe for logs (no names).
    pub fn code(&self) -> &str {
        match self {
            Self::NoRoute => "no-route",
            Self::NoKeys => "no-keys",
            Self::Refused { reason, .. } => match reason.as_str() {
                r @ ("full" | "user_quota" | "recipient_full" | "paused" | "too_big" | "denied" | "recipient" | "off" | "conflict" | "invalid") => r,
                _ => "refused",
            },
            Self::Unreachable => "unreachable",
            Self::GoDirect => "go-direct",
            Self::Canceled => "canceled",
            Self::Failed(_) => "failed",
        }
    }
}

impl std::fmt::Display for DepositError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            Self::NoRoute => "No Transfer Server available".to_owned(),
            Self::NoKeys => "They need to update DropBeam to receive while offline".to_owned(),
            Self::Refused { reason, server } => refusal_text(reason, server),
            Self::Unreachable => "Couldn't reach the Transfer Server".to_owned(),
            Self::GoDirect => "They came online — sending directly".to_owned(),
            Self::Canceled => "canceled".to_owned(),
            Self::Failed(e) => e.clone(),
        })
    }
}

/// The short note a chat bubble shows for a server refusal.
pub fn refusal_text(reason: &str, server: &str) -> String {
    match reason {
        "full" | "user_quota" | "recipient_full" => format!("{server} is full"),
        "paused" => format!("{server} is paused"),
        "too_big" => format!("Too big for {server}"),
        "denied" | "recipient" => format!("{server} can't hold this"),
        "off" => format!("{server} isn't a Transfer Server any more"),
        _ => format!("Couldn't use {server}"),
    }
}

/// Short machine note stored on a chat message for the UI.
pub fn note_for(e: &DepositError) -> Option<String> {
    match e {
        DepositError::NoKeys => Some("needs_update".into()),
        DepositError::Refused { reason, .. } => Some(match reason.as_str() {
            "full" | "user_quota" | "recipient_full" => "full".into(),
            "paused" => "paused".into(),
            _ => "refused".into(),
        }),
        DepositError::Unreachable => Some("unreachable".into()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Held {
    pub server: String,
    pub name: String,
    pub item_id: String,
    pub until: u64,
}

enum RpcError {
    Recipient(Vec<String>),
    Refused(String),
    Unreachable,
    GoDirect,
    Canceled,
    Failed(String),
}

/// Payload for a file deposit.
struct Upload<'a> {
    files: &'a [(PathBuf, u64, u64)],
    key: seal::PayloadKey,
    progress: &'a (dyn Fn(u64, u64) + Send + Sync),
    cancel: &'a AtomicBool,
    go_direct: &'a (dyn Fn(u64, u64) -> bool + Send + Sync),
}

async fn connect(ep: &iroh::Endpoint, eid: &str, budget: Duration) -> Option<iroh::endpoint::Connection> {
    let id = eid.parse::<iroh::EndpointId>().ok()?;
    tokio::time::timeout(budget, ep.connect(crate::iroh_net::dial_addr(id), crate::iroh_net::ALPN)).await.ok()?.ok()
}

async fn rpc(conn: &iroh::endpoint::Connection, req: &Value) -> Result<Value> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    tokio::time::timeout(Duration::from_secs(20), read_frame_cap(&mut recv, 4 << 20)).await.context("server didn't answer")?
}

/// Leave a small (chat/op) sealed item on `server`, or on this device's own
/// server for `SELF`.
async fn deposit_small(ep: &iroh::Endpoint, config: &Path, server: &str, env: &seal::Envelope, push: &Value) -> Result<u64, RpcError> {
    if server != SELF {
        return deposit_on(ep, server, env, push, None, true).await;
    }
    let header = serde_json::to_value(env).map_err(|e| RpcError::Failed(e.to_string()))?;
    let req = json!({"kind": "mailbox.deposit", "v": super::VERSION, "header": header, "push": push, "all_devices": true});
    let me = ep.id().to_string();
    let config = config.to_path_buf();
    let r = tokio::task::spawn_blocking(move || super::server::deposit_local(&config, &me, &req)).await
        .map_err(|e| RpcError::Failed(e.to_string()))?;
    r.map_err(|reply| {
        let reason = reply["reason"].as_str().unwrap_or("refused").to_owned();
        if reason == "recipient" {
            let outside = reply["not_members"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            RpcError::Recipient(outside)
        } else {
            RpcError::Refused(reason)
        }
    })
}

/// One request/reply with `server` (or this device's own server for `SELF`).
async fn server_rpc(ep: &iroh::Endpoint, config: &Path, server: &str, req: &Value) -> Option<Value> {
    if server == SELF {
        return Some(super::server::local_rpc(config, &ep.id().to_string(), req));
    }
    let conn = connect(ep, server, Duration::from_secs(6)).await?;
    let reply = rpc(&conn, req).await;
    conn.close(0u32.into(), b"done");
    reply.ok()
}

/// Hand our sealed push token to a server (it can only use it via the relay).
pub async fn push_register(ep: &iroh::Endpoint, server: &str, sealed_token: &str) -> bool {
    let Some(conn) = connect(ep, server, Duration::from_secs(8)).await else { return false };
    let reply = rpc(&conn, &json!({"kind": "mailbox.push-register", "v": super::VERSION, "sealed_token": sealed_token})).await;
    conn.close(0u32.into(), b"done");
    reply.is_ok_and(|r| r["ok"].as_bool() == Some(true))
}

/// Ask `server` what we may do there (None = unreachable / not a server).
pub async fn server_hello(ep: &iroh::Endpoint, server: &str) -> Option<Value> {
    let conn = connect(ep, server, Duration::from_secs(6)).await?;
    let reply = rpc(&conn, &json!({"kind": "mailbox.hello", "v": super::VERSION})).await.ok()?;
    conn.close(0u32.into(), b"done");
    (reply["ok"].as_bool() == Some(true)).then_some(reply)
}

fn is_file_changed(files: &[(PathBuf, u64, u64)]) -> bool {
    files.iter().any(|(p, size, mtime)| {
        std::fs::metadata(p).map(|m| m.len() != *size || crate::iroh_net::mtime_secs(&m) != *mtime).unwrap_or(true)
    })
}

/// One deposit attempt of an already-sealed envelope on one server.
async fn deposit_on(ep: &iroh::Endpoint, server: &str, env: &seal::Envelope, push: &Value, upload: Option<&Upload<'_>>, all_devices: bool) -> Result<u64, RpcError> {
    let conn = connect(ep, server, Duration::from_secs(8)).await.ok_or(RpcError::Unreachable)?;
    let result = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|_| RpcError::Unreachable)?;
        let header = serde_json::to_value(env).map_err(|e| RpcError::Failed(e.to_string()))?;
        write_frame(&mut send, &json!({"kind": "mailbox.deposit", "v": super::VERSION, "header": header, "push": push,
            // It goes to EVERY device it's sealed for (the server keeps it until each has it).
            "all_devices": all_devices}))
            .await.map_err(|_| RpcError::Unreachable)?;
        let reply = tokio::time::timeout(Duration::from_secs(30), read_frame_cap(&mut recv, 64 * 1024)).await
            .map_err(|_| RpcError::Unreachable)?.map_err(|_| RpcError::Unreachable)?;
        if reply["ok"].as_bool() != Some(true) {
            let reason = reply["reason"].as_str().unwrap_or("refused").to_owned();
            if reason == "recipient" {
                let outside = reply["not_members"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                return Err(RpcError::Recipient(outside));
            }
            return Err(RpcError::Refused(reason));
        }
        if reply["state"].as_str() == Some("held") {
            return Ok(reply["held_until"].as_u64().unwrap_or(0));
        }
        let have = reply["have"].as_u64().unwrap_or(0);
        if let Some(up) = upload {
            stream_payload(&mut send, env, up, have).await?;
        }
        send.finish().map_err(|_| RpcError::Unreachable)?;
        let done = tokio::time::timeout(Duration::from_secs(300), read_frame_cap(&mut recv, 64 * 1024)).await
            .map_err(|_| RpcError::Unreachable)?.map_err(|_| RpcError::Unreachable)?;
        if done["ok"].as_bool() == Some(true) {
            Ok(done["held_until"].as_u64().unwrap_or(0))
        } else {
            Err(RpcError::Refused(done["reason"].as_str().unwrap_or("refused").to_owned()))
        }
    }
    .await;
    conn.close(0u32.into(), b"done");
    result
}

async fn stream_payload(send: &mut iroh::endpoint::SendStream, env: &seal::Envelope, up: &Upload<'_>, have: u64) -> Result<(), RpcError> {
    let io = |e: std::io::Error| RpcError::Failed(e.to_string());
    let first = have / seal::CT_SEG;
    let mut pos = first * seal::SEG; // plaintext offset
    // Find the file holding plaintext offset `pos` (empty files hold nothing).
    let mut file_idx = 0usize;
    let mut file_off = pos;
    while file_idx < up.files.len() && (up.files[file_idx].1 == 0 || file_off >= up.files[file_idx].1) {
        file_off -= up.files[file_idx].1;
        file_idx += 1;
    }
    let mut cur: Option<tokio::fs::File> = None;
    let mut last_check = Instant::now();
    for i in first..env.segs {
        if up.cancel.load(Ordering::SeqCst) {
            return Err(RpcError::Canceled);
        }
        if last_check.elapsed() > Duration::from_secs(30) {
            last_check = Instant::now();
            if (up.go_direct)(pos, env.size) {
                return Err(RpcError::GoDirect);
            }
            if is_file_changed(up.files) {
                return Err(RpcError::Failed("A file changed while it was uploading".into()));
            }
        }
        let want = seal::seg_len(env.size, env.segs, i) as usize;
        let mut buf = vec![0u8; want];
        let mut filled = 0usize;
        while filled < want {
            if file_idx >= up.files.len() {
                return Err(RpcError::Failed("A file got shorter while uploading".into()));
            }
            if cur.is_none() {
                let mut f = tokio::fs::File::open(&up.files[file_idx].0).await.map_err(io)?;
                f.seek(std::io::SeekFrom::Start(file_off)).await.map_err(io)?;
                cur = Some(f);
            }
            let left_in_file = (up.files[file_idx].1 - file_off) as usize;
            let take = left_in_file.min(want - filled);
            cur.as_mut().unwrap().read_exact(&mut buf[filled..filled + take]).await.map_err(io)?;
            filled += take;
            file_off += take as u64;
            if file_off == up.files[file_idx].1 {
                cur = None;
                file_off = 0;
                file_idx += 1;
                while file_idx < up.files.len() && up.files[file_idx].1 == 0 {
                    file_idx += 1;
                }
            }
        }
        up.key.seal_segment(i, i + 1 == env.segs, &mut buf);
        send.write_all(&buf).await.map_err(|_| RpcError::Unreachable)?;
        pos += want as u64;
        (up.progress)(pos, env.size);
    }
    Ok(())
}

/// Leave a chat message (or edit/unsend/reaction op) for person `peer_id`.
/// `frame` is the exact `{kind:"chat", …}` frame a direct send would carry.
pub async fn deposit_chat(net: &IrohState, config: &Path, peer_id: &str, kind: &str, frame: &Value, msg_id: Option<&str>) -> Result<Held, DepositError> {
    deposit_chat_on(net, config, peer_id, kind, frame, msg_id, None).await
}

/// `deposit_chat`, optionally pinned to one server (an edit/reaction follows
/// its held message to the same server, which hands items over in order).
pub async fn deposit_chat_on(net: &IrohState, config: &Path, peer_id: &str, kind: &str, frame: &Value, msg_id: Option<&str>, only_server: Option<&str>) -> Result<Held, DepositError> {
    deposit_chat_for(net, config, peer_id, kind, frame, msg_id, only_server, None, false).await
}

/// `deposit_chat_on`, sealed only for `devices` (some of the person's devices)
/// when given. `copy` = another device already got it directly; this is the
/// per-device copy for the ones that didn't answer (iMessage-style).
#[allow(clippy::too_many_arguments)]
pub async fn deposit_chat_for(net: &IrohState, config: &Path, peer_id: &str, kind: &str, frame: &Value, msg_id: Option<&str>,
    only_server: Option<&str>, devices: Option<&[String]>, copy: bool) -> Result<Held, DepositError> {
    let ep = net.get().cloned().ok_or(DepositError::Unreachable)?;
    let me = ep.id().to_string();
    let mut eids = person_devices(config, peer_id, &me);
    if let Some(only) = devices {
        eids.retain(|e| only.contains(e));
    }
    let mut recips = keys::recipients(config, &eids);
    if recips.is_empty() {
        return Err(DepositError::NoKeys);
    }
    let mut routes = chat_routes(config, &me, &eids);
    if let Some(only) = only_server {
        routes.retain(|r| r.server == only);
    }
    if routes.is_empty() {
        return Err(DepositError::NoRoute);
    }
    let meta = padded(frame).map_err(|e| DepositError::Failed(e.to_string()))?;
    let mut last = DepositError::Unreachable;
    for route in routes {
        let mut to = std::mem::take(&mut recips);
        let mut tries = 0;
        while !to.is_empty() && tries < 3 {
            tries += 1;
            let item_id = uuid::Uuid::new_v4().to_string();
            let (env, _) = seal::seal(ep.secret_key(), &item_id, kind, now(), &to, &meta, 0).map_err(|e| DepositError::Failed(e.to_string()))?;
            let push = super::push::previews(config, &to, frame);
            match deposit_small(&ep, config, &route.server, &env, &push).await {
                Ok(until) => {
                    let sealed_for: Vec<String> = to.iter().map(|r| r.eid.clone()).collect();
                    let n = sealed_for.len();
                    with_sent(config, |m| {
                        m.insert(item_id.clone(), Sent {
                            item_id: item_id.clone(), server: route.server.clone(), server_name: route.name.clone(),
                            kind: kind.to_owned(), peer_id: peer_id.to_owned(), msg_id: msg_id.map(String::from), xfer_id: None,
                            created_ms: env.created_ms, state: "held".into(), updated_ms: now(), held_until: until,
                            to: sealed_for, copy, ..Default::default()
                        });
                    });
                    log::info!("mailbox: {kind} held on a Transfer Server for {n} offline device(s){}", if copy { " (another device already has it)" } else { "" });
                    return Ok(Held { server: route.server.clone(), name: route.name.clone(), item_id, until });
                }
                Err(RpcError::Recipient(outside)) => {
                    to.retain(|r| !outside.contains(&r.eid));
                    last = DepositError::Refused { reason: "recipient".into(), server: route.name.clone() };
                }
                Err(RpcError::Refused(reason)) => {
                    last = DepositError::Refused { reason, server: route.name.clone() };
                    break;
                }
                Err(RpcError::Unreachable) | Err(RpcError::GoDirect) | Err(RpcError::Canceled) => {
                    last = DepositError::Unreachable;
                    break;
                }
                Err(RpcError::Failed(e)) => {
                    last = DepositError::Failed(e);
                    break;
                }
            }
        }
        recips = keys::recipients(config, &eids);
    }
    Err(last)
}

/// A chat frame's bytes, padded to a 512-byte bucket so a server can't tell a
/// "k" from a paragraph by size. Receivers ignore the `_pad` field.
fn padded(frame: &Value) -> Result<Vec<u8>> {
    let mut v = frame.clone();
    let base = serde_json::to_vec(&v)?.len();
    let target = (base + 12).div_ceil(512) * 512;
    if let Some(o) = v.as_object_mut() {
        o.insert("_pad".into(), json!("x".repeat(target.saturating_sub(base + 10))));
    }
    Ok(serde_json::to_vec(&v)?)
}

/// One file of a file deposit: source path, the name the recipient lands it
/// under (relative, `/`-separated), size, mtime (secs).
#[derive(Clone, Debug)]
pub struct DepositFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub mtime: u64,
}

fn files_sig(files: &[DepositFile], dirs: &[String]) -> String {
    let mut h = Sha256::new();
    for f in files {
        h.update(f.path.to_string_lossy().as_bytes());
        h.update([0]);
        h.update(f.name.as_bytes());
        h.update(f.size.to_be_bytes());
        h.update(f.mtime.to_be_bytes());
    }
    for d in dirs {
        h.update(d.as_bytes());
        h.update([1]);
    }
    hex::encode(&h.finalize()[..16])
}

async fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        anyhow::ensure!(!cancel.load(Ordering::SeqCst), "canceled");
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Leave a friend file send (`xfer_id` = the chat transfer id) on a server.
#[allow(clippy::too_many_arguments)]
pub async fn deposit_files(
    net: &IrohState,
    config: &Path,
    peer_id: &str,
    xfer_id: &str,
    transfer_id: &str,
    files: &[DepositFile],
    dirs: &[String],
    top_names: &[String],
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    on_upload_start: &(dyn Fn(&str) + Send + Sync),
    cancel: &AtomicBool,
    only: Option<&[String]>,
) -> Result<Held, DepositError> {
    let ep = net.get().cloned().ok_or(DepositError::Unreachable)?;
    let me = ep.id().to_string();
    let mut eids = person_devices(config, peer_id, &me);
    // Sealed for just these devices (the others got it directly, or will).
    if let Some(only) = only {
        eids.retain(|e| only.contains(e));
    }
    let recips = keys::recipients(config, &eids);
    if recips.is_empty() {
        return Err(DepositError::NoKeys);
    }
    let routes = routes(config, &me, &eids);
    if routes.is_empty() {
        return Err(DepositError::NoRoute);
    }
    let size: u64 = files.iter().map(|f| f.size).sum();
    let sig = files_sig(files, dirs);
    let raw: Vec<(PathBuf, u64, u64)> = files.iter().map(|f| (f.path.clone(), f.size, f.mtime)).collect();
    let go_direct = |done: u64, total: u64| any_live(&eids) && done.saturating_mul(2) < total;

    // An interrupted earlier deposit of these same files resumes where it stopped.
    let mut sealed_for: Vec<&str> = recips.iter().map(|r| r.eid.as_str()).collect();
    sealed_for.sort_unstable();
    let resumable = sent_all(config).into_values().find(|s| {
        let mut to: Vec<&str> = s.to.iter().map(String::as_str).collect();
        to.sort_unstable();
        s.state == "uploading" && s.xfer_id.as_deref() == Some(xfer_id) && s.files_sig.as_deref() == Some(&sig) && s.header.is_some() && s.file_key.is_some()
            // Only a partial sealed for the SAME devices may be resumed.
            && to == sealed_for
    });
    let mut prepared: Option<(seal::Envelope, [u8; 32], String)> = resumable.as_ref().and_then(|s| {
        let env: seal::Envelope = serde_json::from_value(s.header.clone()?).ok()?;
        let fk = seal::key32(s.file_key.as_deref()?).ok()?;
        Some((env, fk, s.server.clone()))
    });
    if is_file_changed(&raw) {
        return Err(DepositError::Failed("A file changed or went missing before it could be sent".into()));
    }
    let mut ordered: Vec<Route> = routes.clone();
    if let Some((_, _, server)) = &prepared {
        ordered.sort_by_key(|r| r.server != *server);
    }
    let mut last = DepositError::Unreachable;
    for route in ordered {
        let (env, fk) = match prepared.take().filter(|(_, _, s)| *s == route.server) {
            Some((env, fk, _)) => (env, fk),
            None => {
                // Ask first: no point hashing 20 GB for a server that can't take it.
                match server_hello(&ep, &route.server).await {
                    None => {
                        last = DepositError::Unreachable;
                        continue;
                    }
                    Some(h) => {
                        let q = &h["quota"];
                        let ct = size + seal::segs_for("file", size) * seal::TAG;
                        let refuse = if h["paused"].as_bool() == Some(true) {
                            Some("paused")
                        } else if q["item_max"].as_u64().is_some_and(|m| size > m) {
                            Some("too_big")
                        } else if q["cap"].as_u64().zip(q["used"].as_u64()).is_some_and(|(cap, used)| used.saturating_add(ct) > cap) {
                            Some("full")
                        } else if q["user_cap"].as_u64().zip(q["user_used"].as_u64()).is_some_and(|(cap, used)| used.saturating_add(ct) > cap) {
                            Some("user_quota")
                        } else {
                            None
                        };
                        if let Some(reason) = refuse {
                            last = DepositError::Refused { reason: reason.into(), server: route.name.clone() };
                            continue;
                        }
                    }
                }
                // Hash once (end-to-end integrity rows on the recipient), then seal.
                let mut manifest = Vec::with_capacity(files.len());
                for f in files {
                    let digest = sha256_file(&f.path, cancel).await.map_err(|e| {
                        if cancel.load(Ordering::SeqCst) { DepositError::Canceled } else { DepositError::Failed(format!("{e:#}")) }
                    })?;
                    manifest.push(json!({"name": f.name, "size": f.size, "mtime": f.mtime, "sha256": digest}));
                }
                let note = file_note(config, peer_id, xfer_id);
                let meta = json!({"xfer_id": xfer_id, "files": manifest, "dirs": dirs, "names": top_names, "note": note});
                let meta = serde_json::to_vec(&meta).map_err(|e| DepositError::Failed(e.to_string()))?;
                let item_id = uuid::Uuid::new_v4().to_string();
                let (env, fk) = seal::seal(ep.secret_key(), &item_id, "file", now(), &recips, &meta, size)
                    .map_err(|e| DepositError::Failed(e.to_string()))?;
                (env, fk)
            }
        };
        with_sent(config, |m| {
            m.insert(env.item_id.clone(), Sent {
                item_id: env.item_id.clone(), server: route.server.clone(), server_name: route.name.clone(), kind: "file".into(),
                peer_id: peer_id.to_owned(), msg_id: None, xfer_id: Some(xfer_id.to_owned()), created_ms: env.created_ms,
                state: "uploading".into(), updated_ms: now(), held_until: 0,
                header: serde_json::to_value(&env).ok(), file_key: Some(seal::b64(&fk)), files_sig: Some(sig.clone()),
                bytes: size, names: top_names.to_vec(), transfer_id: Some(transfer_id.to_owned()),
                delivered_to: vec![], to: env.stanzas.iter().map(|st| st.eid.clone()).collect(), copy: false,
            });
        });
        on_upload_start(&route.name);
        let up = Upload { files: &raw, key: seal::payload_key(&fk, &env.item_id), progress, cancel, go_direct: &go_direct };
        let push = super::push::file_previews(config, &recips, top_names);
        let outcome = deposit_on(&ep, &route.server, &env, &push, Some(&up), true).await;
        let finish = |state: &str, until: u64| {
            with_sent(config, |m| {
                if let Some(s) = m.get_mut(&env.item_id) {
                    s.state = state.to_owned();
                    s.updated_ms = now();
                    s.held_until = until;
                    if state != "uploading" {
                        s.header = None;
                        s.file_key = None;
                    }
                }
            })
        };
        match outcome {
            Ok(until) => {
                finish("held", until);
                log::info!("mailbox: file send held on a Transfer Server ({} bytes)", size);
                return Ok(Held { server: route.server.clone(), name: route.name.clone(), item_id: env.item_id.clone(), until });
            }
            Err(RpcError::GoDirect) => {
                finish("canceled", 0);
                cancel_on(&ep, config, &route.server, &env.item_id).await;
                return Err(DepositError::GoDirect);
            }
            Err(RpcError::Canceled) => {
                // Keep the partial for a resume unless the user canceled for good;
                // the caller decides (a pause resumes, a cancel calls `abandon`).
                return Err(DepositError::Canceled);
            }
            Err(RpcError::Unreachable) => {
                // Resumable: keep "uploading" so a retry continues where it stopped.
                last = DepositError::Unreachable;
                prepared = None;
            }
            Err(RpcError::Recipient(_)) => {
                finish("rejected", 0);
                last = DepositError::Refused { reason: "recipient".into(), server: route.name.clone() };
            }
            Err(RpcError::Refused(reason)) if matches!(reason.as_str(), "busy" | "interrupted" | "storage") => {
                // Transient: keep the partial so the next try resumes it.
                last = DepositError::Unreachable;
                prepared = None;
            }
            Err(RpcError::Refused(reason)) => {
                finish("rejected", 0);
                last = DepositError::Refused { reason, server: route.name.clone() };
            }
            Err(RpcError::Failed(e)) => {
                finish("rejected", 0);
                cancel_on(&ep, config, &route.server, &env.item_id).await;
                return Err(DepositError::Failed(e));
            }
        }
    }
    Err(last)
}

/// A user-canceled file send: drop any partial upload for it on its server.
pub fn abandon(net: &IrohState, config: &Path, xfer_id: &str) {
    let items: Vec<Sent> = sent_all(config).into_values()
        .filter(|s| s.xfer_id.as_deref() == Some(xfer_id) && matches!(s.state.as_str(), "uploading" | "held"))
        .collect();
    if items.is_empty() {
        return;
    }
    with_sent(config, |m| {
        for s in &items {
            if let Some(e) = m.get_mut(&s.item_id) {
                e.state = "canceled".into();
                e.updated_ms = now();
                e.header = None;
                e.file_key = None;
            }
        }
    });
    if let Some(ep) = net.get().cloned() {
        let config = config.to_path_buf();
        tauri::async_runtime::spawn(async move {
            for s in items {
                cancel_on(&ep, &config, &s.server, &s.item_id).await;
            }
        });
    }
}

async fn cancel_on(ep: &iroh::Endpoint, config: &Path, server: &str, item_id: &str) {
    let _ = server_rpc(ep, config, server, &json!({"kind": "mailbox.cancel", "item_id": item_id})).await;
}

/// The chat card we posted for this file send (so the recipient's copy keeps
/// its id, caption and place even when the file lands before the note).
fn file_note(config: &Path, peer_id: &str, xfer_id: &str) -> Value {
    let thread = crate::friends::thread_owner(config, peer_id).map_or_else(|| peer_id.to_owned(), |o| o.id);
    crate::chat::messages(config, &thread).into_iter().chain(crate::chat::messages(config, peer_id))
        .find(|m| m.from_me && m.file_xfer_id.as_deref() == Some(xfer_id))
        .map(|m| json!({"id": m.id, "text": m.text, "ts": m.ts, "seq": m.seq, "files": m.files, "bytes": m.bytes}))
        .unwrap_or(Value::Null)
}

/// The server holding our message `msg_id`, if one is.
pub fn held_server_of(config: &Path, msg_id: &str) -> Option<String> {
    sent_all(config).into_values().find(|s| s.msg_id.as_deref() == Some(msg_id) && s.state == "held").map(|s| s.server)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Unsend {
    /// The server still had it: it's gone, the friend never saw it.
    Removed,
    /// It already reached them (the unsend must travel as an op).
    Delivered,
    /// Couldn't tell (server unreachable) — try again later.
    Unknown,
}

/// Unsend a message a server is holding: delete the server copy.
pub async fn unsend_held(net: &IrohState, config: &Path, msg_id: &str) -> Unsend {
    let Some(ep) = net.get().cloned() else { return Unsend::Unknown };
    let Some(s) = sent_all(config).into_values().find(|s| s.msg_id.as_deref() == Some(msg_id) && s.state == "held") else {
        return Unsend::Unknown;
    };
    let Some(reply) = server_rpc(&ep, config, &s.server, &json!({"kind": "mailbox.cancel", "item_id": s.item_id})).await else { return Unsend::Unknown };
    let outcome = if reply["canceled"].as_bool() == Some(true) {
        Unsend::Removed
    } else if reply["state"].as_str() == Some("delivered") {
        Unsend::Delivered
    } else if reply["ok"].as_bool() == Some(true) {
        // Gone some other way (expired/refused): nothing reached them.
        Unsend::Removed
    } else {
        Unsend::Unknown
    };
    if outcome != Unsend::Unknown {
        with_sent(config, |m| {
            if let Some(e) = m.get_mut(&s.item_id) {
                e.state = if outcome == Unsend::Delivered { "delivered".into() } else { "canceled".into() };
                e.updated_ms = now();
            }
        });
    }
    outcome
}

/// A held chat message reached some of their devices directly after all
/// (`got`). A server copy sealed only for devices that now have it is removed;
/// one still waiting for another device (an iPhone asleep in a pocket) stays,
/// so that device still gets it — the devices that have it just dedupe.
pub fn delivered_directly(net: &IrohState, config: &Path, msg_id: &str, got: &[String]) {
    let items: Vec<Sent> = sent_all(config).into_values()
        .filter(|s| s.msg_id.as_deref() == Some(msg_id) && s.state == "held")
        .filter(|s| s.to.is_empty() || s.to.iter().all(|t| got.contains(t) || s.delivered_to.contains(t)))
        .collect();
    if items.is_empty() {
        return;
    }
    with_sent(config, |m| {
        for s in &items {
            if let Some(e) = m.get_mut(&s.item_id) {
                e.state = "delivered".into();
                e.updated_ms = now();
            }
        }
    });
    if let Some(ep) = net.get().cloned() {
        let config = config.to_path_buf();
        tauri::async_runtime::spawn(async move {
            for s in items {
                cancel_on(&ep, &config, &s.server, &s.item_id).await;
            }
        });
    }
}

/// Server copies of our message `msg_id` still waiting for (some of) the
/// person's devices, or delivered through a server — an edit/unsend/reaction
/// has to follow the message there.
pub fn chat_copies(config: &Path, msg_id: &str) -> Vec<Sent> {
    sent_all(config).into_values()
        .filter(|s| s.kind == "chat" && s.msg_id.as_deref() == Some(msg_id) && matches!(s.state.as_str(), "held" | "delivered") && !s.to.is_empty())
        .collect()
}

/// Take back one held copy (an unsend before that device fetched it).
pub async fn cancel_copy(net: &IrohState, config: &Path, s: &Sent) -> Unsend {
    let Some(ep) = net.get().cloned() else { return Unsend::Unknown };
    let Some(reply) = server_rpc(&ep, config, &s.server, &json!({"kind": "mailbox.cancel", "item_id": s.item_id})).await else { return Unsend::Unknown };
    let outcome = if reply["canceled"].as_bool() == Some(true) {
        Unsend::Removed
    } else if reply["state"].as_str() == Some("delivered") {
        Unsend::Delivered
    } else if reply["ok"].as_bool() == Some(true) {
        Unsend::Removed
    } else {
        Unsend::Unknown
    };
    if outcome != Unsend::Unknown {
        with_sent(config, |m| {
            if let Some(e) = m.get_mut(&s.item_id) {
                e.state = if outcome == Unsend::Delivered { "delivered".into() } else { "canceled".into() };
                e.updated_ms = now();
            }
        });
    }
    outcome
}

// ── receipts ────────────────────────────────────────────────────────────────

/// What changed for one of our held items (for the UI).
#[derive(Debug, Clone)]
pub struct Receipt {
    pub sent: Sent,
    /// A final state, or "held" when only `delivered_to` changed.
    pub state: String,
    /// Recipient devices that have it so far.
    pub delivered_to: Vec<String>,
}

/// Ask each server about our held items. Returns final-state transitions.
pub async fn refresh_status(net: &IrohState, config: &Path) -> Vec<Receipt> {
    let Some(ep) = net.get().cloned() else { return vec![] };
    let pending: Vec<Sent> = sent_all(config).into_values().filter(|s| s.state == "held").collect();
    let mut by_server: HashMap<String, Vec<Sent>> = HashMap::new();
    for s in pending {
        by_server.entry(s.server.clone()).or_default().push(s);
    }
    let mut out = Vec::new();
    for (server, items) in by_server {
        let ids: Vec<&str> = items.iter().map(|s| s.item_id.as_str()).collect();
        let Some(reply) = server_rpc(&ep, config, &server, &json!({"kind": "mailbox.status", "item_ids": ids})).await else {
            // The server's been gone past these items' expiry: they can't
            // arrive through it any more, so let the sender take over again.
            let t = now();
            for s in items.into_iter().filter(|s| s.held_until > 0 && t > s.held_until + 6 * 3600 * 1000) {
                out.push(Receipt { sent: s, state: "lost".into(), delivered_to: vec![] });
            }
            continue;
        };
        if reply["ok"].as_bool() != Some(true) {
            continue;
        }
        let states: HashMap<String, (String, Vec<String>)> = reply["items"].as_array().map(|a| a.iter().filter_map(|v| {
            let to: Vec<String> = v["delivered_to"].as_array().map(|d| d.iter().filter_map(|e| e.as_str().map(String::from)).collect()).unwrap_or_default();
            Some((v["id"].as_str()?.to_owned(), (v["state"].as_str()?.to_owned(), to)))
        }).collect()).unwrap_or_default();
        for s in items {
            let Some((state, delivered_to)) = states.get(&s.item_id) else { continue };
            let final_state = match state.as_str() {
                "delivered" | "expired" | "rejected" | "canceled" => state.clone(),
                "unknown" => "lost".into(),
                // Still held — but maybe some of the recipient's devices have it now.
                _ if delivered_to.iter().any(|d| !s.delivered_to.contains(d)) => "held".into(),
                _ => continue,
            };
            out.push(Receipt { sent: s, state: final_state, delivered_to: delivered_to.clone() });
        }
    }
    if !out.is_empty() {
        with_sent(config, |m| {
            for r in &out {
                if let Some(e) = m.get_mut(&r.sent.item_id) {
                    e.state = r.state.clone();
                    e.updated_ms = now();
                    for d in &r.delivered_to {
                        if !e.delivered_to.contains(d) {
                            e.delivered_to.push(d.clone());
                        }
                    }
                }
            }
        });
    }
    out
}

// ── receiving ───────────────────────────────────────────────────────────────

static FETCHING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static NOTIFIED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

struct FetchGuard(String);
impl Drop for FetchGuard {
    fn drop(&mut self) {
        if let Some(s) = FETCHING.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            s.remove(&self.0);
        }
    }
}

/// A server says it holds something for us: pull it (rate-limited per server).
pub fn on_notify(net: &IrohState, config: &Path, from: &str) -> bool {
    // Only servers we use or our friends use may make us pull.
    let me = net.get().map(|e| e.id().to_string()).unwrap_or_default();
    if crate::block::is_blocked(config, from) || !fetch_candidates(config, &me).iter().any(|c| c == from) {
        return false;
    }
    {
        let mut g = NOTIFIED.lock().unwrap_or_else(|p| p.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        if m.get(from).is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return true;
        }
        m.insert(from.to_owned(), Instant::now());
        if m.len() > 1024 {
            m.retain(|_, t| t.elapsed() < Duration::from_secs(60));
        }
    }
    let Some(app) = net.app.get() else {
        // Headless (tests): the caller drives fetches itself.
        return true;
    };
    let Some(state) = tauri::Manager::try_state::<Arc<IrohState>>(app) else { return true };
    let net = state.inner().clone();
    let config = config.to_path_buf();
    let from = from.to_owned();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = fetch_from(&net, &config, &from).await {
            log::info!("mailbox: fetch after notify failed: {e:#}");
        }
    });
    true
}

/// Servers worth asking for our items: ones we use, plus any a friend told
/// us holds their messages (they may deposit replies there for us).
pub fn fetch_candidates(config: &Path, me: &str) -> Vec<String> {
    let mut out: Vec<String> = servers(config).into_iter().filter(|s| !s.revoked).map(|s| s.eid).collect();
    for (peer, p) in keys::peers(config) {
        // Only what current friends (or our own devices) told us counts.
        if crate::friends::chat_sender(config, &peer).is_none() && !crate::account::is_own_device(config, &peer) {
            continue;
        }
        for s in p.inbox.iter().chain(p.sends.iter()) {
            if !out.contains(&s.eid) {
                out.push(s.eid.clone());
            }
        }
    }
    out.retain(|e| e != me && !crate::block::is_blocked(config, e));
    out.truncate(12);
    out
}

/// Pull from every candidate server in parallel. Returns items landed.
pub async fn fetch_all(net: &Arc<IrohState>, config: &Path) -> usize {
    let Some(ep) = net.get() else { return 0 };
    let me = ep.id().to_string();
    let mut tasks = tokio::task::JoinSet::new();
    for server in fetch_candidates(config, &me) {
        let net = net.clone();
        let config = config.to_path_buf();
        tasks.spawn(async move { fetch_from(&net, &config, &server).await.unwrap_or(0) });
    }
    let mut n = 0;
    while let Some(r) = tasks.join_next().await {
        n += r.unwrap_or(0);
    }
    n
}

/// Pull everything `server` holds for us. Idempotent; one run per server at a time.
pub async fn fetch_from(net: &IrohState, config: &Path, server: &str) -> Result<usize> {
    {
        let mut g = FETCHING.lock().unwrap_or_else(|p| p.into_inner());
        if !g.get_or_insert_with(HashSet::new).insert(server.to_owned()) {
            return Ok(0);
        }
    }
    let _guard = FetchGuard(server.to_owned());
    let ep = net.get().cloned().context("not ready")?;
    let conn = connect(&ep, server, Duration::from_secs(8)).await.context("server unreachable")?;
    let mut landed = 0usize;
    let result: Result<()> = async {
        loop {
            let list = rpc(&conn, &json!({"kind": "mailbox.fetch", "v": super::VERSION})).await?;
            anyhow::ensure!(list["ok"].as_bool() == Some(true), "server refused the fetch");
            let server_name = list["name"].as_str().unwrap_or("Transfer Server").chars().take(64).collect::<String>();
            let items = list["items"].as_array().cloned().unwrap_or_default();
            if list["more"].as_bool() != Some(true) {
                // Held-for-approval entries this server no longer has (expired,
                // canceled, delivered elsewhere) stop asking.
                let listed: HashSet<&str> = items.iter().filter_map(|i| i["item_id"].as_str()).collect();
                prune_pending(config, server, &listed);
            }
            if items.is_empty() {
                break;
            }
            let mut progressed = false;
            for item in &items {
                let id = item["item_id"].as_str().unwrap_or("").to_owned();
                if uuid::Uuid::parse_str(&id).is_err() {
                    continue;
                }
                let decision = if already_seen(config, &id) {
                    Some((true, "duplicate".to_owned()))
                } else {
                    match receive_one(net, config, &ep, &conn, &server_name, item).await {
                        Ok(d) => d,
                        Err(e) => {
                            log::info!("mailbox: couldn't take an item yet: {e:#}");
                            None
                        }
                    }
                };
                if let Some((ok, reason)) = decision {
                    if ok {
                        mark_seen(config, &id);
                        landed += 1;
                    }
                    let _ = rpc(&conn, &json!({"kind": "mailbox.ack", "item_id": id, "ok": ok, "reason": reason})).await;
                    progressed = true;
                }
            }
            if !progressed || list["more"].as_bool() != Some(true) {
                break;
            }
        }
        Ok(())
    }
    .await;
    conn.close(0u32.into(), b"done");
    result?;
    if landed > 0 {
        log::info!("mailbox: received {landed} item(s) from a Transfer Server");
    }
    Ok(landed)
}

/// Take one listed item. Ok(Some((ok, reason))) = ack it; Ok(None) = leave it
/// for later (e.g. disk full); Err = transient.
async fn receive_one(net: &IrohState, config: &Path, ep: &iroh::Endpoint, conn: &iroh::endpoint::Connection, server_name: &str, item: &Value) -> Result<Option<(bool, String)>> {
    let id = item["item_id"].as_str().unwrap_or("").to_owned();
    let me = ep.id().to_string();
    // Small items come inline; files are fetched (resumably) below.
    let header = match item.get("header").filter(|h| h.is_object()) {
        Some(h) => h.clone(),
        None => {
            let (mut send, mut recv) = conn.open_bi().await?;
            write_frame(&mut send, &json!({"kind": "mailbox.get", "item_id": id, "have": u64::MAX})).await?;
            send.finish()?;
            let reply = tokio::time::timeout(Duration::from_secs(20), read_frame_cap(&mut recv, 1 << 20)).await??;
            anyhow::ensure!(reply["ok"].as_bool() == Some(true), "item is gone");
            reply["header"].clone()
        }
    };
    let env: seal::Envelope = match serde_json::from_value(header) {
        Ok(e) => e,
        Err(_) => return Ok(Some((false, "malformed".into()))),
    };
    if env.item_id != id || !env.stanzas.iter().any(|s| s.eid == me) {
        return Ok(Some((false, "malformed".into())));
    }
    let secrets = keys::all_secrets(config);
    anyhow::ensure!(!secrets.is_empty(), "mailbox key unavailable right now");
    let opened = secrets.iter().map(|k| seal::open(&env, &me, k)).find(|r| r.is_ok())
        .unwrap_or_else(|| seal::open(&env, &me, &secrets[0]));
    let (fk, meta) = match opened {
        Ok(v) => v,
        Err(e) => {
            log::warn!("mailbox: refused an item that failed verification: {e:#}");
            return Ok(Some((false, "verification".into())));
        }
    };
    // Only friends may reach us this way; the signature proved who sent it.
    if crate::friends::chat_sender(config, &env.from).is_none() {
        return Ok(Some((false, "unknown sender".into())));
    }
    let meta: Value = match serde_json::from_slice(&meta) {
        Ok(v) => v,
        Err(_) => return Ok(Some((false, "malformed".into()))),
    };
    match env.kind.as_str() {
        "chat" | "op" => {
            if meta["kind"].as_str() != Some("chat") {
                return Ok(Some((false, "malformed".into())));
            }
            let applied = crate::iroh_net::apply_incoming_chat(net, config, &env.from, &meta, Some(server_name), Some(env.created_ms))?;
            if !applied && env.kind == "op" && now().saturating_sub(env.created_ms) < 3 * super::server::DAY_MS {
                // Its message hasn't reached this device yet (another server, or
                // still on its way): leave the edit/reaction there and try later.
                log::info!("mailbox: a held edit/reaction is waiting for its message");
                return Ok(None);
            }
            Ok(Some((true, String::new())))
        }
        "file" => receive_file(net, config, conn, server_name, &env, &fk, &meta).await,
        _ => Ok(Some((false, "malformed".into()))),
    }
}

/// True the first time an item fails integrity (worth one fresh download).
fn integrity_retry(item_id: &str) -> bool {
    static TRIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut g = TRIED.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashSet::new).insert(item_id.to_owned())
}

fn inbox_dir(config: &Path) -> PathBuf {
    config.join("mailbox-in")
}

#[derive(Clone, Debug)]
struct ManifestFile {
    name: String,
    size: u64,
    sha256: String,
}

async fn receive_file(net: &IrohState, config: &Path, conn: &iroh::endpoint::Connection, server_name: &str, env: &seal::Envelope, fk: &[u8; 32], meta: &Value) -> Result<Option<(bool, String)>> {
    let xfer = meta["xfer_id"].as_str().unwrap_or("");
    if uuid::Uuid::parse_str(xfer).is_err() {
        return Ok(Some((false, "malformed".into())));
    }
    let files: Vec<ManifestFile> = meta["files"].as_array().map(|a| a.iter().filter_map(|f| Some(ManifestFile {
        name: f["name"].as_str()?.to_owned(),
        size: f["size"].as_u64()?,
        sha256: f["sha256"].as_str()?.to_owned(),
    })).collect()).unwrap_or_default();
    let dirs: Vec<String> = meta["dirs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let listed = meta["files"].as_array().map(|a| a.len()).unwrap_or(0);
    let sum = files.iter().try_fold(0u64, |n, f| n.checked_add(f.size));
    if files.len() != listed || sum != Some(env.size) || (files.is_empty() && dirs.is_empty()) || files.len() > 100_000 {
        return Ok(Some((false, "malformed".into())));
    }
    let link_id = crate::iroh_net::incoming_chat_id(&env.from, xfer);
    // Already here (a direct copy won the race)? Then the server copy is redundant.
    if direct_landed(config, &link_id) {
        return Ok(Some((true, "duplicate".into())));
    }
    // "Ask before accepting" friends: a held send waits on the server until the
    // user says yes, exactly like a direct offer would.
    let friend = crate::friends::chat_sender(config, &env.from).context("unknown sender")?;
    match decision(config, &link_id) {
        Some(false) => {
            forget_pending(config, &link_id);
            return Ok(Some((false, "declined".into())));
        }
        Some(true) => {}
        None if !friend.auto_accept => {
            let names: Vec<String> = meta["names"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            let fresh = note_pending(config, PendingFile {
                link_id: link_id.clone(), peer_id: friend.id.clone(), server: conn.remote_id().to_string(),
                server_name: server_name.to_owned(), item_id: env.item_id.clone(), bytes: env.size,
                names: if names.is_empty() { files.iter().map(|f| f.name.clone()).collect() } else { names }, at: now(),
            });
            if fresh {
                if let Some(app) = net.app.get() {
                    use tauri::Emitter;
                    let _ = app.emit("mailbox://pending", ());
                }
            }
            return Ok(None);
        }
        None => {}
    }
    let dest = crate::iroh_net::receive_dir(net, config);
    crate::iroh_net::ensure_writable(&dest).await.context("Downloads isn't writable yet")?;
    if let Some((free, _)) = crate::locations::volume_bytes(&dest) {
        if free < env.ct_size().saturating_mul(2).saturating_add(256 << 20) {
            log::warn!("mailbox: not enough space to take a held file yet");
            return Ok(None);
        }
    }
    // 1) Ciphertext to a private resumable partial.
    std::fs::create_dir_all(inbox_dir(config))?;
    let ct_path = inbox_dir(config).join(format!("{}.ct", env.item_id));
    let have = tokio::fs::metadata(&ct_path).await.map(|m| m.len()).unwrap_or(0).min(env.ct_size());
    if have < env.ct_size() {
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(&mut send, &json!({"kind": "mailbox.get", "item_id": env.item_id, "have": have})).await?;
        send.finish()?;
        let reply = tokio::time::timeout(Duration::from_secs(30), read_frame_cap(&mut recv, 1 << 20)).await??;
        anyhow::ensure!(reply["ok"].as_bool() == Some(true), "item is gone");
        anyhow::ensure!(reply["ct_size"].as_u64() == Some(env.ct_size()) && reply["from_offset"].as_u64() == Some(have), "size mismatch");
        let mut out = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&ct_path).await?;
        out.set_len(have).await?;
        out.seek(std::io::SeekFrom::Start(have)).await?;
        let mut remaining = env.ct_size() - have;
        let mut buf = vec![0u8; 256 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = tokio::time::timeout(Duration::from_secs(90), recv.read(&mut buf[..want])).await
                .context("download stalled")??
                .context("download ended early")?;
            out.write_all(&buf[..n]).await?;
            remaining -= n as u64;
        }
        out.sync_all().await?;
    }
    // 2) Decrypt, verify every file's sha256, publish into Downloads.
    let item_id = env.item_id.clone();
    let segs = env.segs;
    let size = env.size;
    let key = *fk;
    let files2 = files.clone();
    let dirs2 = dirs.clone();
    let dest2 = dest.clone();
    let ct2 = ct_path.clone();
    let landed = tokio::task::spawn_blocking(move || decrypt_and_land(&ct2, &item_id, &key, size, segs, &files2, &dirs2, &dest2)).await?;
    let landed = match landed {
        Ok(l) => l,
        Err(e) => {
            let corrupt = format!("{e:#}").contains("authentication") || format!("{e:#}").contains("integrity");
            if corrupt {
                let _ = std::fs::remove_file(&ct_path);
                // Our own partial may be what's damaged (a crash mid-write):
                // download it once more from scratch before turning it down.
                if integrity_retry(&env.item_id) {
                    log::warn!("mailbox: a held file failed its integrity check; downloading it again");
                    return Err(e);
                }
                log::warn!("mailbox: a held file failed its integrity check twice; refusing it");
                return Ok(Some((false, "integrity".into())));
            }
            return Err(e);
        }
    };
    let _ = std::fs::remove_file(&ct_path);
    forget_pending(config, &link_id);
    let names: Vec<String> = meta["names"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    crate::iroh_net::land_server_files(net, config, &env.from, xfer, &meta["note"], &names, &files.iter().map(|f| (f.name.clone(), f.size)).collect::<Vec<_>>(), &dirs, &landed, &dest, server_name, env.created_ms);
    Ok(Some((true, String::new())))
}

/// Stream-decrypt `ct_path` into the manifest's files under `dest`. Returns the
/// landed items: (manifest key "file:i:name" / "dir:name", path).
#[allow(clippy::too_many_arguments)]
fn decrypt_and_land(ct_path: &Path, item_id: &str, fk: &[u8; 32], size: u64, segs: u64, files: &[ManifestFile], dirs: &[String], dest: &Path) -> Result<Vec<(String, PathBuf)>> {
    REUSED.with(|r| r.borrow_mut().clear());
    let mut landed: Vec<(String, PathBuf)> = Vec::new();
    let mut staged: Vec<PathBuf> = Vec::new();
    let result = land_all(ct_path, item_id, fk, size, segs, files, dirs, dest, &mut landed, &mut staged);
    for p in &staged {
        let _ = std::fs::remove_file(p);
    }
    if let Err(e) = result {
        // Never leave half a delivery: remove what this pass published (never a
        // byte-identical copy that was already there and got reused).
        for (key, path) in &landed {
            if key.starts_with("file:") && !staged_reused(path) {
                let _ = std::fs::remove_file(path);
            }
        }
        return Err(e);
    }
    Ok(landed)
}

thread_local! {
    /// Files a landing pass reused rather than wrote (never rolled back).
    static REUSED: std::cell::RefCell<HashSet<PathBuf>> = std::cell::RefCell::new(HashSet::new());
}
fn note_reused(p: &Path) {
    REUSED.with(|r| r.borrow_mut().insert(p.to_path_buf()));
}
fn staged_reused(p: &Path) -> bool {
    REUSED.with(|r| r.borrow().contains(p))
}

struct OpenOut {
    file: std::fs::File,
    part: PathBuf,
    hash: Sha256,
    written: u64,
}

fn open_out(files: &[ManifestFile], i: usize, dest: &Path, staged: &mut Vec<PathBuf>) -> Result<OpenOut> {
    let rel = crate::iroh_net::receive_rel(&files[i].name);
    let natural = crate::iroh_net::ensure_parent_or_flat(dest, &rel);
    let dir = natural.parent().unwrap_or(dest).to_path_buf();
    let part = dir.join(format!(".dropbeam-mbx-{}.part", uuid::Uuid::new_v4()));
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
    staged.push(part.clone());
    Ok(OpenOut { file, part, hash: Sha256::new(), written: 0 })
}

fn close_out(files: &[ManifestFile], i: usize, o: OpenOut, dest: &Path, landed: &mut Vec<(String, PathBuf)>, staged: &mut Vec<PathBuf>) -> Result<()> {
    o.file.sync_all()?;
    drop(o.file);
    anyhow::ensure!(o.written == files[i].size, "integrity: size mismatch");
    anyhow::ensure!(hex::encode(o.hash.finalize()) == files[i].sha256.to_lowercase(), "integrity: sha256 mismatch");
    let rel = crate::iroh_net::receive_rel(&files[i].name);
    let natural = crate::iroh_net::ensure_parent_or_flat(dest, &rel);
    // The same file already landed here (a direct copy of this send, or the
    // user's own identical file): keep that one instead of adding "name (2)".
    let path = match crate::iroh_net::identical_landed(&o.part, &natural) {
        Some(existing) => {
            let _ = std::fs::remove_file(&o.part);
            note_reused(&existing);
            existing
        }
        None => crate::iroh_net::publish_unique(&o.part, &natural)?,
    };
    staged.retain(|p| p != &o.part);
    landed.push((format!("file:{i}:{}", files[i].name), path));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn land_all(ct_path: &Path, item_id: &str, fk: &[u8; 32], size: u64, segs: u64, files: &[ManifestFile], dirs: &[String], dest: &Path,
    landed: &mut Vec<(String, PathBuf)>, staged: &mut Vec<PathBuf>) -> Result<()> {
    use std::io::{Read, Write};
    let pk = seal::payload_key(fk, item_id);
    let mut ct = std::fs::File::open(ct_path)?;
    for d in dirs {
        let path = dest.join(crate::iroh_net::receive_rel(d));
        std::fs::create_dir_all(&path)?;
        landed.push((format!("dir:{d}"), path));
    }
    let mut idx = 0usize;
    let mut out: Option<OpenOut> = None;
    // Zero-length files at the cursor land immediately.
    let skip_empty = |idx: &mut usize, landed: &mut Vec<(String, PathBuf)>, staged: &mut Vec<PathBuf>| -> Result<()> {
        while *idx < files.len() && files[*idx].size == 0 {
            let o = open_out(files, *idx, dest, staged)?;
            close_out(files, *idx, o, dest, landed, staged)?;
            *idx += 1;
        }
        Ok(())
    };
    skip_empty(&mut idx, landed, staged)?;
    for i in 0..segs {
        let plen = seal::seg_len(size, segs, i) as usize;
        let mut buf = vec![0u8; plen + seal::TAG as usize];
        ct.read_exact(&mut buf)?;
        pk.open_segment(i, i + 1 == segs, &mut buf)?;
        let mut at = 0usize;
        while at < buf.len() {
            anyhow::ensure!(idx < files.len(), "integrity: more data than files");
            if out.is_none() {
                out = Some(open_out(files, idx, dest, staged)?);
            }
            let o = out.as_mut().expect("opened above");
            let left = (files[idx].size - o.written) as usize;
            let take = left.min(buf.len() - at);
            o.file.write_all(&buf[at..at + take])?;
            o.hash.update(&buf[at..at + take]);
            o.written += take as u64;
            at += take;
            if o.written == files[idx].size {
                let done = out.take().expect("open");
                close_out(files, idx, done, dest, landed, staged)?;
                idx += 1;
                skip_empty(&mut idx, landed, staged)?;
            }
        }
    }
    anyhow::ensure!(idx == files.len() && out.is_none(), "integrity: payload ended early");
    Ok(())
}

// ── held files waiting for the user's OK ("Ask before accepting") ──────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingFile {
    /// The chat card's transfer link ("receive:<peer>:<xfer>").
    pub link_id: String,
    pub peer_id: String,
    pub server: String,
    pub server_name: String,
    pub item_id: String,
    pub bytes: u64,
    pub names: Vec<String>,
    pub at: u64,
}

fn pending_path(config: &Path) -> PathBuf {
    config.join("mailbox-pending.json")
}
fn decisions_path(config: &Path) -> PathBuf {
    config.join("mailbox-decisions.json")
}

pub fn pending_files(config: &Path) -> Vec<PendingFile> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let map: BTreeMap<String, PendingFile> = read_store(&pending_path(config));
    map.into_values().collect()
}

/// Returns true when this is news (first time we see it waiting).
fn note_pending(config: &Path, p: PendingFile) -> bool {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: BTreeMap<String, PendingFile> = read_store(&pending_path(config));
    let fresh = !map.contains_key(&p.link_id);
    if fresh {
        map.insert(p.link_id.clone(), p);
        write_store(&pending_path(config), &map);
    }
    fresh
}

fn prune_pending(config: &Path, server: &str, listed: &HashSet<&str>) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: BTreeMap<String, PendingFile> = read_store(&pending_path(config));
    let before = map.len();
    map.retain(|_, p| p.server != server || listed.contains(p.item_id.as_str()));
    if map.len() != before {
        write_store(&pending_path(config), &map);
    }
}

fn forget_pending(config: &Path, link_id: &str) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: BTreeMap<String, PendingFile> = read_store(&pending_path(config));
    if map.remove(link_id).is_some() {
        write_store(&pending_path(config), &map);
    }
    let mut d: HashMap<String, (bool, u64)> = read_store(&decisions_path(config));
    if d.remove(link_id).is_some() {
        write_store(&decisions_path(config), &d);
    }
}

fn decision(config: &Path, link_id: &str) -> Option<bool> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let d: HashMap<String, (bool, u64)> = read_store(&decisions_path(config));
    d.get(link_id).map(|(ok, _)| *ok)
}

/// The user accepted or declined a held file send; pull it (or refuse it) now.
pub fn decide_file(config: &Path, link_id: &str, accept: bool) {
    {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut d: HashMap<String, (bool, u64)> = read_store(&decisions_path(config));
        let t = now();
        d.insert(link_id.to_owned(), (accept, t));
        d.retain(|_, (_, at)| *at + SEEN_MS > t);
        write_store(&decisions_path(config), &d);
    }
    fetch_soon();
}

// ── direct-landed ledger ────────────────────────────────────────────────────

fn landed_path(config: &Path) -> PathBuf {
    config.join("mailbox-landed.json")
}

/// A friend's linked file send finished landing DIRECTLY (receive side), so a
/// server copy of the same send is redundant.
pub fn note_direct_landed(config: &Path, link_id: &str) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map: HashMap<String, u64> = read_store(&landed_path(config));
    let t = now();
    map.insert(link_id.to_owned(), t);
    map.retain(|_, at| *at + SEEN_MS > t);
    write_store(&landed_path(config), &map);
}

fn direct_landed(config: &Path, link_id: &str) -> bool {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let map: HashMap<String, u64> = read_store(&landed_path(config));
    map.contains_key(link_id)
}

// ── background ──────────────────────────────────────────────────────────────

static WAKE: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
fn wake_cell() -> &'static tokio::sync::Notify {
    WAKE.get_or_init(tokio::sync::Notify::new)
}
static FETCH_NOW: AtomicBool = AtomicBool::new(false);

/// Re-check receipts soon (after a deposit, a new server, …).
pub fn wake() {
    wake_cell().notify_one();
}

/// Pull from servers right away (app foregrounded, network changed).
pub fn fetch_soon() {
    FETCH_NOW.store(true, Ordering::SeqCst);
    wake_cell().notify_one();
}

/// Startup fetch, then receipts every 5 minutes while anything is held, and a
/// fetch sweep every 30 minutes (servers also poke us when we come online).
pub fn spawn(net: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            if net.get().is_some() && crate::iroh_net::location_config(&net).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
        let mut last_fetch = Instant::now() - Duration::from_secs(3600);
        loop {
            let Ok(config) = crate::iroh_net::location_config(&net) else { break };
            if FETCH_NOW.swap(false, Ordering::SeqCst) || last_fetch.elapsed() > Duration::from_secs(1800) {
                last_fetch = Instant::now();
                fetch_all(&net, &config).await;
            }
            let receipts = refresh_status(&net, &config).await;
            crate::iroh_net::apply_receipts(&net, &config, &receipts);
            let rotated = super::keys::maybe_rotate(&config);
            if super::push::import_token_file(&config) || rotated {
                // Friends need our push key to seal previews for us.
                if let Some(app) = net.app.get() {
                    if let Some(n) = tauri::Manager::try_state::<Arc<IrohState>>(app) {
                        crate::iroh_net::broadcast_profile(app.clone(), n.inner().clone());
                    }
                }
            }
            if check_introduced(&net, &config).await {
                if let Some(app) = net.app.get() {
                    use tauri::Emitter;
                    let _ = app.emit("mailbox://servers", ());
                    if let Some(n) = tauri::Manager::try_state::<Arc<IrohState>>(app) {
                        crate::iroh_net::broadcast_profile(app.clone(), n.inner().clone());
                    }
                }
            }
            push_members(&net, &config).await;
            super::push::register_everywhere(&net, &config).await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(300)) => {},
                _ = wake_cell().notified() => {},
            }
        }
    });
}
