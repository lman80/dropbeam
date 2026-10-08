//! Account recovery code (UX D1 / 3.3): the account key written down as words.
//!
//! An account is its key (link.rs) and nothing else: there is no server and no
//! password. The recovery code is that key in a form a person can keep on paper:
//!   * 12 words for an account minted by this build (the key is derived from 16
//!     random bytes, `key_from_seed`);
//!   * 24 words for an account minted before (its 32 key bytes, verbatim). Those
//!     keys can't be shortened, and swapping in a new key would make every
//!     friend lose track of the person, so they keep their longer code.
//!     Both are BIP39 word lists with BIP39's checksum, so a typo is caught.
//!
//! Restoring on a new device installs the same account key under a NEW endpoint
//! (device) key. The endpoint key is deliberately not derived: two live devices
//! sharing one endpoint id (the "lost" phone turns up, or a thief has it) would
//! fight over every connection and be indistinguishable to friends. A new
//! endpoint that proves the account is exactly what every build already treats
//! as "another device of this friend" (friends.rs `AddedDevice`).
//!
//! What comes back, and how:
//!   * Finding each other. The new device knows no one, and friends only know
//!     the lost devices' ids. A restored device publishes a small signed record
//!     under the ACCOUNT key on the same pkarr relay iroh already publishes
//!     addresses to (`_dropbeam` TXT: the device ids to reach). Every device
//!     looks up its friends' accounts every few hours and greets any new device
//!     it finds there. Only someone who already knows the account key's public
//!     half (friends, own devices) can look it up.
//!   * Friends. While friends with this build exchange hellos, each side hands
//!     the other a "vouch": its account's signature over the other's device id.
//!     A friend greeting the restored device presents the vouch it holds; it
//!     verifies against the recovered key, so former friends come back without a
//!     request — and nobody else can claim to be one (a stranger's hello is a
//!     friend request as always).
//!   * Chat history. The restored device asks each friend who comes back for
//!     their copy of the conversation (`restore-sync`); the friend only answers a
//!     device proving the account of someone it already has as a friend.
//!   * Devices from before and shared folders: the friend also lists the other
//!     devices it knew for the person (shown as "from before", one tap removes
//!     them so a lost or stolen one can't keep speaking as the user) and the
//!     folders it shares with them (so the user can ask to be invited again).
//!
//! The words are the account: never logged, never sent over the network, wiped
//! from memory after use where Rust owns them.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;
use zeroize::{Zeroize, Zeroizing};

use crate::{chat, friends, iroh_net::{self, IrohState}, AppState};

// ── what people read ───────────────────────────────────────────────────────

pub(crate) const ALREADY_THIS: &str = "This device already uses that recovery code — nothing to restore.";
pub(crate) const LINKED_ELSEWHERE: &str = "This device is already linked with your other devices. To restore a different account here, first open Settings → Devices and choose Remove This Device from My Devices.";
const NOT_READY: &str = "DropBeam is still connecting — try again in a moment.";
const WRONG_WORDS: &str = "Those words don’t add up — one of them is probably mistyped. Check each word against your paper.";

/// Restored devices keep looking for friends (publishing where to find them,
/// asking for chat history) this long.
const RESTORE_WINDOW_MS: u64 = 30 * 24 * 3600 * 1000;
/// A QR of the code reads `dropbeamrecover1:word word …`.
pub(crate) const QR_PREFIX: &str = "dropbeamrecover1:";

// ── the word list (BIP39 English, sha256 2f5eed53…dbda) ───────────────────

const WORDLIST: &str = include_str!("recovery/bip39-english.txt");

fn words() -> &'static [&'static str] {
    static W: OnceLock<Vec<&'static str>> = OnceLock::new();
    W.get_or_init(|| {
        let w: Vec<&str> = WORDLIST.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        assert_eq!(w.len(), 2048, "BIP39 list");
        w
    })
}

/// The index of `word`: the exact word, or (BIP39 words are unique in their
/// first four letters) any longer-than-three-letter start of exactly one word.
fn lookup(word: &str) -> Option<usize> {
    let list = words();
    if let Ok(i) = list.binary_search(&word) { return Some(i); }
    if word.len() < 4 { return None; }
    let mut hits = list.iter().enumerate().filter(|(_, w)| w.starts_with(word));
    let (i, _) = hits.next()?;
    hits.next().is_none().then_some(i)
}

/// The words of `entropy` (16 bytes → 12 words, 32 → 24): BIP39 encoding,
/// the entropy's bits followed by the first len/4 bits of its SHA-256.
pub(crate) fn encode(entropy: &[u8]) -> Zeroizing<Vec<String>> {
    assert!(entropy.len() == 16 || entropy.len() == 32);
    let check_bits = entropy.len() / 4;
    let mut bits: Zeroizing<Vec<bool>> = Zeroizing::new(Vec::with_capacity(entropy.len() * 8 + check_bits));
    for b in entropy { for i in (0..8).rev() { bits.push(b >> i & 1 == 1); } }
    let hash = Sha256::digest(entropy);
    for i in 0..check_bits { bits.push(hash[i / 8] >> (7 - i % 8) & 1 == 1); }
    Zeroizing::new(bits.chunks(11).map(|c| {
        let idx = c.iter().fold(0usize, |acc, b| acc << 1 | usize::from(*b));
        words()[idx].to_owned()
    }).collect())
}

/// Split what someone typed (or a scanned QR) into lowercase words: any
/// punctuation, numbering ("1. abandon") and line breaks are separators.
fn split(text: &str) -> Zeroizing<Vec<String>> {
    let lower = Zeroizing::new(text.trim().to_lowercase());
    let body = lower.strip_prefix(QR_PREFIX).unwrap_or(&lower);
    Zeroizing::new(body.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty()).map(str::to_owned).collect())
}

/// Live feedback while words are typed, and the final check.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Words entered so far.
    pub count: usize,
    /// Positions (0-based) of words that aren't on the list.
    pub unknown: Vec<usize>,
    /// 12 or 24 known words were entered.
    pub complete: bool,
    /// …and their checksum matches: these words can be restored.
    pub valid: bool,
    /// What to tell the person, if anything is wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

pub(crate) fn check(text: &str) -> Check {
    let ws = split(text);
    let unknown: Vec<usize> = ws.iter().enumerate().filter(|(_, w)| lookup(w).is_none()).map(|(i, _)| i).collect();
    let count = ws.len();
    let complete = (count == 12 || count == 24) && unknown.is_empty();
    let valid = complete && decode(text).is_ok();
    let problem = if let Some(i) = unknown.first() {
        Some(format!("Word {} isn’t one of the recovery words. Check its spelling.", i + 1))
    } else if count > 24 {
        Some("That’s more than 24 words. Your code has 12 or 24.".into())
    } else if complete && !valid {
        Some(WRONG_WORDS.into())
    } else {
        None
    };
    Check { count, unknown, complete, valid, problem }
}

/// The bytes behind a 12- or 24-word code (checksum verified).
pub(crate) fn decode(text: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let ws = split(text);
    if ws.len() != 12 && ws.len() != 24 {
        return Err(format!("Your recovery code has 12 or 24 words — {} were entered.", ws.len()));
    }
    let mut idx: Zeroizing<Vec<usize>> = Zeroizing::new(Vec::with_capacity(ws.len()));
    for (n, w) in ws.iter().enumerate() {
        idx.push(lookup(w).ok_or_else(|| format!("Word {} isn’t one of the recovery words. Check its spelling.", n + 1))?);
    }
    let mut bits: Zeroizing<Vec<bool>> = Zeroizing::new(Vec::with_capacity(ws.len() * 11));
    for i in idx.iter() { for b in (0..11).rev() { bits.push(i >> b & 1 == 1); } }
    let ent_bits = ws.len() * 11 * 32 / 33;
    let mut entropy: Zeroizing<Vec<u8>> = Zeroizing::new(bits[..ent_bits].chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, b| acc << 1 | u8::from(*b))).collect());
    let hash = Sha256::digest(entropy.as_slice());
    let ok = bits[ent_bits..].iter().enumerate().all(|(i, b)| (hash[i / 8] >> (7 - i % 8) & 1 == 1) == *b);
    if !ok {
        entropy.zeroize();
        return Err(WRONG_WORDS.into());
    }
    Ok(entropy)
}

/// The account key a 12-word seed stands for. Domain-separated, so the same
/// 16 bytes used anywhere else (another wallet's words) give a different key.
pub(crate) fn key_from_seed(seed: &[u8; 16]) -> iroh::SecretKey {
    let mut h = Sha256::new();
    h.update(b"dropbeam-account-from-words/1");
    h.update(seed);
    let mut bytes: [u8; 32] = h.finalize().into();
    let key = iroh::SecretKey::from_bytes(&bytes);
    bytes.zeroize();
    key
}

/// The key (and, for 12 words, the seed) a decoded code stands for.
fn key_from_entropy(entropy: &[u8]) -> (iroh::SecretKey, Option<Zeroizing<[u8; 16]>>) {
    match entropy.len() {
        16 => {
            let seed = Zeroizing::new(<[u8; 16]>::try_from(entropy).expect("16 bytes"));
            (key_from_seed(&seed), Some(seed))
        }
        _ => {
            let mut bytes = <[u8; 32]>::try_from(entropy).expect("32 bytes");
            let key = iroh::SecretKey::from_bytes(&bytes);
            bytes.zeroize();
            (key, None)
        }
    }
}

/// This device's recovery words (minting a 12-word account if it has none).
pub(crate) fn code_words(dir: &Path) -> Result<Zeroizing<Vec<String>>, String> {
    let (key, seed) = crate::link::account_for_recovery(dir)?;
    Ok(match seed {
        Some(s) => encode(s.as_slice()),
        None => { let bytes = Zeroizing::new(key.to_bytes()); encode(bytes.as_slice()) }
    })
}

// ── recovery bookkeeping (recovery.json) ───────────────────────────────────

#[derive(Default, Serialize, Deserialize, Clone, Debug)]
struct Book {
    /// The account whose code the user confirmed writing down (cleared in
    /// effect when the account changes: a code for another account).
    #[serde(default)]
    saved_for: String,
    #[serde(default)]
    saved_at: u64,
    /// When the user last put off saving it (the reminder waits a while).
    #[serde(default)]
    later_at: u64,
    /// When this device restored `restored_account` from words.
    #[serde(default)]
    restored_at: u64,
    #[serde(default)]
    restored_account: String,
    /// Friends' devices that sent back their copy of the history (ms).
    #[serde(default)]
    synced: HashMap<String, u64>,
    /// Unanswered history requests per friend device: (tries, last try ms), for backoff.
    #[serde(default)]
    tries: HashMap<String, (u32, u64)>,
    /// The person's other devices friends knew: from before the restore.
    #[serde(default)]
    old_devices: HashMap<String, OldDevice>,
    /// Old devices the user said to keep (not lost).
    #[serde(default)]
    kept: HashSet<String>,
    /// Shared folders friends said they had with the person.
    #[serde(default)]
    folders: Vec<FolderNote>,
    /// Names of friends who came back (for the summary).
    #[serde(default)]
    returned: Vec<String>,
    /// Last rendezvous publish (ms).
    #[serde(default)]
    published_at: u64,
}

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OldDevice {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// The friend who knew it.
    #[serde(default)]
    pub via: String,
}

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderNote {
    pub name: String,
    pub with: String,
}

static LOCK: Mutex<()> = Mutex::new(());

fn book_path(dir: &Path) -> PathBuf {
    dir.join("recovery.json")
}
fn read_book(dir: &Path) -> Book {
    std::fs::read(book_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn with_book<T>(dir: &Path, f: impl FnOnce(&mut Book) -> T) -> T {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut b = read_book(dir);
    let out = f(&mut b);
    if let Ok(bytes) = serde_json::to_vec(&b) {
        if let Err(e) = crate::settings::write_atomic(&book_path(dir), &bytes) {
            log::warn!("recovery: cannot save recovery.json: {e}");
        }
    }
    out
}

/// This device restored its account from words recently, and still has it.
pub(crate) fn restore_active(dir: &Path) -> bool {
    let b = read_book(dir);
    b.restored_at > 0 && chat::now_ms().saturating_sub(b.restored_at) < RESTORE_WINDOW_MS
        && crate::account::my_pub(dir).is_some_and(|a| a == b.restored_account)
}

// ── vouches: the account's word that a friend's device is a friend ────────

fn vouch_message(eid: &str) -> String {
    format!("dropbeam-friend-vouch/1|{eid}")
}

fn vouches_path(dir: &Path) -> PathBuf {
    dir.join("friend-vouches.json")
}
fn read_vouches(dir: &Path) -> HashMap<String, Value> {
    std::fs::read(vouches_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Extra fields for a hello (or hello reply) to `peer`: our account's vouch
/// for them, if they're a friend, and the vouch THEIR account gave us, so a
/// restored device of theirs can recognize us. `peer_account` names their
/// account when we have no record for `peer` yet (a rendezvous greeting).
pub(crate) fn hello_fields(dir: &Path, peer: &str, peer_account: Option<&str>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let record = friends::chat_sender(dir, peer);
    let mine = crate::account::my_pub(dir);
    if let Some(mine) = &mine {
        // Only a friend (not a request, not one of our own devices, not blocked).
        if record.is_some() && !crate::account::is_own_device(dir, peer) {
            if let Some(sig) = crate::link::sign_with_account(dir, &vouch_message(peer)) {
                out.insert("account_pub".into(), json!(mine));
                out.insert("vouch".into(), json!(sig));
            }
        }
    }
    let theirs = peer_account.map(str::to_owned).or_else(|| {
        friends::load_raw(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(peer)).and_then(|f| f.account_pub)
    });
    if let Some(account) = theirs.filter(|a| Some(a) != mine.as_ref()) {
        if let Some(sig) = read_vouches(dir).get(&account).and_then(|v| v["sig"].as_str()) {
            out.insert("your_vouch".into(), json!(sig));
        }
    }
    out
}

/// Keep the vouch a friend's account gave this device (`me`), from a hello or
/// a hello reply. Verified before it is kept; only that account could make it.
pub(crate) fn store_vouch(dir: &Path, me: &str, v: &Value) -> bool {
    let (Some(account), Some(sig)) = (v["account_pub"].as_str(), v["vouch"].as_str()) else { return false };
    if crate::account::my_pub(dir).as_deref() == Some(account) || crate::block::account_blocked(dir, account) { return false; }
    if !crate::link::verify_account(account, sig, &vouch_message(me)) { return false; }
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut all = read_vouches(dir);
    if all.get(account).and_then(|x| x["sig"].as_str()) == Some(sig) { return false; }
    if all.len() >= 512 && !all.contains_key(account) { return false; }
    all.insert(account.to_owned(), json!({"sig": sig, "at": chat::now_ms()}));
    if let Ok(bytes) = serde_json::to_vec(&all) {
        let _ = crate::settings::write_atomic(&vouches_path(dir), &bytes);
    }
    true
}

/// A hello from `who` carries this account's own vouch for that device: it was
/// one of the user's friends before the restore.
pub(crate) fn vouched_by_me(dir: &Path, who: &str, req: &Value) -> bool {
    let (Some(mine), Some(sig)) = (crate::account::my_pub(dir), req["your_vouch"].as_str()) else { return false };
    crate::link::verify_account(&mine, sig, &vouch_message(who))
}

/// A former friend came back by their vouch (friends.rs added them).
pub(crate) fn note_returned(dir: &Path, name: &str) {
    if !restore_active(dir) { return; }
    let name = friends::sanitize_display_name(name, "A friend");
    with_book(dir, |b| {
        if !b.returned.contains(&name) && b.returned.len() < 200 { b.returned.push(name); }
    });
    nudge();
}

// ── restore-sync: a friend sends back their copy ───────────────────────────

/// Messages a friend sends back (newest kept) and the frame budget for them.
const SYNC_MESSAGES: usize = 500;
const SYNC_BUDGET: usize = 8 << 20;
const SYNC_FRAME_CAP: usize = 12 << 20;

/// Friend side: answer a device proving the account of someone who is already
/// a friend here with our copy of the conversation, the person's other devices
/// we know and the folders we share with them. Anyone else gets a plain no.
pub(crate) fn serve_restore(dir: &Path, who: &str, req: &Value) -> Value {
    let no = || json!({"kind": "restore-sync-no"});
    let Some(account) = req["account_pub"].as_str().filter(|key| {
        crate::link::verify_account(key, req["account_sig"].as_str().unwrap_or(""), who)
    }) else { return no() };
    if crate::account::my_pub(dir).as_deref() == Some(account) || friends::is_revoked(dir, account, who)
        || crate::block::is_blocked(dir, who) || crate::block::account_blocked(dir, account) {
        return no();
    }
    if !answer_budget(dir, who) { return no(); }
    let records: Vec<crate::models::Friend> = friends::load(dir).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account) && f.endpoint_id.is_some()).collect();
    if records.is_empty() { return no(); }
    // One person, one conversation (threads may still sit on several records).
    let mut seen = HashSet::new();
    let mut messages: Vec<chat::ChatMessage> = records.iter().flat_map(|f| chat::messages(dir, &f.id))
        .filter(|m| seen.insert((m.id.clone(), m.from_me))).collect();
    messages.sort_by_key(|m| (m.ts, m.seq, m.id.clone()));
    if messages.len() > SYNC_MESSAGES { messages.drain(..messages.len() - SYNC_MESSAGES); }
    for m in &mut messages {
        m.path = None;
        m.held_on = None;
        m.server_note = None;
        m.via = None;
        m.deliveries.clear();
    }
    while !messages.is_empty() && serde_json::to_vec(&messages).map_or(0, |v| v.len()) > SYNC_BUDGET {
        let drop = (messages.len() / 4).max(1);
        messages.drain(..drop);
    }
    let devices: Vec<Value> = records.iter().filter(|f| f.endpoint_id.as_deref() != Some(who))
        .filter(|f| !friends::is_revoked(dir, account, f.endpoint_id.as_deref().unwrap_or("")))
        .take(16)
        .map(|f| json!({"eid": f.endpoint_id, "kind": f.device_kind, "os": f.device_os, "model": f.device_model}))
        .collect();
    let eids: HashSet<&str> = records.iter().filter_map(|f| f.endpoint_id.as_deref()).collect();
    let mut folders: Vec<String> = crate::pairing::load(dir).into_iter()
        .filter(|p| p.endpoint_id.as_deref().is_some_and(|e| eids.contains(e)))
        .filter_map(|p| Path::new(&p.folder).file_name().map(|n| n.to_string_lossy().chars().take(80).collect()))
        .collect();
    folders.sort();
    folders.dedup();
    folders.truncate(50);
    log::info!("recovery: sent {} messages back to a friend's restored device", messages.len());
    json!({"kind": "restore-sync-ok", "v": 1, "messages": messages, "devices": devices, "folders": folders})
}

/// At most one full answer per device every few minutes.
fn answer_budget(dir: &Path, who: &str) -> bool {
    static RECENT: Mutex<Option<HashMap<(PathBuf, String), std::time::Instant>>> = Mutex::new(None);
    let mut g = RECENT.lock().unwrap_or_else(|p| p.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, t| t.elapsed() < Duration::from_secs(300));
    let k = (dir.to_path_buf(), who.to_owned());
    if map.contains_key(&k) || map.len() > 256 { return false; }
    map.insert(k, std::time::Instant::now());
    true
}

/// What one friend's answer added here.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Applied {
    pub messages: usize,
    pub devices: usize,
    pub folders: usize,
}

/// Restored side: take a friend's answer. Their copy is from THEIR side of the
/// conversation, so who-sent-what flips; it is merged like an own-device sync
/// (no notifications, no unread), and nothing in it is trusted beyond text.
pub(crate) fn apply_restore(dir: &Path, me: &str, friend_eid: &str, reply: &Value) -> Result<Applied, String> {
    if reply["kind"] != "restore-sync-ok" { return Err("not answered".into()); }
    let friend = friends::chat_sender(dir, friend_eid).ok_or("not a friend")?;
    let mut out = Applied::default();
    let mut msgs: Vec<chat::ChatMessage> = reply["messages"].as_array().map(|a| a.iter().take(SYNC_MESSAGES * 2)
        .filter_map(|v| serde_json::from_value::<chat::ChatMessage>(v.clone()).ok()).collect()).unwrap_or_default();
    for m in &mut msgs {
        m.from_me = !m.from_me;
        for r in &mut m.reactions { r.from_me = !r.from_me; }
        for r in &mut m.reaction_revs { r.from_me = !r.from_me; }
        m.status = m.from_me.then(|| "delivered".to_owned());
        m.path = None;
        m.held_on = None;
        m.server_note = None;
        m.via = None;
        m.deliveries.clear();
        m.file_xfer_id = None;
    }
    if !msgs.is_empty() {
        out.messages = chat::merge_synced(dir, &friend.id, msgs);
    }
    let own: HashSet<String> = crate::account::own_devices(dir).into_iter().filter_map(|f| f.endpoint_id).collect();
    let devices: Vec<(String, OldDevice)> = reply["devices"].as_array().map(|a| a.iter().take(16).filter_map(|d| {
        let eid = d["eid"].as_str()?;
        let parsed = eid.parse::<iroh::EndpointId>().ok()?;
        (parsed.to_string() == eid && eid != me && !own.contains(eid) && !crate::account::device_was_removed(dir, eid)).then(|| (eid.to_owned(), OldDevice {
            kind: d["kind"].as_str().map(|s| s.chars().take(16).collect()),
            os: d["os"].as_str().map(|s| s.chars().take(16).collect()),
            model: d["model"].as_str().map(|s| friends::sanitize_display_name(s, "")).filter(|s| !s.is_empty()),
            via: friend.name.clone(),
        }))
    }).collect()).unwrap_or_default();
    let folders: Vec<String> = reply["folders"].as_array().map(|a| a.iter().take(50)
        .filter_map(|f| f.as_str()).map(|f| friends::sanitize_display_name(f, "")).filter(|f| !f.is_empty()).collect()).unwrap_or_default();
    with_book(dir, |b| {
        for (eid, d) in devices {
            if b.old_devices.len() < 32 && b.old_devices.insert(eid, d).is_none() { out.devices += 1; }
        }
        for name in folders {
            let note = FolderNote { name, with: friend.name.clone() };
            if !b.folders.contains(&note) && b.folders.len() < 100 { b.folders.push(note); out.folders += 1; }
        }
        b.synced.insert(friend_eid.to_owned(), chat::now_ms());
        b.tries.remove(friend_eid);
    });
    Ok(out)
}

/// Ask `eid` (a friend) for their copy, over an open bi-stream.
pub(crate) async fn request_restore(dir: &Path, me: &str, send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<Value> {
    let account = crate::account::my_pub(dir).ok_or_else(|| anyhow::anyhow!("no account"))?;
    let sig = crate::link::sign_endpoint(dir, me).ok_or_else(|| anyhow::anyhow!("no account"))?;
    iroh_net::write_frame(send, &json!({"kind": "restore-sync", "v": 1, "account_pub": account, "account_sig": sig})).await?;
    send.finish()?;
    Ok(tokio::time::timeout(Duration::from_secs(90), iroh_net::read_frame_cap(recv, SYNC_FRAME_CAP)).await??)
}

/// iroh_net's dispatcher: a `restore-sync` stream.
pub(crate) async fn serve(net: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream) -> anyhow::Result<()> {
    let app = net.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?;
    let st = app.state::<Arc<AppState>>();
    let dir = st.config_dir.clone();
    let who_owned = who.to_owned();
    let req = req.clone();
    let reply = tokio::task::spawn_blocking(move || serve_restore(&dir, &who_owned, &req)).await?;
    iroh_net::write_frame(send, &reply).await?;
    send.finish()?;
    Ok(())
}

// ── rendezvous: where friends find a restored account ──────────────────────

const PKARR_RELAY: &str = "https://dns.iroh.link/pkarr";
const RV_NAME: &str = "_dropbeam";
/// A record older than this is ignored (the device it names is long gone).
const RV_MAX_AGE_MS: u64 = 60 * 24 * 3600 * 1000;
const PUBLISH_EVERY_MS: u64 = 6 * 3600 * 1000;
const LOOKUP_EVERY: Duration = Duration::from_secs(3 * 3600);

/// The signed record a restored device publishes: the devices to greet.
pub(crate) fn rendezvous_packet(key: &iroh::SecretKey, eids: &[String]) -> Result<iroh_dns::pkarr::SignedPacket, String> {
    let values: Vec<String> = std::iter::once("v=1".to_owned()).chain(eids.iter().take(4).map(|e| format!("e={e}"))).collect();
    iroh_dns::pkarr::SignedPacket::from_txt_strings(key, RV_NAME, values, 300).map_err(|e| e.to_string())
}

/// The device ids in a record, if it's well formed, signed by `account` and recent.
pub(crate) fn rendezvous_eids(account: &iroh::PublicKey, payload: &[u8], now_ms: u64) -> Vec<String> {
    let Ok(packet) = iroh_dns::pkarr::SignedPacket::from_relay_payload(account, payload) else { return vec![] };
    let at_ms = packet.timestamp().as_micros() / 1000;
    if now_ms.saturating_sub(at_ms) > RV_MAX_AGE_MS { return vec![]; }
    let txt = packet.txt_records(RV_NAME);
    if !txt.iter().any(|t| t == "v=1") { return vec![]; }
    txt.iter().filter_map(|t| t.strip_prefix("e="))
        .filter(|e| e.parse::<iroh::EndpointId>().is_ok_and(|p| p.to_string() == *e))
        .take(4).map(str::to_owned).collect()
}

fn http() -> reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default()).clone()
}

fn account_public(account_hex: &str) -> Option<iroh::PublicKey> {
    let bytes: [u8; 32] = hex::decode(account_hex).ok()?.try_into().ok()?;
    iroh::PublicKey::from_bytes(&bytes).ok()
}

async fn publish(dir: &Path, me: &str) -> Result<(), String> {
    let key = crate::link::account_key(dir).ok_or("no account")?;
    let mut eids = vec![me.to_owned()];
    eids.extend(crate::account::own_devices(dir).into_iter().filter_map(|f| f.endpoint_id));
    let packet = rendezvous_packet(&key, &eids)?;
    let url = format!("{PKARR_RELAY}/{}", key.public().to_z32());
    let r = http().put(url).body(packet.to_relay_payload()).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() { return Err(format!("relay said {}", r.status())); }
    log::info!("recovery: published where friends can find this restored device");
    Ok(())
}

async fn resolve(account_hex: &str) -> Vec<String> {
    let Some(public) = account_public(account_hex) else { return vec![] };
    let url = format!("{PKARR_RELAY}/{}", public.to_z32());
    let Ok(r) = http().get(url).send().await else { return vec![] };
    if !r.status().is_success() { return vec![]; }
    let Ok(bytes) = r.bytes().await else { return vec![] };
    rendezvous_eids(&public, &bytes, chat::now_ms())
}

/// Friends' accounts to look up, with the devices we already know for each.
fn lookup_targets(dir: &Path) -> Vec<(String, HashSet<String>)> {
    let mine = crate::account::my_pub(dir);
    let mut by_account: HashMap<String, HashSet<String>> = HashMap::new();
    for f in friends::load(dir) {
        let (Some(a), Some(e)) = (f.account_pub, f.endpoint_id) else { continue };
        if Some(&a) == mine.as_ref() || crate::block::account_blocked(dir, &a) { continue; }
        by_account.entry(a).or_default().insert(e);
    }
    let mut out: Vec<_> = by_account.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.truncate(64);
    out
}

/// Look friends' accounts up and greet any device of theirs we don't know yet
/// (a restored device): our hello carries the vouch they gave us.
async fn look_up_friends(app: &AppHandle, net: &Arc<IrohState>, dir: &Path) {
    let my_name = app.try_state::<Arc<AppState>>().map(|st| st.settings.lock().unwrap().display_name.clone()).unwrap_or_default();
    for (account, known) in lookup_targets(dir) {
        for eid in resolve(&account).await {
            if known.contains(&eid) || friends::is_revoked(dir, &account, &eid) || crate::block::is_blocked(dir, &eid)
                || net.get().is_some_and(|ep| ep.id().to_string() == eid) {
                continue;
            }
            log::info!("recovery: a friend's account names a device we don't know; saying hello");
            let extra = hello_fields(dir, &eid, Some(&account));
            iroh_net::say_hello_to_endpoint_with(net.clone(), eid, my_name.clone(), Value::Object(extra));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ── the restored device's loop ─────────────────────────────────────────────

fn wake() -> &'static Notify {
    static N: OnceLock<Notify> = OnceLock::new();
    N.get_or_init(Notify::new)
}

/// Something worth acting on now (a friend came back, a restore happened).
pub(crate) fn nudge() {
    wake().notify_one();
}

/// Friends' devices to ask for their copy now (not yet answered, not backing off).
fn due_friends(dir: &Path, now: u64) -> Vec<String> {
    let b = read_book(dir);
    let mine = crate::account::my_pub(dir);
    friends::load(dir).into_iter()
        .filter(|f| f.account_pub != mine)
        .filter_map(|f| f.endpoint_id)
        .filter(|e| !b.synced.contains_key(e))
        .filter(|e| b.tries.get(e).is_none_or(|(n, at)| now.saturating_sub(*at) >= backoff_ms(*n)))
        .take(32)
        .collect()
}

fn backoff_ms(tries: u32) -> u64 {
    (60_000u64 << tries.min(8)).min(6 * 3600 * 1000)
}

async fn ask_friend(app: &AppHandle, net: &Arc<IrohState>, dir: &Path, eid: &str) {
    let Some(ep) = net.get().cloned() else { return };
    let me = ep.id().to_string();
    let Ok(parsed) = eid.parse::<iroh::EndpointId>() else { return };
    let result = async {
        let conn = tokio::time::timeout(Duration::from_secs(20), ep.connect(iroh_net::dial_addr(parsed), iroh_net::ALPN)).await??;
        let (mut send, mut recv) = conn.open_bi().await?;
        request_restore(dir, &me, &mut send, &mut recv).await
    }.await;
    match result.map_err(|e| e.to_string()).and_then(|reply| {
        if reply["kind"] == "restore-sync-no" {
            // They don't (or don't yet) count us as their friend: don't ask again soon.
            return Err("declined".into());
        }
        apply_restore(dir, &me, eid, &reply)
    }) {
        Ok(applied) => {
            log::info!("recovery: a friend sent back {} messages, {} old devices, {} folders", applied.messages, applied.devices, applied.folders);
            let _ = app.emit("chat://changed", ());
            let _ = app.emit("recovery://changed", ());
            crate::account::note_change();
        }
        Err(e) => {
            log::debug!("recovery: history request not answered ({e})");
            with_book(dir, |b| {
                let t = b.tries.entry(eid.to_owned()).or_insert((0, 0));
                *t = (t.0.saturating_add(1), chat::now_ms());
            });
        }
    }
}

/// Background work: every device looks friends' accounts up now and then; a
/// restored device also publishes where it is and asks friends for history.
pub fn spawn(app: AppHandle, net: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let Some(dir) = app.try_state::<Arc<AppState>>().map(|st| st.config_dir.clone()) else { return };
        let mut next_lookup = tokio::time::Instant::now() + Duration::from_secs(160);
        loop {
            if restore_active(&dir) {
                if let Some(me) = net.get().map(|e| e.id().to_string()) {
                    let now = chat::now_ms();
                    if now.saturating_sub(read_book(&dir).published_at) >= PUBLISH_EVERY_MS {
                        match publish(&dir, &me).await {
                            Ok(()) => with_book(&dir, |b| b.published_at = now),
                            Err(e) => log::debug!("recovery: publish failed ({e})"),
                        }
                    }
                    for eid in due_friends(&dir, now) {
                        ask_friend(&app, &net, &dir, &eid).await;
                    }
                }
            }
            if tokio::time::Instant::now() >= next_lookup {
                look_up_friends(&app, &net, &dir).await;
                next_lookup = tokio::time::Instant::now() + LOOKUP_EVERY;
            }
            let pause = if restore_active(&dir) { Duration::from_secs(60) } else { Duration::from_secs(1800) };
            tokio::select! {
                _ = wake().notified() => tokio::time::sleep(Duration::from_secs(2)).await,
                _ = tokio::time::sleep(pause) => {}
            }
        }
    });
}

// ── restoring ──────────────────────────────────────────────────────────────

/// Install the account from `text` on this device (endpoint `me`).
pub(crate) fn restore_from_words(dir: &Path, me: &str, text: &str) -> Result<String, String> {
    let entropy = decode(text)?;
    let (key, seed) = key_from_entropy(&entropy);
    drop(entropy);
    let others = crate::account::own_devices(dir);
    crate::link::install_recovered(dir, &key, seed.as_deref(), &others)?;
    let account = hex::encode(key.public().as_bytes());
    // A fresh book for this account: this device is its only member, so any
    // device from before that turns up asks for approval (S4) — and one
    // a friend reports is listed as "from before" until kept or removed.
    crate::account::start_restored_book(dir, &account, me);
    crate::account::forget_left(dir, &account);
    let now = chat::now_ms();
    with_book(dir, |b| {
        *b = Book { saved_for: account.clone(), saved_at: now, restored_at: now, restored_account: account.clone(), ..Default::default() };
    });
    log::info!("recovery: restored an account from its recovery words");
    Ok(account)
}

// ── commands ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The user confirmed writing down the code of the account this device uses.
    saved: bool,
    /// This device belongs to an account (it has a code to show).
    has_account: bool,
    /// When the user last said "later" (ms, 0 = never).
    later_at: u64,
    /// After a restore: what came back so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    restore: Option<RestoreView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreView {
    restored_at: u64,
    /// Friends who sent back their history.
    friends_synced: usize,
    returned: Vec<String>,
    old_devices: Vec<OldDeviceView>,
    folders: Vec<FolderNote>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OldDeviceView {
    endpoint_id: String,
    #[serde(flatten)]
    device: OldDevice,
}

pub(crate) fn status(dir: &Path) -> Status {
    let account = crate::account::my_pub(dir);
    let b = read_book(dir);
    let restore = restore_active(dir).then(|| RestoreView {
        restored_at: b.restored_at,
        friends_synced: b.synced.len(),
        returned: b.returned.clone(),
        old_devices: {
            let mut v: Vec<OldDeviceView> = b.old_devices.iter()
                .filter(|(e, _)| !b.kept.contains(*e) && !crate::account::device_was_removed(dir, e) && !crate::account::is_own_device(dir, e))
                .map(|(e, d)| OldDeviceView { endpoint_id: e.clone(), device: d.clone() }).collect();
            v.sort_by(|a, b| a.endpoint_id.cmp(&b.endpoint_id));
            v
        },
        folders: b.folders.clone(),
    });
    Status { saved: account.as_deref().is_some_and(|a| a == b.saved_for), has_account: account.is_some(), later_at: b.later_at, restore }
}

#[tauri::command]
pub fn recovery_status(state: State<'_, Arc<AppState>>) -> Status {
    status(&state.config_dir)
}

/// The words to write down, plus the text a QR of them holds. Creates the
/// account if this device has none yet (a single device can be recovered too).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reveal {
    words: Vec<String>,
    qr: String,
}

#[tauri::command]
pub fn recovery_reveal(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<Reveal, String> {
    let had = crate::account::my_pub(&state.config_dir).is_some();
    let words = code_words(&state.config_dir)?;
    if !had {
        // A new account: friends learn it in our next hello, which is what lets
        // them find this person again after a restore.
        if let Some(net) = app.try_state::<Arc<IrohState>>() { iroh_net::broadcast_profile(app.clone(), net.inner().clone()); }
        let _ = app.emit("friends://changed", ());
    }
    let qr = format!("{QR_PREFIX}{}", words.join(" "));
    Ok(Reveal { words: words.to_vec(), qr })
}

/// The user proved they wrote the code down (picked the right words).
#[derive(Deserialize)]
pub struct Answer {
    index: usize,
    word: String,
}

pub(crate) fn confirm_saved(dir: &Path, answers: &[Answer]) -> Result<(), String> {
    let account = crate::account::my_pub(dir).ok_or("Show your recovery code first.")?;
    let words = code_words(dir)?;
    if answers.is_empty() || answers.iter().any(|a| words.get(a.index).is_none_or(|w| *w != a.word.trim().to_lowercase())) {
        return Err("That’s not the right word. Look at your paper again.".into());
    }
    with_book(dir, |b| { b.saved_for = account; b.saved_at = chat::now_ms(); });
    Ok(())
}

#[tauri::command]
pub fn recovery_confirm_saved(state: State<'_, Arc<AppState>>, answers: Vec<Answer>) -> Result<(), String> {
    confirm_saved(&state.config_dir, &answers)
}

#[tauri::command]
pub fn recovery_later(state: State<'_, Arc<AppState>>) {
    with_book(&state.config_dir, |b| b.later_at = chat::now_ms());
}

#[tauri::command]
pub fn recovery_check(text: String) -> Check {
    let mut text = text;
    let out = check(&text);
    text.zeroize();
    out
}

#[tauri::command]
pub fn recovery_restore(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>, text: String) -> Result<(), String> {
    let mut text = text;
    let me = iroh.get().map(|e| e.id().to_string()).ok_or_else(|| NOT_READY.to_owned());
    let result = me.and_then(|me| restore_from_words(&state.config_dir, &me, &text));
    text.zeroize();
    result?;
    // Friends we already have here (a device that was used on its own) hear
    // the account now; everyone else finds us through the rendezvous record.
    iroh_net::broadcast_profile(app.clone(), iroh.inner().clone());
    crate::account::account_sync_now();
    nudge();
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("recovery://changed", ());
    Ok(())
}

/// Remove devices from before the restore (lost or stolen): signed removals,
/// so friends stop treating them as the user (S4).
#[tauri::command]
pub fn recovery_remove_old_devices(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>, endpoint_ids: Vec<String>) -> Result<(), String> {
    let signer = iroh.get().map(|e| e.secret_key().clone()).ok_or(NOT_READY)?;
    let dir = &state.config_dir;
    let known: HashSet<String> = read_book(dir).old_devices.keys().cloned().collect();
    for eid in endpoint_ids.iter().filter(|e| known.contains(*e)) {
        crate::account::remove_device(dir, eid, Some(&signer))?;
    }
    iroh_net::broadcast_profile(app.clone(), iroh.inner().clone());
    crate::account::account_sync_now();
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("recovery://changed", ());
    Ok(())
}

/// Print the window (the recovery sheet is the only thing the print stylesheet
/// shows while the words are on screen; "Save as PDF" is in the same dialog).
#[tauri::command]
pub fn recovery_print(webview: tauri::Webview) -> Result<(), String> {
    webview.print().map_err(|_| "Printing isn’t available here. Write the words down instead.".to_owned())
}

/// The user still has this old device: stop suggesting to remove it.
#[tauri::command]
pub fn recovery_keep_old_device(app: AppHandle, state: State<'_, Arc<AppState>>, endpoint_id: String) {
    with_book(&state.config_dir, |b| { b.kept.insert(endpoint_id); });
    let _ = app.emit("recovery://changed", ());
}

#[cfg(test)]
mod tests;
