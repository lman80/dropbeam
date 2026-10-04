//! Multi-device accounts, peer to peer. Every device that holds the same account
//! key (minted/shared by link.rs) is one of the user's OWN devices. Own devices
//! keep each other's device list, friends and chat history in step over iroh —
//! there is no server; the key is the account.
//!
//! One "account-sync" exchange runs on a single bi-stream, four frames:
//!   A → B  A's roster + friends + tombstones, one hash per chat thread (signed)
//!   B → A  B's roster + friends + tombstones, (key, hash) lists for every thread
//!          whose hash differs, avatars B wants
//!   A → B  messages B lacks or holds older, keys A wants, avatars B asked for
//!   B → A  the messages A wanted, avatars A asked for
//! B answers only a device that signs its endpoint id with the SAME account key.
//! Chat threads are matched by the friend's endpoint id (friend ids are local).
//! Merges are last-writer-wins per message (`ChatMessage::rev`) + furthest
//! delivery status, so repeated exchanges converge and stop.
//!
//! Who you are vs. which device: the display name and profile photo belong to
//! the PERSON — one account-wide profile, last change wins on every own device,
//! so friends see the same name/photo whichever device they reach. Each device
//! keeps its own device name ("Ashton's MacBook Pro", "iPhone") for the device
//! list ("Your Mac"). A name nobody chose (the computer's default) never
//! spreads; a chosen one replaces it everywhere.
//!
//! Ordering is Lamport-style throughout: a relink, re-add, rename, edit or
//! removal is stamped one past the newest stamp it has seen, so devices whose
//! clocks disagree still agree on the outcome. Removals are tombstones merged
//! by max; a device that leaves while the others are offline says so in its
//! next hello (`left_accounts`), since it can no longer sign for the account.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::{chat, friends, iroh_net::{self, IrohState}, models::Friend, AppState};

pub(crate) const SYNC_V: u64 = 1;
/// Cap for every frame after the first (the dispatcher caps the first at 1 MiB).
const FRAME_CAP: usize = 48 << 20;
/// Serialized message bytes per frame; anything left over goes next round.
const MSG_BUDGET: usize = 24 << 20;
const AVATAR_CAP: usize = 2 << 20;
/// Re-exchange with a device at least this often even if nothing changed here,
/// so its changes still arrive if its own nudge to us was lost.
const REFRESH: Duration = Duration::from_secs(10 * 60);
/// Only absurd timestamps from another device are clamped. Ordering is
/// Lamport-style everywhere (a re-add, relink or edit is stamped one past the
/// newest stamp it saw), so a device whose clock runs fast or slow can't pin a
/// removal: a tighter clamp here would make the devices disagree for good (the
/// fast device keeps its own stamp while the others keep a clamped copy).
const SKEW_MS: u64 = 10 * 365 * 24 * 3600 * 1000;
const AVATAR_BUDGET: usize = 8 << 20;

fn clamp(t: u64) -> u64 {
    t.min(chat::now_ms().saturating_add(SKEW_MS))
}

// ── change notification ─────────────────────────────────────────────────────

fn changed() -> &'static Notify {
    static N: OnceLock<Notify> = OnceLock::new();
    N.get_or_init(Notify::new)
}

/// Something syncable changed (a chat message, a friend): wake the sync loop.
/// Cheap; the loop debounces and skips devices already in step.
pub fn note_change() {
    changed().notify_one();
}

// ── persistent account bookkeeping (account-state.json) ────────────────────

#[derive(Default, Serialize, Deserialize, Clone)]
struct Book {
    /// The account these entries belong to (reset when the key changes).
    #[serde(default)]
    account: String,
    /// When each own device joined the account (ms), merged by max.
    #[serde(default)]
    linked: HashMap<String, u64>,
    /// Devices removed from the account (ms), merged by max. A device is out
    /// while its removal is newer than its link time.
    #[serde(default)]
    removed_devices: HashMap<String, u64>,
    /// Friends removed on some own device (ms), by endpoint id, merged by max.
    #[serde(default)]
    removed_friends: HashMap<String, u64>,
    /// The person's name + picture as last agreed with the other own devices.
    #[serde(default)]
    profile: Profile,
    /// S4: each device's link time, signed by a DIFFERENT device's endpoint key.
    /// Once `proofs_v` is 1, a device counts as one of the account's devices
    /// only with such a proof, and link times arrive from other devices only
    /// as proofs signed by a device still in the account — holding the
    /// account key alone (a removed or stolen device, or a fresh endpoint made
    /// with that key) no longer gets anyone in.
    #[serde(default)]
    link_proofs: HashMap<String, Vec<Stamp>>,
    /// 0 = a book from before link proofs (legacy rules until upgraded).
    #[serde(default)]
    proofs_v: u8,
    /// This device's endpoint id: the root every membership chain must reach
    /// (a proof only counts while its signer is itself a member — so removing
    /// a device also drops every device only IT vouched for).
    #[serde(default)]
    me: String,
    /// Signed removals (S4): removal times only arrive as these, signed by the
    /// removed device itself (it left) or by a device still in the account.
    /// They are also what friends are told, so a removed device can no longer
    /// pose as the user to them.
    #[serde(default)]
    removal_proofs: HashMap<String, Stamp>,
    /// Devices that prove the account key but no device vouched for (linked by
    /// an older build, or by a device since removed): shown in Devices as
    /// "Needs approval" until the user approves or removes them.
    #[serde(default)]
    pending_devices: HashMap<String, PendingDevice>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct PendingDevice {
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    at: u64,
}

/// Vouchers kept per device (one per signer, newest first).
const MAX_VOUCHERS: usize = 8;

/// A device's link time, signed by another device of the account (S4).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Stamp {
    at: u64,
    /// The signing device's endpoint id.
    by: String,
    sig: String,
}

impl Stamp {
    pub(crate) fn new(at: u64, by: &str, sig: &str) -> Stamp {
        Stamp { at, by: by.to_owned(), sig: sig.to_owned() }
    }
}

fn stamp_message(account: &str, eid: &str, at: u64) -> String {
    format!("dropbeam-account-link/1|{account}|{eid}|{at}")
}
fn removal_message(account: &str, eid: &str, at: u64) -> String {
    format!("dropbeam-account-remove/1|{account}|{eid}|{at}")
}

fn sign_stamp(signer: &iroh::SecretKey, account: &str, eid: &str, at: u64) -> Stamp {
    Stamp { at, by: signer.public().to_string(), sig: hex::encode(signer.sign(stamp_message(account, eid, at).as_bytes()).to_bytes()) }
}
fn sign_removal(signer: &iroh::SecretKey, account: &str, eid: &str, at: u64) -> Stamp {
    Stamp { at, by: signer.public().to_string(), sig: hex::encode(signer.sign(removal_message(account, eid, at).as_bytes()).to_bytes()) }
}

fn sig_ok(by: &str, message: &str, sig: &str) -> bool {
    let Ok(key) = by.parse::<iroh::PublicKey>() else { return false };
    let Some(sig) = hex::decode(sig).ok().and_then(|v| <[u8; 64]>::try_from(v).ok()) else { return false };
    key.verify(message.as_bytes(), &iroh::Signature::from_bytes(&sig)).is_ok()
}

/// A well-formed proof for `eid`, signed by some OTHER device (membership of
/// the signer is the caller's check).
fn stamp_valid(account: &str, eid: &str, s: &Stamp) -> bool {
    s.by != eid && sig_ok(&s.by, &stamp_message(account, eid, s.at), &s.sig)
}
/// A well-formed removal of `eid` (signer membership is the caller's check;
/// a device may always remove itself).
pub(crate) fn removal_valid(account: &str, eid: &str, s: &Stamp) -> bool {
    sig_ok(&s.by, &removal_message(account, eid, s.at), &s.sig)
}

/// The account-wide profile: ONE name and picture for the person, the same on
/// every own device (so friends see the same you whichever device they reach).
/// Each device keeps its own device name ("Ashton's MacBook Pro") for the
/// device list. Last writer wins, per field.
#[derive(Default, Serialize, Deserialize, Clone)]
struct Profile {
    /// False until this device first recorded its settings here.
    #[serde(default)]
    init: bool,
    #[serde(default)]
    name: String,
    /// When the name was chosen (ms). 0 = a default nobody chose (the
    /// computer's name), 1 = chosen before this was tracked.
    #[serde(default)]
    name_at: u64,
    /// The local picture path this was last recorded with ("" = none).
    #[serde(default)]
    avatar: String,
    #[serde(default)]
    avatar_at: u64,
}

/// What a device says about the profile on the wire.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub(crate) struct ProfileRec {
    #[serde(default)]
    name: String,
    #[serde(default)]
    name_at: u64,
    #[serde(default)]
    avatar_at: u64,
    #[serde(default)]
    has_avatar: bool,
}

static BOOK_LOCK: Mutex<()> = Mutex::new(());

fn book_path(dir: &Path) -> std::path::PathBuf {
    dir.join("account-state.json")
}

fn read_book(dir: &Path, account: &str) -> Book {
    let b: Book = std::fs::read(book_path(dir)).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if b.account == account { b } else { Book { account: account.to_owned(), ..Default::default() } }
}

fn write_book(dir: &Path, b: &Book) {
    if let Ok(bytes) = serde_json::to_vec(b) {
        if let Err(e) = crate::settings::write_atomic(&book_path(dir), &bytes) {
            log::warn!("account: cannot save account-state.json: {e}");
        }
    }
}

fn with_book<T>(dir: &Path, account: &str, f: impl FnOnce(&mut Book) -> T) -> T {
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut b = read_book(dir, account);
    let out = f(&mut b);
    write_book(dir, &b);
    out
}

impl Book {
    fn is_removed(&self, eid: &str) -> bool {
        self.removed_devices.get(eid).is_some_and(|r| *r >= self.linked.get(eid).copied().unwrap_or(0))
    }
    /// One of the account's devices right now (S4): not removed and, once
    /// proofs are in use, vouched for by a chain of proofs that reaches this
    /// device through devices that are all still in the account.
    fn is_member(&self, eid: &str) -> bool {
        if self.is_removed(eid) { return false; }
        if self.proofs_v == 0 { return true; }
        if self.me.is_empty() { return self.link_proofs.get(eid).is_some_and(|v| !v.is_empty()); }
        self.members().contains(eid)
    }
    fn members(&self) -> HashSet<String> {
        let mut set = HashSet::new();
        if !self.me.is_empty() && !self.is_removed(&self.me) { set.insert(self.me.clone()); }
        loop {
            let before = set.len();
            for (eid, stamps) in &self.link_proofs {
                if !set.contains(eid) && !self.is_removed(eid) && stamps.iter().any(|p| set.contains(&p.by)) {
                    set.insert(eid.clone());
                }
            }
            if set.len() == before { return set; }
        }
    }
    /// Merge removals another device sent: only signed ones, by the removed
    /// device itself or a device still in the account. True if anything changed.
    fn merge_removals(&mut self, account: &str, me: &str, removals: &HashMap<String, Stamp>) -> bool {
        let mut changed = false;
        for (eid, s) in removals {
            if self.removal_proofs.get(eid).is_some_and(|cur| cur.at >= s.at) { continue; }
            let signer_ok = s.by == *eid || s.by == me || self.is_member(&s.by);
            if !signer_ok || !removal_valid(account, eid, s) { continue; }
            let at = clamp(s.at);
            let x = self.removed_devices.entry(eid.clone()).or_default();
            *x = (*x).max(at);
            self.removal_proofs.insert(eid.clone(), Stamp { at, ..s.clone() });
            self.pending_devices.remove(eid);
            changed = true;
        }
        changed
    }
    /// Merge link proofs from another device: only ones signed by a device that
    /// is (by this book) still in the account, or by `me`. Repeats until no
    /// more apply, since one proof can vouch for the signer of another.
    fn merge_proofs(&mut self, account: &str, me: &str, proofs: &HashMap<String, Vec<Stamp>>) -> bool {
        let mut changed = false;
        loop {
            let mut progress = false;
            for (eid, stamps) in proofs {
                for s in stamps.iter().take(MAX_VOUCHERS) {
                    // One voucher per signer per device; a newer one replaces it.
                    // (A proof for `me` is kept too: this device forwards it, so
                    // a device that hasn't heard of a relink yet can learn it.)
                    if self.link_proofs.get(eid).is_some_and(|v| v.iter().any(|c| c.by == s.by && c.at >= s.at)) {
                        continue;
                    }
                    let signer_ok = s.by == me || self.is_member(&s.by);
                    if !signer_ok || !stamp_valid(account, eid, s) {
                        continue;
                    }
                    let at = clamp(s.at);
                    let x = self.linked.entry(eid.clone()).or_default();
                    *x = (*x).max(at);
                    self.add_voucher(eid, Stamp { at, ..s.clone() });
                    self.pending_devices.remove(eid);
                    progress = true;
                    changed = true;
                }
            }
            if !progress { return changed; }
        }
    }
    fn add_voucher(&mut self, eid: &str, s: Stamp) {
        let v = self.link_proofs.entry(eid.to_owned()).or_default();
        v.retain(|c| c.by != s.by);
        v.push(s);
        // Bounded: keep the newest few vouchers.
        v.sort_by_key(|c| std::cmp::Reverse(c.at));
        v.truncate(MAX_VOUCHERS);
    }
    fn my_voucher(&self, eid: &str) -> Option<&Stamp> {
        self.link_proofs.get(eid).and_then(|v| v.iter().find(|c| c.by == self.me))
    }
}

/// Bring this device's book onto link proofs (S4), signing as `signer`: every
/// device THIS device knows locally to be in the account (its own link records
/// and own-device records — never anything merely heard from the wire) gets a
/// proof from it. Runs before every exchange; cheap when nothing is new.
fn ensure_proofs(dir: &Path, account: &str, signer: &iroh::SecretKey) {
    let me = signer.public().to_string();
    // Read the friend list before BOOK_LOCK (friends::load can take it).
    let own: Vec<String> = friends::load_raw(dir).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account)).filter_map(|f| f.endpoint_id).collect();
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut b = read_book(dir, account);
    let mut changed = false;
    if b.me != me {
        b.me = me.clone();
        changed = true;
    }
    if b.proofs_v == 0 {
        // Upgrading: the devices this one already trusted keep their place.
        for eid in own {
            if eid != me && !b.is_removed(&eid) && !b.linked.contains_key(&eid) {
                b.linked.insert(eid, 1);
            }
        }
        b.proofs_v = 1;
        changed = true;
    }
    // Vouch only for devices linked HERE (no voucher at all yet), and renew our
    // own voucher after a relink. Never auto-vouch for a device whose only
    // vouchers came from devices since removed — that's what transitive
    // revocation means; the user approves it again in Devices.
    let due: Vec<(String, u64)> = b.linked.iter()
        .filter(|(e, at)| **e != me && !b.is_removed(e) && match b.link_proofs.get(*e) {
            None => true,
            Some(v) if v.is_empty() => true,
            Some(_) => b.my_voucher(e).is_some_and(|p| p.at < **at),
        })
        .map(|(e, at)| (e.clone(), *at)).collect();
    for (eid, at) in due {
        let st = sign_stamp(signer, account, &eid, at);
        b.add_voucher(&eid, st);
        changed = true;
    }
    if changed { write_book(dir, &b); }
}

pub(crate) fn my_pub(dir: &Path) -> Option<String> {
    crate::link::account_pub(dir)
}

/// This device's own name for the device list ("Ashton's MacBook Pro"; on iOS,
/// where apps can't read the user's device name, just "iPhone"/"iPad"). The
/// person's name is the account-wide display name instead.
pub(crate) fn device_name(kind: &str) -> String {
    #[cfg(target_os = "ios")]
    {
        (if kind == "tablet" { "iPad" } else { "iPhone" }).to_owned()
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = kind;
        static NAME: OnceLock<String> = OnceLock::new();
        NAME.get_or_init(|| {
            let n = whoami::devicename();
            if n.trim().is_empty() { "My Computer".to_owned() } else { n.trim().to_owned() }
        }).clone()
    }
}

/// A display name nobody chose: the device's own default ("Ashton's MacBook
/// Pro", "iPhone"). Such a name never spreads to the user's other devices.
fn is_default_name(name: &str, kind: &str) -> bool {
    let n = name.trim();
    let lower = n.to_lowercase();
    // Case-insensitive: people type "Ashton's Iphone" as often as "iPhone".
    let device_like = ["iphone", "ipad", "macbook", "imac", "mac mini", "mac studio", "mac pro", "my computer",
        "android", "galaxy", "pixel", "'s mac", "’s mac", "'s pc", "’s pc", "laptop", "desktop"]
        .iter().any(|d| lower.contains(d)) || n.starts_with("DESKTOP-") || n.starts_with("LAPTOP-");
    n.is_empty() || n == device_name(kind) || device_like
}

/// Bring the book's profile up to date with this device's settings (recording
/// a change the user made here), then return what to tell the other devices.
/// Runs under BOOK_LOCK, which also orders it against adopting a remote change.
fn note_local_profile(b: &mut Book, s: &crate::models::Settings) {
    let p = &mut b.profile;
    let avatar = if s.avatar.trim().is_empty() || !Path::new(&s.avatar).exists() { String::new() } else { s.avatar.clone() };
    if !p.init {
        *p = Profile { init: true, name: s.display_name.trim().to_owned(),
            name_at: u64::from(!is_default_name(&s.display_name, &s.device_kind)),
            avatar_at: u64::from(!avatar.is_empty()), avatar };
        return;
    }
    if p.name != s.display_name.trim() {
        p.name = s.display_name.trim().to_owned();
        p.name_at = chat::now_ms().max(p.name_at.saturating_add(1));
    }
    if p.avatar != avatar {
        p.avatar = avatar;
        p.avatar_at = chat::now_ms().max(p.avatar_at.saturating_add(1));
    }
}

fn profile_rec(p: &Profile) -> ProfileRec {
    ProfileRec { name: p.name.clone(), name_at: p.name_at, avatar_at: p.avatar_at, has_avatar: !p.avatar.is_empty() }
}

/// This device's profile as the other own devices should see it.
fn local_profile(st: &AppState, account: &str) -> ProfileRec {
    with_book(&st.config_dir, account, |b| {
        note_local_profile(b, &st.settings.lock().unwrap());
        profile_rec(&b.profile)
    })
}

/// What adopting another device's profile changed here.
#[derive(Default)]
struct ProfileApplied {
    /// The display name or picture changed (tell the UI + friends).
    changed: bool,
    /// Their picture is newer: ask for it.
    want_avatar: bool,
}

/// The display name another own device replaced here (old → new), so a window
/// still holding the old settings can't undo the synced rename by saving
/// them (D17).
static REPLACED_NAME: Mutex<Option<(String, String)>> = Mutex::new(None);
fn note_replaced_name(old: &str, new: &str) {
    *REPLACED_NAME.lock().unwrap_or_else(|p| p.into_inner()) = Some((old.trim().to_owned(), new.trim().to_owned()));
}
/// True when a settings save carrying `incoming` as the name is just a stale
/// copy from before the synced rename to `current` (keep `current`).
pub(crate) fn stale_display_name(incoming: &str, current: &str) -> bool {
    let g = REPLACED_NAME.lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref().is_some_and(|(old, new)| old == incoming.trim() && new == current.trim() && old != new)
}

fn save_settings(st: &AppState, s: &crate::models::Settings) {
    if let Err(e) = crate::settings::save(&st.config_dir, s) {
        log::warn!("account: cannot save the synced profile: {e}");
    }
}

/// Adopt the parts of another own device's profile that are newer than ours.
fn adopt_profile(st: &AppState, account: &str, theirs: &ProfileRec) -> ProfileApplied {
    let mut out = ProfileApplied::default();
    with_book(&st.config_dir, account, |b| {
        let mut s = st.settings.lock().unwrap();
        note_local_profile(b, &s);
        let p = &mut b.profile;
        let name = theirs.name.trim().chars().take(64).collect::<String>();
        // A default name (the other computer's own name) is never adopted: only
        // a name somebody chose travels between devices.
        // Two names both set before the devices met (stamp 1) are both the
        // user's choice: keep ours rather than let an alphabetical tie-break
        // rename this device. Only a real, timestamped rename (or replacing a
        // default device name, stamp 0) travels.
        let wins = theirs.name_at > p.name_at && (theirs.name_at > 1 || p.name_at == 0);
        if !name.is_empty() && theirs.name_at > 0 && wins {
            if name != p.name {
                note_replaced_name(&s.display_name, &name);
                s.display_name = name.clone();
                save_settings(st, &s);
                out.changed = true;
            }
            p.name = name;
            p.name_at = theirs.name_at;
        }
        if theirs.avatar_at > p.avatar_at {
            if theirs.has_avatar {
                out.want_avatar = true;
            } else {
                // The picture was removed on the other device.
                if !s.avatar.is_empty() {
                    let _ = std::fs::remove_file(&s.avatar);
                    s.avatar.clear();
                    save_settings(st, &s);
                    out.changed = true;
                }
                p.avatar.clear();
                p.avatar_at = theirs.avatar_at;
            }
        }
    });
    out
}

/// The picture to hand another own device: the file itself when it's small, a
/// re-encoded copy otherwise.
fn profile_avatar_payload(st: &AppState, account: &str) -> Option<Value> {
    let (path, at) = with_book(&st.config_dir, account, |b| {
        note_local_profile(b, &st.settings.lock().unwrap());
        (b.profile.avatar.clone(), b.profile.avatar_at)
    });
    if path.is_empty() {
        return None;
    }
    let bytes = crate::iroh_net::avatar_for_sync(&path)?;
    Some(json!({"at": at, "b64": STANDARD.encode(bytes)}))
}

/// Store a newer picture another own device sent; true if it was adopted.
fn adopt_profile_avatar(st: &AppState, account: &str, v: &Value) -> bool {
    let Some(at) = v["at"].as_u64().map(clamp) else { return false };
    let Some(bytes) = v["b64"].as_str().filter(|s| s.len() <= AVATAR_CAP * 4 / 3 + 4)
        .and_then(|s| STANDARD.decode(s).ok()).filter(|b| b.len() <= AVATAR_CAP && friends::is_safe_avatar(b)) else { return false };
    let dir = st.config_dir.clone();
    with_book(&dir, account, |b| {
        let mut s = st.settings.lock().unwrap();
        note_local_profile(b, &s);
        if at <= b.profile.avatar_at {
            return false;
        }
        let ext = image::guess_format(&bytes).ok().and_then(|f| f.extensions_str().first().copied()).unwrap_or("jpg");
        let path = dir.join(format!("avatar-{}.{ext}", chat::now_ms()));
        if std::fs::write(&path, &bytes).is_err() {
            return false;
        }
        // Same housekeeping as picking a picture: only the current one stays.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("avatar-") && e.path() != path {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        s.avatar = path.to_string_lossy().into_owned();
        save_settings(st, &s);
        b.profile.avatar = s.avatar.clone();
        b.profile.avatar_at = at;
        true
    })
}

/// This account's roster and friend removals: (device link times, device
/// removal times, friend removal times), all ms by endpoint id. Empty maps when
/// this device has no account.
pub(crate) fn roster_times(dir: &Path) -> (HashMap<String, u64>, HashMap<String, u64>, HashMap<String, u64>) {
    let Some(account) = my_pub(dir) else { return Default::default() };
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let b = read_book(dir, &account);
    (b.linked, b.removed_devices, b.removed_friends)
}

/// True when `account_pub` is this device's account and `eid` was removed from it.
pub(crate) fn is_removed_device(dir: &Path, account_pub: &str, eid: &str) -> bool {
    if my_pub(dir).as_deref() != Some(account_pub) {
        return false;
    }
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // Also "not vouched for" (S4): proving the account key isn't enough.
    !read_book(dir, account_pub).is_member(eid)
}

/// A device joined the account now (either end of a link): record it and clear
/// any older removal so the relink sticks everywhere.
pub(crate) fn mark_linked(dir: &Path, eid: &str) {
    let Some(account) = my_pub(dir) else { return };
    let now = chat::now_ms();
    with_book(dir, &account, |b| {
        let at = b.linked.entry(eid.to_owned()).or_default();
        *at = (*at).max(now).max(b.removed_devices.get(eid).map_or(0, |r| r.saturating_add(1)));
    });
    note_change();
}

/// A friend was removed on this device: remember it so the user's other devices
/// drop them too (unless they are re-added later).
pub(crate) fn record_friend_removed(dir: &Path, friend: &Friend) {
    let (Some(account), Some(eid)) = (my_pub(dir), friend.endpoint_id.as_deref()) else { return };
    with_book(dir, &account, |b| {
        let at = b.removed_friends.entry(eid.to_owned()).or_default();
        *at = (*at).max(chat::now_ms()).max(friend.created_at);
    });
    note_change();
}

/// The user's own devices (other than this one) that are still in the account.
pub(crate) fn own_devices(dir: &Path) -> Vec<Friend> {
    let Some(account) = my_pub(dir) else { return vec![] };
    let book = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(dir, &account) };
    friends::load(dir).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account.as_str()))
        .filter(|f| f.endpoint_id.as_deref().is_some_and(|e| book.is_member(e)))
        .collect()
}

/// A friend the user removed on one of their devices (don't let a stray hello
/// quietly re-add them; adding them again on purpose still works).
pub(crate) fn friend_removed(dir: &Path, eid: &str) -> bool {
    friend_removed_at(dir, eid).is_some()
}

/// When the user removed the friend at `eid` (on any own device), if they did.
pub(crate) fn friend_removed_at(dir: &Path, eid: &str) -> Option<u64> {
    let account = my_pub(dir)?;
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_book(dir, &account).removed_friends.get(eid).copied()
}

/// A device that was removed from (or left) this device's account and hasn't
/// been linked again.
pub(crate) fn device_was_removed(dir: &Path, eid: &str) -> bool {
    let Some(account) = my_pub(dir) else { return false };
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_book(dir, &account).is_removed(eid)
}

// ── leaving while the other devices are offline ────────────────────────────

fn left_path(dir: &Path) -> std::path::PathBuf {
    dir.join("account-left.json")
}

/// Accounts this device left: account → (when it left, the link time it left).
/// A device can't sign for an account it left, but it can always speak for
/// ITSELF: it tells its former devices in its hello (they see its verified
/// endpoint id), so they drop it even if they were offline when it left.
fn read_left(dir: &Path) -> HashMap<String, Value> {
    std::fs::read(left_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn record_left(dir: &Path, account: &str, at: u64, since: u64, proof: Option<Stamp>) {
    let mut all = read_left(dir);
    // The signed removal (when this device left on its own) rides along, so the
    // devices that hear the notice can pass it on to the rest (S4).
    all.insert(account.to_owned(), json!({"at": at, "since": since, "proof": proof}));
    all.retain(|_, v| chat::now_ms().saturating_sub(v["at"].as_u64().unwrap_or(0)) < 365 * 24 * 3600 * 1000);
    if let Ok(bytes) = serde_json::to_vec(&all) {
        let _ = crate::settings::write_atomic(&left_path(dir), &bytes);
    }
}

/// This device (re)joined `account`: it no longer needs to announce leaving it.
pub(crate) fn forget_left(dir: &Path, account: &str) {
    let mut all = read_left(dir);
    if all.remove(account).is_some() {
        if let Ok(bytes) = serde_json::to_vec(&all) {
            let _ = crate::settings::write_atomic(&left_path(dir), &bytes);
        }
    }
}

/// The hello field announcing the accounts this device left.
pub(crate) fn left_notice(dir: &Path) -> Value {
    let all = read_left(dir);
    if all.is_empty() { Value::Null } else { json!(all) }
}

/// A hello from `who` says it left this device's account: record that (unless
/// it was linked again since) and drop it from the device list. True if it did.
pub(crate) fn apply_left_notice(dir: &Path, who: &str, notice: &Value) -> bool {
    let Some(account) = my_pub(dir) else { return false };
    let Some(n) = notice.get(&account) else { return false };
    let Some(at) = n["at"].as_u64() else { return false };
    let since = n["since"].as_u64();
    // Only a device of this account can leave it (anyone else could otherwise
    // hide themselves from the user's other devices).
    let own = is_own_device(dir, who);
    let removed = with_book(dir, &account, |b| {
        let Some(linked) = b.linked.get(who).copied().or(own.then_some(0)) else { return false };
        // It left the link we know about (the same link time, when it said),
        // not an older one it was since linked again after.
        let current = since.map_or(at > linked, |s| s >= linked);
        if !current || b.is_removed(who) {
            return false;
        }
        let x = b.removed_devices.entry(who.to_owned()).or_default();
        *x = (*x).max(clamp(at)).max(linked.saturating_add(1));
        // Signed by the device itself: keep it, so it reaches the other devices.
        if let Some(p) = serde_json::from_value::<Stamp>(n["proof"].clone()).ok().filter(|p| p.by == who && removal_valid(&account, who, p)) {
            b.removal_proofs.insert(who.to_owned(), p);
        }
        true
    });
    if removed {
        if let Some(f) = friends::load(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(who)
            && f.account_pub.as_deref() == Some(account.as_str())) {
            let _ = friends::remove(dir, &f.id);
        }
        note_change();
    }
    removed
}

/// Merge another device's link/removal times into ours (clamped, by max).
fn merge_roster_times(dir: &Path, account: &str, roster: &Roster, removed_friends: &HashMap<String, u64>) -> Book {
    merge_roster_times_as(dir, account, "", roster, removed_friends)
}

/// Merge what a VERIFIED, still-linked device told us (callers check that
/// first). Link times only arrive as proofs signed by a device in the account
/// (S4); an older book (no proofs yet) still takes them unsigned.
fn merge_roster_times_as(dir: &Path, account: &str, me: &str, roster: &Roster, removed_friends: &HashMap<String, u64>) -> Book {
    with_book(dir, account, |b| {
        if b.proofs_v == 0 {
            for (e, t) in &roster.linked { let x = b.linked.entry(e.clone()).or_default(); *x = (*x).max(clamp(*t)); }
            for (e, t) in &roster.removed { let x = b.removed_devices.entry(e.clone()).or_default(); *x = (*x).max(clamp(*t)); }
        } else {
            b.merge_proofs(account, me, &roster.link_proofs);
            // Removals only as signed statements (unsigned ones — any device
            // could make them up — are ignored).
            b.merge_removals(account, me, &roster.removal_proofs);
        }
        for (e, t) in removed_friends { let x = b.removed_friends.entry(e.clone()).or_default(); *x = (*x).max(clamp(*t)); }
        b.clone()
    })
}

pub(crate) fn is_own_device(dir: &Path, eid: &str) -> bool {
    own_devices(dir).iter().any(|f| f.endpoint_id.as_deref() == Some(eid))
}

// ── in-memory sync status ───────────────────────────────────────────────────

#[derive(Default)]
struct Status {
    /// Local fingerprint at the end of the last good exchange, per device.
    in_step: HashMap<String, (String, Instant)>,
    /// When each device last completed an exchange (ms since epoch).
    last_ok: HashMap<String, u64>,
    /// Retry gate after failures: (not before, consecutive failures).
    backoff: HashMap<String, (Instant, u32)>,
    running: HashSet<String>,
}

fn status() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// One of our devices just connected to us: sync with it soon, whatever the backoff.
pub(crate) fn device_seen(dir: &Path, eid: &str) {
    if !is_own_device(dir, eid) {
        return;
    }
    let mut s = status().lock().unwrap();
    s.backoff.remove(eid);
    let stale = s.in_step.get(eid).is_none_or(|(_, at)| at.elapsed() > Duration::from_secs(60));
    if stale {
        s.in_step.remove(eid);
        drop(s);
        note_change();
    }
}

pub(crate) fn last_sync(eid: &str) -> Option<u64> {
    status().lock().unwrap().last_ok.get(eid).copied()
}

// ── wire records ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
struct DeviceRec {
    eid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    os: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Roster {
    me: DeviceRec,
    #[serde(default)]
    devices: Vec<DeviceRec>,
    #[serde(default)]
    linked: HashMap<String, u64>,
    #[serde(default)]
    removed: HashMap<String, u64>,
    /// Signed link times (S4); older builds ignore them.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    link_proofs: HashMap<String, Vec<Stamp>>,
    /// Signed removals (S4); older builds ignore them.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    removal_proofs: HashMap<String, Stamp>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct FriendRec {
    eid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    name_custom: bool,
    /// When the user renamed them (0 = never / an older build).
    #[serde(default, skip_serializing_if = "is_zero")]
    name_at: u64,
    #[serde(default)]
    created_at: u64,
    #[serde(default = "yes")]
    auto_accept: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    account: Option<String>,
    /// mtime (ms) of the cached profile picture, 0 = none.
    #[serde(default)]
    avatar: u64,
}

fn yes() -> bool {
    true
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    roster: Roster,
    #[serde(default)]
    friends: Vec<FriendRec>,
    #[serde(default)]
    removed_friends: HashMap<String, u64>,
    /// The person's shared name + picture (absent from older builds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<ProfileRec>,
    /// Blocked people by endpoint id, newest decision wins (absent from older
    /// builds, which ignore it; they still drop the person via `removed_friends`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    blocked: HashMap<String, crate::block::BlockRec>,
}

/// Everything this device shares about the account, plus thread summaries.
struct Local {
    account: String,
    me: String,
    meta: Meta,
    /// friend endpoint id → local friend id, for friends whose threads sync.
    threads: HashMap<String, String>,
    /// friend endpoint id → current avatar mtime / path.
    avatars: HashMap<String, (u64, String)>,
}

fn mtime_ms(path: &str) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

fn this_device(st: &AppState, me: &str) -> DeviceRec {
    let s = st.settings.lock().unwrap();
    DeviceRec { eid: me.to_owned(), name: device_name(&s.device_kind), kind: Some(s.device_kind.clone()),
        os: Some(std::env::consts::OS.to_owned()) }
}

fn gather(dir: &Path, account: &str, me: &str, device: &DeviceRec) -> Local {
    let book = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(dir, account) };
    let all = friends::load(dir);
    let mut devices = vec![];
    let mut recs = vec![];
    let mut threads = HashMap::new();
    let mut avatars = HashMap::new();
    for f in all {
        let Some(eid) = f.endpoint_id.clone() else { continue };
        if eid == me {
            continue;
        }
        if f.account_pub.as_deref() == Some(account) {
            if book.is_member(&eid) {
                devices.push(DeviceRec { eid, name: f.name, kind: f.device_kind, os: f.device_os });
            }
            continue;
        }
        let avatar = f.avatar.as_deref().map(|p| (mtime_ms(p), p.to_owned())).filter(|(m, _)| *m > 0);
        if let Some(a) = &avatar {
            avatars.insert(eid.clone(), a.clone());
        }
        threads.insert(eid.clone(), f.id.clone());
        recs.push(FriendRec { eid, name: f.name, name_custom: f.name_custom, name_at: f.name_at, created_at: f.created_at,
            auto_accept: f.auto_accept, kind: f.device_kind, os: f.device_os, account: f.account_pub,
            avatar: avatar.map_or(0, |a| a.0) });
    }
    recs.sort_by(|a, b| a.eid.cmp(&b.eid));
    devices.sort_by(|a, b| a.eid.cmp(&b.eid));
    Local {
        account: account.to_owned(),
        me: me.to_owned(),
        meta: Meta {
            roster: Roster { me: device.clone(), devices, linked: book.linked, removed: book.removed_devices, link_proofs: book.link_proofs, removal_proofs: book.removal_proofs },
            friends: recs,
            removed_friends: book.removed_friends,
            profile: None,
            blocked: crate::block::snapshot(dir),
        },
        threads,
        avatars,
    }
}

fn thread_hash(digest: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    for (k, v) in digest {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(v.as_bytes());
        h.update(b";");
    }
    hex::encode(&h.finalize()[..8])
}

fn summaries(dir: &Path, local: &Local) -> HashMap<String, String> {
    local.threads.iter().filter_map(|(eid, fid)| {
        let d = chat::sync_digest(dir, fid);
        (!d.is_empty()).then(|| (eid.clone(), thread_hash(&d)))
    }).collect()
}

/// A digest of everything this device would send, to skip devices already in step.
fn fingerprint(dir: &Path, local: &Local) -> String {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&local.meta.friends).unwrap_or_default());
    h.update(serde_json::to_vec(&local.meta.roster.devices).unwrap_or_default());
    let mut sums: Vec<_> = summaries(dir, local).into_iter().collect();
    sums.sort();
    h.update(serde_json::to_vec(&sums).unwrap_or_default());
    let mut tombs: Vec<_> = local.meta.removed_friends.iter().chain(local.meta.roster.removed.iter()).collect();
    tombs.sort();
    h.update(serde_json::to_vec(&tombs).unwrap_or_default());
    h.update(serde_json::to_vec(&local.meta.profile).unwrap_or_default());
    let mut blocks: Vec<_> = local.meta.blocked.iter().map(|(e, r)| (e, r.blocked, r.at)).collect();
    blocks.sort();
    h.update(serde_json::to_vec(&blocks).unwrap_or_default());
    hex::encode(&h.finalize()[..12])
}

// ── applying what another own device told us ───────────────────────────────

struct Applied {
    /// Friends newly learned (this device should introduce itself to them).
    new_friends: Vec<String>,
    /// Own devices this device just learned about (e.g. linked elsewhere).
    new_devices: Vec<DeviceRec>,
    /// Friends whose picture the other device has newer.
    want_avatars: Vec<String>,
    left_account: bool,
    changed: bool,
}

impl Applied {
    fn left() -> Applied {
        Applied { new_friends: vec![], new_devices: vec![], want_avatars: vec![], left_account: true, changed: true }
    }
}

fn apply_meta(dir: &Path, local: &Local, from: &str, meta: &Meta) -> Applied {
    let account = &local.account;
    let mut out = Applied { new_friends: vec![], new_devices: vec![], want_avatars: vec![], left_account: false, changed: false };
    // 1. Tombstones + link times, merged by max.
    let book = merge_roster_times_as(dir, account, &local.me, &meta.roster, &meta.removed_friends);
    if book.is_removed(&local.me) {
        leave_account(dir, account, &local.me);
        return Applied::left();
    }
    // Blocks decided on another own device: adopt, and drop the person here.
    if crate::block::merge(dir, &meta.blocked, clamp) | crate::block::purge_blocked_friends(dir) {
        out.changed = true;
    }
    // 2. Devices: the sender itself and every device it lists.
    let mut devices = meta.roster.devices.clone();
    let mut sender = meta.roster.me.clone();
    sender.eid = from.to_owned();
    devices.push(sender);
    let known: HashSet<String> = friends::load(dir).into_iter()
        .filter(|f| f.account_pub.as_deref() == Some(account.as_str())).filter_map(|f| f.endpoint_id).collect();
    let mut device_ids = HashSet::new();
    for d in &devices {
        // Only devices vouched for in the account (S4); the sender itself was
        // verified before anything was merged.
        if d.eid == local.me || d.eid.parse::<iroh::EndpointId>().is_err() || !(book.is_member(&d.eid) || d.eid == from) {
            if d.eid != local.me && !book.is_removed(&d.eid) && d.eid.parse::<iroh::EndpointId>().is_ok() {
                note_pending_device(dir, &d.eid, &d.name, d.kind.as_deref(), d.os.as_deref());
            }
            continue;
        }
        device_ids.insert(d.eid.clone());
        let linked = book.linked.get(&d.eid).copied().unwrap_or(0);
        // A device's name comes only from the device itself (its own roster
        // entry); third-hand names would ping-pong between devices forever.
        let own_entry = d.eid == from;
        out.changed |= friends::upsert_own_device(dir, &d.eid, &d.name, d.kind.as_deref(), d.os.as_deref(), account, linked, own_entry);
        if !known.contains(&d.eid) {
            out.new_devices.push(d.clone());
        }
    }
    // Devices removed from the account: forget them here too.
    for f in friends::load(dir) {
        let Some(eid) = f.endpoint_id.as_deref() else { continue };
        if f.account_pub.as_deref() == Some(account.as_str()) && book.is_removed(eid) {
            let _ = friends::remove(dir, &f.id);
            out.changed = true;
        }
    }
    // 3. Friends. A friend counts as added when the LATEST add on any device
    // says so: a re-add on one device outlives an older removal on another.
    let their_adds: HashMap<&str, u64> = meta.friends.iter().map(|r| (r.eid.as_str(), r.created_at)).collect();
    for r in &meta.friends {
        if r.eid == local.me || device_ids.contains(&r.eid) || r.account.as_deref() == Some(account.as_str())
            || r.eid.parse::<iroh::EndpointId>().is_err() {
            continue;
        }
        if book.removed_friends.get(&r.eid).is_some_and(|t| *t >= r.created_at) {
            continue;
        }
        // Someone blocked (on any own device) never comes back through sync.
        if crate::block::is_blocked(dir, &r.eid) || r.account.as_deref().is_some_and(|a| crate::block::account_blocked(dir, a)) {
            continue;
        }
        // A device that left or was removed from the account isn't a friend.
        if book.is_removed(&r.eid) && book.removed_devices.get(&r.eid).is_some_and(|t| *t >= r.created_at) {
            continue;
        }
        let (friend, added) = friends::import_synced_friend(dir, &friends::SyncedFriend {
            endpoint_id: &r.eid, name: &r.name, name_custom: r.name_custom, name_at: r.name_at, created_at: r.created_at,
            auto_accept: r.auto_accept, device_kind: r.kind.as_deref(), device_os: r.os.as_deref(),
            account_pub: r.account.as_deref(),
        });
        out.changed |= added;
        if added {
            out.new_friends.push(r.eid.clone());
        }
        let mine = friend.avatar.as_deref().map_or(0, mtime_ms);
        if r.avatar > mine {
            out.want_avatars.push(r.eid.clone());
        }
    }
    for f in friends::load(dir) {
        let Some(eid) = f.endpoint_id.as_deref() else { continue };
        if f.account_pub.as_deref() == Some(account.as_str()) {
            continue;
        }
        let added = f.created_at.max(their_adds.get(eid).copied().unwrap_or(0));
        if book.removed_friends.get(eid).is_some_and(|t| *t >= added) {
            let _ = friends::remove(dir, &f.id);
            out.changed = true;
        }
    }
    // What was learned may regroup a friend's devices (a newly known device of
    // theirs, or its account): keep each person's conversation in one thread.
    if friends::fold_person_threads(dir) > 0 {
        out.changed = true;
    }
    out
}

/// This device is out of `account` (removed elsewhere, or leaving): forget the
/// key, drop the former own devices' records, and remember that it
/// left so it can tell devices that were offline.
fn leave_account(dir: &Path, account: &str, me: &str) {
    log::warn!("account: this device left its account; unlinking");
    let (since, proof) = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); let b = read_book(dir, account);
        (b.linked.get(me).copied().unwrap_or(0), b.removal_proofs.get(me).filter(|p| p.by == me).cloned()) };
    record_left(dir, account, proof.as_ref().map_or_else(chat::now_ms, |p| p.at), since, proof);
    crate::link::forget_key(dir);
    friends::clear_account(dir, account);
    let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = std::fs::remove_file(book_path(dir));
}

fn avatars_for(local: &Local, wanted: &[String]) -> HashMap<String, Value> {
    let mut budget = AVATAR_BUDGET;
    wanted.iter().filter_map(|eid| {
        let (mtime, path) = local.avatars.get(eid)?;
        let bytes = std::fs::read(path).ok().filter(|b| !b.is_empty() && b.len() <= AVATAR_CAP && b.len() <= budget)?;
        budget -= bytes.len(); // the rest go next round
        Some((eid.clone(), json!({"mtime": mtime, "b64": STANDARD.encode(bytes)})))
    }).collect()
}

fn apply_avatars(dir: &Path, avatars: &Value) -> bool {
    let Some(map) = avatars.as_object() else { return false };
    let mut changed = false;
    for (eid, v) in map {
        if eid.parse::<iroh::EndpointId>().is_err() {
            continue;
        }
        let Some(bytes) = v["b64"].as_str().filter(|s| s.len() <= AVATAR_CAP * 4 / 3 + 4)
            .and_then(|s| STANDARD.decode(s).ok()).filter(|b| b.len() <= AVATAR_CAP && friends::is_safe_avatar(b)) else { continue };
        let mtime = v["mtime"].as_u64().unwrap_or(0);
        let path = dir.join(format!("friend-avatar-{eid}.jpg"));
        let current = mtime_ms(&path.to_string_lossy());
        if mtime <= current || std::fs::write(&path, &bytes).is_err() {
            continue;
        }
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(mtime));
        }
        changed |= friends::set_avatar_by_endpoint(dir, eid, path.to_string_lossy().into_owned());
    }
    changed
}

/// (key, hash) lists of our threads whose summary differs from theirs.
fn differing_lists(dir: &Path, local: &Local, theirs: &HashMap<String, String>) -> HashMap<String, Vec<(String, String)>> {
    let mine = summaries(dir, local);
    let mut out = HashMap::new();
    for eid in mine.keys().chain(theirs.keys()).collect::<HashSet<_>>() {
        if mine.get(eid) == theirs.get(eid) {
            continue;
        }
        let Some(fid) = local.threads.get(eid) else { continue };
        out.insert(eid.clone(), chat::sync_digest(dir, fid));
    }
    out
}

/// Messages to send + keys to request, given the other side's lists.
fn plan(dir: &Path, local: &Local, lists: &HashMap<String, Vec<(String, String)>>)
    -> (HashMap<String, Vec<chat::ChatMessage>>, HashMap<String, Vec<String>>) {
    let mut send = HashMap::new();
    let mut want = HashMap::new();
    let mut budget = MSG_BUDGET;
    for (eid, theirs) in lists {
        let Some(fid) = local.threads.get(eid) else { continue };
        let theirs: HashMap<&str, &str> = theirs.iter().map(|(k, h)| (k.as_str(), h.as_str())).collect();
        let mine = chat::sync_digest(dir, fid);
        let mine_map: HashMap<&str, &str> = mine.iter().map(|(k, h)| (k.as_str(), h.as_str())).collect();
        let out_keys: HashSet<String> = mine.iter()
            .filter(|(k, h)| theirs.get(k.as_str()) != Some(&h.as_str()))
            .map(|(k, _)| k.clone()).collect();
        let in_keys: Vec<String> = theirs.iter()
            .filter(|(k, h)| mine_map.get(*k) != Some(*h))
            .map(|(k, _)| (*k).to_owned()).collect();
        if !in_keys.is_empty() {
            want.insert(eid.clone(), in_keys);
        }
        if out_keys.is_empty() || budget == 0 {
            continue;
        }
        // Newest first so a capped round still brings the recent conversation.
        let mut msgs = chat::sync_messages(dir, fid, &out_keys);
        msgs.reverse();
        let mut kept = vec![];
        for m in msgs {
            let size = serde_json::to_vec(&m).map_or(0, |v| v.len());
            if size > budget { budget = 0; break; }
            budget -= size;
            kept.push(m);
        }
        send.insert(eid.clone(), kept);
    }
    (send, want)
}

fn wanted_messages(dir: &Path, local: &Local, want: &Value) -> HashMap<String, Vec<chat::ChatMessage>> {
    let mut out = HashMap::new();
    let mut budget = MSG_BUDGET;
    for (eid, keys) in want.as_object().into_iter().flatten() {
        let Some(fid) = local.threads.get(eid) else { continue };
        let keys: HashSet<String> = keys.as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect();
        let mut msgs = chat::sync_messages(dir, fid, &keys);
        msgs.reverse();
        let mut kept = vec![];
        for m in msgs {
            let size = serde_json::to_vec(&m).map_or(0, |v| v.len());
            if size > budget { budget = 0; break; }
            budget -= size;
            kept.push(m);
        }
        out.insert(eid.clone(), kept);
    }
    out
}

fn apply_messages(dir: &Path, messages: &Value) -> usize {
    let mut changed = 0;
    for (eid, msgs) in messages.as_object().into_iter().flatten() {
        let Some(friend) = friends::load(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(eid.as_str())) else { continue };
        // A friend's extra device talks in the person's thread here too.
        let owner = friends::thread_owner(dir, &friend.id).map_or(friend.id, |o| o.id);
        let Ok(msgs) = serde_json::from_value::<Vec<chat::ChatMessage>>(msgs.clone()) else { continue };
        // iPhone: a push for one of these later must not ring again.
        let ids: Vec<&str> = msgs.iter().filter(|m| !m.from_me).map(|m| m.id.as_str()).collect();
        crate::mailbox::push::note_have(dir, &ids);
        changed += chat::merge_synced(dir, &owner, msgs);
    }
    changed
}

fn announce(app: &AppHandle, net: &Arc<IrohState>, out: &Outcome) {
    let applied = &out.applied;
    if applied.changed || !applied.new_friends.is_empty() {
        let _ = app.emit("friends://changed", ());
        // Cheap: the Blocked list re-reads a small file (blocks may have merged).
        let _ = app.emit("blocked://changed", ());
    }
    if out.chats > 0 {
        let _ = app.emit("chat://changed", ());
    }
    if applied.left_account {
        let _ = app.emit("account://left", ());
    }
    // A device linked through another one (or whose link reply got lost): this
    // is the moment this device knows it's in, so any "Add a device" screen
    // waiting on it can finish.
    for d in &applied.new_devices {
        let _ = app.emit("link://linked", json!({"endpoint_id": d.eid, "name": d.name,
            "device_kind": d.kind.clone().unwrap_or_default(), "device_os": d.os}));
    }
    let Some(st) = app.try_state::<Arc<AppState>>() else { return };
    // The shared name/picture changed here: refresh the UI and tell friends.
    if out.profile_changed {
        let settings = st.settings.lock().unwrap().clone();
        let _ = app.emit("settings://changed", &settings);
        iroh_net::broadcast_profile(app.clone(), net.clone());
    }
    // Friends this device just learned about: introduce ourselves so they can
    // reach (and recognize) this device too.
    if !applied.new_friends.is_empty() {
        let name = st.settings.lock().unwrap().display_name.clone();
        for eid in &applied.new_friends {
            iroh_net::say_hello_to_endpoint(net.clone(), eid.clone(), name.clone());
        }
    }
}

// ── the exchange ────────────────────────────────────────────────────────────

fn sign(dir: &Path, me: &str) -> Option<String> {
    crate::link::sign_endpoint(dir, me)
}

/// Who this device is, for one exchange (kept free of the app handle so the
/// wire protocol can be exercised over real loopback connections in tests).
pub(crate) struct Ctx {
    dir: std::path::PathBuf,
    me: String,
    device: DeviceRec,
    /// Settings owner, for the shared profile (None: don't share a profile).
    st: Option<Arc<AppState>>,
    /// This device's endpoint key, to sign link proofs (S4).
    signer: Option<iroh::SecretKey>,
}

impl Ctx {
    fn from_app(app: &AppHandle, net: &IrohState) -> anyhow::Result<Ctx> {
        let st = app.try_state::<Arc<AppState>>().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.inner().clone();
        let ep = net.get().ok_or_else(|| anyhow::anyhow!("network not ready"))?;
        let me = ep.id().to_string();
        Ok(Ctx { dir: st.config_dir.clone(), device: this_device(&st, &me), me, st: Some(st), signer: Some(ep.secret_key().clone()) })
    }
    fn ensure_proofs(&self, account: &str) {
        if let Some(k) = &self.signer { ensure_proofs(&self.dir, account, k); }
    }
    fn gather(&self, account: &str) -> Local {
        let mut local = gather(&self.dir, account, &self.me, &self.device);
        local.meta.profile = self.st.as_ref().map(|st| local_profile(st, account));
        local
    }
    /// Adopt the other device's newer profile parts (name now, picture later).
    fn adopt_profile(&self, account: &str, theirs: Option<&ProfileRec>) -> ProfileApplied {
        match (&self.st, theirs) {
            (Some(st), Some(p)) => adopt_profile(st, account, p),
            _ => ProfileApplied::default(),
        }
    }
    fn avatar_payload(&self, account: &str, wanted: bool) -> Value {
        if !wanted { return Value::Null }
        self.st.as_ref().and_then(|st| profile_avatar_payload(st, account)).unwrap_or(Value::Null)
    }
    fn adopt_avatar(&self, account: &str, v: &Value) -> bool {
        !v.is_null() && self.st.as_ref().is_some_and(|st| adopt_profile_avatar(st, account, v))
    }
}

/// What one exchange changed locally.
struct Outcome {
    applied: Applied,
    chats: usize,
    /// The shared display name or picture changed on this device.
    profile_changed: bool,
}

impl Outcome {
    fn left() -> Outcome {
        Outcome { applied: Applied::left(), chats: 0, profile_changed: false }
    }
}

/// Dialer side of one exchange over an open bi-stream to own device `eid`.
async fn client_exchange(ctx: &Ctx, eid: &str, send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<Outcome> {
    let dir = &ctx.dir;
    let account = my_pub(dir).ok_or_else(|| anyhow::anyhow!("no account"))?;
    ctx.ensure_proofs(&account);
    let local = ctx.gather(&account);
    let hello = json!({
        "kind": "account-sync", "v": SYNC_V, "account": account,
        "sig": sign(dir, &ctx.me).ok_or_else(|| anyhow::anyhow!("no account key"))?,
        "meta": local.meta, "summaries": summaries(dir, &local),
    });
    iroh_net::write_frame(send, &hello).await?;
    let t = Duration::from_secs(60);
    let reply = tokio::time::timeout(t, iroh_net::read_frame_cap(recv, FRAME_CAP)).await??;
    match reply["kind"].as_str() {
        Some("account-sync-ok") => {}
        Some("account-sync-removed") => {
            // The other device says this one was removed. Only believed with a
            // signed removal from a device this one still counts in the
            // account (or from itself) — a bare reply would let any key holder
            // knock devices out (S4).
            let proof: Option<Stamp> = serde_json::from_value(reply["proof"].clone()).ok();
            let accepted = proof.is_some_and(|p| with_book(dir, &account, |b| {
                b.merge_removals(&account, &ctx.me, &HashMap::from([(ctx.me.clone(), p)])) && b.is_removed(&ctx.me)
            }));
            if !accepted {
                anyhow::bail!("account sync refused (unsigned removal)");
            }
            leave_account(dir, &account, &ctx.me);
            return Ok(Outcome::left());
        }
        other => anyhow::bail!("account sync refused ({})", other.unwrap_or("?")),
    }
    let meta: Meta = serde_json::from_value(reply["meta"].clone())?;
    let applied = apply_meta(dir, &local, eid, &meta);
    if applied.left_account {
        return Ok(Outcome { applied, chats: 0, profile_changed: false });
    }
    let profile = ctx.adopt_profile(&account, meta.profile.as_ref());
    let local = ctx.gather(&account);
    let lists: HashMap<String, Vec<(String, String)>> = serde_json::from_value(reply["lists"].clone()).unwrap_or_default();
    let (messages, want) = plan(dir, &local, &lists);
    let their_wants: Vec<String> = serde_json::from_value(reply["want_avatars"].clone()).unwrap_or_default();
    iroh_net::write_frame(send, &json!({
        "messages": messages, "want": want,
        "avatars": avatars_for(&local, &their_wants), "want_avatars": applied.want_avatars,
        "profile_avatar": ctx.avatar_payload(&account, reply["want_profile_avatar"] == true),
        "want_profile_avatar": profile.want_avatar,
    })).await?;
    send.finish()?;
    let last = tokio::time::timeout(t, iroh_net::read_frame_cap(recv, FRAME_CAP)).await??;
    let chats = apply_messages(dir, &last["messages"]);
    let avatars = apply_avatars(dir, &last["avatars"]);
    let got_avatar = ctx.adopt_avatar(&account, &last["profile_avatar"]);
    Ok(Outcome { applied: Applied { changed: applied.changed || avatars, ..applied }, chats,
        profile_changed: profile.changed || got_avatar })
}

/// Listener side of one exchange started by `who` (its first frame is `req`).
/// Ok(None) = refused (not our account, or a removed device).
async fn server_exchange(ctx: &Ctx, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<Option<Outcome>> {
    let dir = &ctx.dir;
    let account = my_pub(dir);
    let verified = account.as_deref().is_some_and(|a| req["account"].as_str() == Some(a)
        && crate::link::verify_account(a, req["sig"].as_str().unwrap_or(""), who));
    let Some(account) = account.filter(|_| verified) else {
        iroh_net::write_frame(send, &json!({"kind": "account-sync-denied"})).await?;
        send.finish()?;
        return Ok(None);
    };
    let meta: Meta = serde_json::from_value(req["meta"].clone())?;
    ctx.ensure_proofs(&account);
    // S4: decide whether the caller is still one of our devices BEFORE merging
    // anything it says. Only its SIGNED link proofs are taken first (a device
    // relinked elsewhere must not be told it's removed by a device that hasn't
    // heard yet — and a removed device can't forge another device's signature).
    let book = with_book(dir, &account, |b| {
        if b.proofs_v > 0 { b.merge_proofs(&account, &ctx.me, &meta.roster.link_proofs); }
        b.clone()
    });
    if book.proofs_v == 0 {
        // A book from before link proofs: the old order (link times first).
        if merge_roster_times(dir, &account, &meta.roster, &meta.removed_friends).is_removed(who) {
            iroh_net::write_frame(send, &json!({"kind": "account-sync-removed"})).await?;
            send.finish()?;
            return Ok(None);
        }
    } else if book.is_removed(who) {
        // Tell it with the signed removal (it won't leave on our word alone).
        let reply = match book.removal_proofs.get(who) {
            Some(p) => json!({"kind": "account-sync-removed", "proof": p}),
            None => json!({"kind": "account-sync-denied"}),
        };
        iroh_net::write_frame(send, &reply).await?;
        send.finish()?;
        return Ok(None);
    } else if !book.is_member(who) {
        // Holds the account key but no device of the account vouched for it.
        iroh_net::write_frame(send, &json!({"kind": "account-sync-denied"})).await?;
        send.finish()?;
        return Ok(None);
    }
    let local = ctx.gather(&account);
    let applied = apply_meta(dir, &local, who, &meta);
    if applied.left_account {
        iroh_net::write_frame(send, &json!({"kind": "account-sync-denied"})).await?;
        send.finish()?;
        return Ok(Some(Outcome { applied, chats: 0, profile_changed: false }));
    }
    let profile = ctx.adopt_profile(&account, meta.profile.as_ref());
    let local = ctx.gather(&account);
    let theirs: HashMap<String, String> = serde_json::from_value(req["summaries"].clone()).unwrap_or_default();
    iroh_net::write_frame(send, &json!({
        "kind": "account-sync-ok", "meta": local.meta,
        "lists": differing_lists(dir, &local, &theirs), "want_avatars": applied.want_avatars,
        "want_profile_avatar": profile.want_avatar,
    })).await?;
    let t = Duration::from_secs(60);
    let third = tokio::time::timeout(t, iroh_net::read_frame_cap(recv, FRAME_CAP)).await??;
    let chats = apply_messages(dir, &third["messages"]);
    let avatars = apply_avatars(dir, &third["avatars"]);
    let got_avatar = ctx.adopt_avatar(&account, &third["profile_avatar"]);
    let their_wants: Vec<String> = serde_json::from_value(third["want_avatars"].clone()).unwrap_or_default();
    let local = ctx.gather(&account);
    iroh_net::write_frame(send, &json!({
        "messages": wanted_messages(dir, &local, &third["want"]),
        "avatars": avatars_for(&local, &their_wants),
        "profile_avatar": ctx.avatar_payload(&account, third["want_profile_avatar"] == true),
    })).await?;
    send.finish()?;
    let _ = tokio::time::timeout(Duration::from_secs(10), send.stopped()).await;
    Ok(Some(Outcome { applied: Applied { changed: applied.changed || avatars, ..applied }, chats,
        profile_changed: profile.changed || got_avatar }))
}

/// Dialer side: run one full exchange with own device `eid`.
async fn sync_with(app: &AppHandle, net: &Arc<IrohState>, eid: &str) -> anyhow::Result<()> {
    let ctx = Ctx::from_app(app, net)?;
    let conn = iroh_net::friend_connection(net, eid).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let out = client_exchange(&ctx, eid, &mut send, &mut recv).await?;
    announce(app, net, &out);
    anyhow::ensure!(!out.applied.left_account, "removed from account");
    Ok(())
}

/// Listener side: serve one exchange started by `who`.
pub(crate) async fn serve(state: &IrohState, who: &str, req: &Value,
    send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<()> {
    let app = state.app.get().ok_or_else(|| anyhow::anyhow!("app unavailable"))?.clone();
    let net = app.try_state::<Arc<IrohState>>().ok_or_else(|| anyhow::anyhow!("network unavailable"))?.inner().clone();
    let ctx = Ctx::from_app(&app, &net)?;
    let Some(out) = server_exchange(&ctx, who, req, send, recv).await? else { return Ok(()) };
    if !out.applied.left_account {
        let mut s = status().lock().unwrap();
        s.last_ok.insert(who.to_owned(), chat::now_ms());
        s.backoff.remove(who);
    }
    announce(&app, &net, &out);
    let _ = app.emit("account://synced", who);
    Ok(())
}

// ── the loop ────────────────────────────────────────────────────────────────

async fn round(app: &AppHandle, net: &Arc<IrohState>, force: bool) {
    let Ok(ctx) = Ctx::from_app(app, net) else { return };
    let dir = ctx.dir.clone();
    let Some(account) = my_pub(&dir) else { return };
    ctx.ensure_proofs(&account);
    let fp = fingerprint(&dir, &ctx.gather(&account));
    let devices: Vec<String> = own_devices(&dir).into_iter().filter_map(|f| f.endpoint_id).collect();
    let targets: Vec<String> = {
        let mut s = status().lock().unwrap();
        let mut picked = vec![];
        for eid in devices {
            let busy = s.running.contains(&eid);
            let waiting = !force && s.backoff.get(&eid).is_some_and(|(until, _)| Instant::now() < *until);
            let in_step = !force && s.in_step.get(&eid).is_some_and(|(f, at)| *f == fp && at.elapsed() < REFRESH);
            if !busy && !waiting && !in_step {
                s.running.insert(eid.clone());
                picked.push(eid);
            }
        }
        picked
    };
    let ctx = Arc::new(ctx);
    let jobs = targets.into_iter().map(|eid| {
        let (app, net, ctx, account) = (app.clone(), net.clone(), ctx.clone(), account.clone());
        async move {
            let res = tokio::time::timeout(Duration::from_secs(150), sync_with(&app, &net, &eid)).await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
            // Fingerprint outside the status lock (it reads the stores).
            let fp = res.is_ok().then(|| fingerprint(&ctx.dir, &ctx.gather(&account)));
            let mut s = status().lock().unwrap();
            s.running.remove(&eid);
            match res {
                Ok(()) => {
                    s.in_step.insert(eid.clone(), (fp.unwrap_or_default(), Instant::now()));
                    s.last_ok.insert(eid.clone(), chat::now_ms());
                    s.backoff.remove(&eid);
                    drop(s);
                    let _ = app.emit("account://synced", &eid);
                }
                Err(e) => {
                    let fails = s.backoff.get(&eid).map_or(0, |b| b.1) + 1;
                    let wait = Duration::from_secs((30u64 << fails.min(5)).min(600));
                    s.backoff.insert(eid.clone(), (Instant::now() + wait, fails));
                    log::debug!("account sync with {} failed ({fails}x): {e:#}", &eid[..eid.len().min(10)]);
                }
            }
        }
    });
    let handles: Vec<_> = jobs.map(tauri::async_runtime::spawn).collect();
    for h in handles {
        let _ = h.await;
    }
}

/// Keep this device's account in step with the user's other devices.
pub fn spawn(app: AppHandle, net: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(12)).await;
        loop {
            let force = FORCE.swap(false, std::sync::atomic::Ordering::SeqCst);
            round(&app, &net, force).await;
            tokio::select! {
                _ = changed().notified() => {
                    // Debounce bursts (a thread of messages, a hello storm).
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(120)) => {}
            }
        }
    });
}

static FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ── commands ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DeviceView {
    friend_id: Option<String>,
    endpoint_id: String,
    name: String,
    device_kind: Option<String>,
    device_os: Option<String>,
    last_sync_ms: Option<u64>,
    this_device: bool,
    /// Proves the account key but no remaining device vouched for it: the user
    /// approves (or removes) it before it gets anything.
    needs_approval: bool,
}

pub(crate) fn device_views(st: &AppState, me: &str) -> Vec<DeviceView> {
    let s = st.settings.lock().unwrap();
    let mut out = vec![DeviceView { friend_id: None, endpoint_id: me.to_owned(), name: device_name(&s.device_kind),
        device_kind: Some(s.device_kind.clone()), device_os: Some(std::env::consts::OS.to_owned()),
        last_sync_ms: None, this_device: true, needs_approval: false }];
    drop(s);
    for f in own_devices(&st.config_dir) {
        let eid = f.endpoint_id.clone().unwrap_or_default();
        out.push(DeviceView { friend_id: Some(f.id), last_sync_ms: last_sync(&eid), endpoint_id: eid, name: f.name,
            device_kind: f.device_kind, device_os: f.device_os, this_device: false, needs_approval: false });
    }
    if let Some(account) = my_pub(&st.config_dir) {
        let b = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(&st.config_dir, &account) };
        for (eid, p) in b.pending_devices.iter().filter(|(e, _)| !b.is_member(e) && !b.is_removed(e)) {
            out.push(DeviceView { friend_id: None, endpoint_id: eid.clone(), name: p.name.clone(), device_kind: p.kind.clone(),
                device_os: p.os.clone(), last_sync_ms: None, this_device: false, needs_approval: true });
        }
    }
    out
}

/// Sync with every own device now (pull-to-refresh / "Sync now").
#[tauri::command]
pub fn account_sync_now() {
    {
        let mut s = status().lock().unwrap();
        s.backoff.clear();
        s.in_step.clear();
    }
    FORCE.store(true, std::sync::atomic::Ordering::SeqCst);
    note_change();
}

/// Remove one of the user's devices from the account, on every device.
#[tauri::command]
pub fn account_remove_device(app: AppHandle, state: State<'_, Arc<AppState>>, endpoint_id: String) -> Result<(), String> {
    let dir = &state.config_dir;
    let net = app.try_state::<Arc<IrohState>>().map(|n| n.inner().clone());
    let signer = net.as_ref().and_then(|n| n.get().map(|e| e.secret_key().clone()));
    remove_device(dir, &endpoint_id, signer.as_ref())?;
    let _ = app.emit("friends://changed", ());
    account_sync_now();
    // Friends hear it in our next hello (S4): the removed device can no longer
    // speak for this person to them.
    if let Some(net) = net { iroh_net::broadcast_profile(app.clone(), net); }
    Ok(())
}

/// Remove `eid` from the account here, with a removal signed by this device
/// (the other devices and friends only believe signed removals).
pub(crate) fn remove_device(dir: &Path, eid: &str, signer: Option<&iroh::SecretKey>) -> Result<(), String> {
    let account = my_pub(dir).ok_or("This device isn't linked to an account.")?;
    with_book(dir, &account, |b| {
        let at = chat::now_ms().max(b.removed_devices.get(eid).copied().unwrap_or(0)).max(b.linked.get(eid).map_or(0, |l| l.saturating_add(1)));
        b.removed_devices.insert(eid.to_owned(), at);
        if let Some(k) = signer { b.removal_proofs.insert(eid.to_owned(), sign_removal(k, &account, eid, at)); }
        b.pending_devices.remove(eid);
    });
    if let Some(f) = friends::load(dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(eid)) {
        friends::remove(dir, &f.id)?;
    }
    note_change();
    Ok(())
}

/// The user approved a device that proves the account key but that no
/// remaining device had vouched for ("Needs approval" in Devices): vouch for it.
pub(crate) fn approve_device(dir: &Path, eid: &str, signer: &iroh::SecretKey) -> Result<(), String> {
    let account = my_pub(dir).ok_or("This device isn't linked to an account.")?;
    let pending = with_book(dir, &account, |b| {
        let p = b.pending_devices.remove(eid);
        let at = chat::now_ms().max(b.linked.get(eid).copied().unwrap_or(0).saturating_add(1))
            .max(b.removed_devices.get(eid).map_or(0, |r| r.saturating_add(1)));
        if p.is_some() { b.linked.insert(eid.to_owned(), at); }
        if p.is_some() { b.add_voucher(eid, sign_stamp(signer, &account, eid, at)); }
        p
    }).ok_or("That device isn't waiting for approval.")?;
    friends::upsert_own_device(dir, eid, &pending.name, pending.kind.as_deref(), pending.os.as_deref(), &account, chat::now_ms(), true);
    note_change();
    Ok(())
}

#[tauri::command]
pub fn account_approve_device(app: AppHandle, state: State<'_, Arc<AppState>>, endpoint_id: String) -> Result<(), String> {
    let signer = app.try_state::<Arc<IrohState>>().and_then(|n| n.get().map(|e| e.secret_key().clone())).ok_or("DropBeam is still connecting — try again in a moment.")?;
    approve_device(&state.config_dir, &endpoint_id, &signer)?;
    let _ = app.emit("friends://changed", ());
    account_sync_now();
    Ok(())
}

/// A device proved this account's key (signed hello) but isn't vouched for:
/// list it as "Needs approval" (never as one of the user's devices yet).
pub(crate) fn note_pending_device(dir: &Path, eid: &str, name: &str, kind: Option<&str>, os: Option<&str>) {
    let Some(account) = my_pub(dir) else { return };
    with_book(dir, &account, |b| {
        if b.proofs_v == 0 || b.is_member(eid) || b.is_removed(eid) || eid == b.me { return; }
        if b.pending_devices.len() >= 16 && !b.pending_devices.contains_key(eid) { return; }
        b.pending_devices.insert(eid.to_owned(), PendingDevice { name: friends::sanitize_display_name(name, "Unknown device"),
            kind: kind.map(|k| k.chars().take(16).collect()), os: os.map(|o| o.chars().take(16).collect()), at: chat::now_ms() });
    });
}

/// The signed removals of this account's devices, for friends (S4): told in
/// every hello, so a removed device can't keep speaking for this person.
pub(crate) fn revocation_notice(dir: &Path) -> Value {
    let Some(account) = my_pub(dir) else { return Value::Null };
    let b = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); read_book(dir, &account) };
    let mut list: Vec<(&String, &Stamp)> = b.removal_proofs.iter().filter(|(e, _)| b.is_removed(e)).collect();
    list.sort_by_key(|(_, s)| std::cmp::Reverse(s.at));
    list.truncate(32);
    if list.is_empty() { return Value::Null; }
    json!(list.into_iter().map(|(e, s)| json!({"eid": e, "at": s.at, "by": s.by, "sig": s.sig})).collect::<Vec<_>>())
}

/// Take this device out of its account (it keeps its data as a standalone device).
#[tauri::command]
pub async fn account_leave(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: State<'_, Arc<IrohState>>) -> Result<(), String> {
    let dir = state.config_dir.clone();
    let account = my_pub(&dir).ok_or("This device isn't linked to an account.")?;
    let me = iroh.get().ok_or("Network not ready")?.id().to_string();
    // Tell the other devices first (best effort), then forget the key. The
    // removal is signed by this device, so they believe it.
    let signer = iroh.get().map(|e| e.secret_key().clone());
    with_book(&dir, &account, |b| {
        let at = chat::now_ms().max(b.linked.get(&me).map_or(0, |l| l.saturating_add(1)));
        b.removed_devices.insert(me.clone(), at);
        if let Some(k) = &signer { b.removal_proofs.insert(me.clone(), sign_removal(k, &account, &me, at)); }
    });
    let net = iroh.inner().clone();
    let targets: Vec<String> = own_devices(&dir).into_iter().filter_map(|f| f.endpoint_id).collect();
    let handles: Vec<_> = targets.into_iter().map(|eid| {
        let (app, net) = (app.clone(), net.clone());
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(20), sync_with(&app, &net, &eid)).await;
        })
    }).collect();
    for h in handles {
        let _ = h.await;
    }
    if my_pub(&dir).is_some() {
        leave_account(&dir, &account, &me);
    }
    let _ = app.emit("friends://changed", ());
    let _ = app.emit("account://left", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("db-account-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn eid() -> String {
        iroh::SecretKey::generate().public().to_string()
    }
    fn local(dir: &Path, account: &str, me: &str) -> Local {
        gather(dir, account, me, &DeviceRec { eid: me.to_owned(), name: "Test".into(), kind: Some("laptop".into()), os: Some("macos".into()) })
    }
    fn text(id: &str, peer: &str, from_me: bool, ts: u64) -> chat::ChatMessage {
        serde_json::from_value(json!({"id": id, "peerId": peer, "fromMe": from_me, "kind": "text",
            "text": format!("m{id}"), "files": [], "bytes": 0, "status": if from_me { Some("sending") } else { None }, "ts": ts, "seq": ts})).unwrap()
    }

    #[test]
    fn book_link_and_removal_ordering() {
        let mut b = Book::default();
        assert!(!b.is_removed("x"));
        b.removed_devices.insert("x".into(), 10);
        assert!(b.is_removed("x"));
        b.linked.insert("x".into(), 11);
        assert!(!b.is_removed("x"), "a relink after the removal wins");
        b.removed_devices.insert("x".into(), 11);
        assert!(b.is_removed("x"), "a removal at the link instant removes");
    }

    #[test]
    fn two_devices_converge_friends_and_chats() {
        let (a, b) = (dir(), dir());
        let key = iroh::SecretKey::generate();
        let account = hex::encode(key.public().as_bytes());
        for d in [&a, &b] { crate::link::adopt_key_for_tests(d, &key); }
        let (me_a, me_b, f1, f2) = (eid(), eid(), eid(), eid());
        // A knows friend 1 with a thread; B knows friend 2 with a thread.
        let fa = friends::upsert_by_endpoint(&a, &f1, "Mong");
        chat::append(&a, &text("1", &fa.id, true, 1));
        chat::append(&a, &text("2", &fa.id, false, 2));
        let fb = friends::upsert_by_endpoint(&b, &f2, "Ethan");
        chat::append(&b, &text("3", &fb.id, false, 3));
        // B → A meta, then A → B meta (both directions of one exchange).
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        apply_meta(&a, &la, &me_b, &lb.meta);
        apply_meta(&b, &lb, &me_a, &la.meta);
        // Each now knows both friends and the other device.
        for (d, other) in [(&a, &me_b), (&b, &me_a)] {
            let all = friends::load(d);
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(f1.as_str())));
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(f2.as_str())));
            assert!(all.iter().any(|f| f.endpoint_id.as_deref() == Some(other.as_str()) && f.account_pub.as_deref() == Some(account.as_str())));
        }
        // Chat anti-entropy: A's lists vs B's summaries, both ways.
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        let lists_from_b = differing_lists(&b, &lb, &summaries(&a, &la));
        let (to_b, a_wants) = plan(&a, &la, &lists_from_b);
        let to_b = serde_json::to_value(&to_b).unwrap();
        assert!(apply_messages(&b, &to_b) >= 2);
        let back = wanted_messages(&b, &lb, &serde_json::to_value(&a_wants).unwrap());
        assert!(apply_messages(&a, &serde_json::to_value(&back).unwrap()) >= 1);
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        assert_eq!(summaries(&a, &la), summaries(&b, &lb), "threads converge");
        // A copy that was still "sending" on A reads "sent" on B (no double send).
        let fb1 = friends::load(&b).into_iter().find(|f| f.endpoint_id.as_deref() == Some(f1.as_str())).unwrap();
        let m = chat::messages(&b, &fb1.id).into_iter().find(|m| m.id == "1").unwrap();
        assert_eq!(m.status.as_deref(), Some("sent"));
        assert!(chat::outbox(&b).is_empty());
        // Nothing left to exchange.
        assert!(differing_lists(&b, &lb, &summaries(&a, &la)).is_empty());
        for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn synced_messages_slot_in_by_time_not_by_the_other_devices_seq() {
        let d = dir();
        let mut local = text("mine", "p", true, 100);
        local.seq = 3;
        chat::append(&d, &local);
        let mut later = text("later", "p", true, 300);
        later.seq = 4;
        chat::append(&d, &later);
        // From a device whose clock for this thread is far ahead.
        let mut theirs = text("theirs", "p", false, 200);
        theirs.seq = 900;
        chat::merge_synced(&d, "p", vec![theirs]);
        let order: Vec<String> = chat::messages(&d, "p").into_iter().map(|m| m.id).collect();
        assert_eq!(order, ["mine", "theirs", "later"]);
        assert!(chat::next_seq(&d, "p") > 4);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn unsend_is_sticky_even_against_a_newer_reaction() {
        let d = dir();
        let mut m = text("x", "p", false, 5);
        m.deleted = true;
        m.text.clear();
        m.rev = 10;
        chat::append(&d, &m);
        let mut reacted = text("x", "p", false, 5);
        reacted.rev = 99;
        reacted.reactions = vec![chat::Reaction { emoji: "❤️".into(), from_me: true }];
        chat::merge_synced(&d, "p", vec![reacted]);
        let got = chat::messages(&d, "p").remove(0);
        assert!(got.deleted && got.text.is_empty(), "a friend's unsend never comes back");
        // …and the undeleted copy adopts the unsend.
        let e = dir();
        chat::append(&e, &text("x", "p", false, 5));
        chat::merge_synced(&e, "p", vec![m]);
        assert!(chat::messages(&e, "p")[0].deleted);
        for d in [d, e] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn future_timestamps_are_clamped_and_relinks_are_learned_before_refusing() {
        let d = dir();
        let key = iroh::SecretKey::generate();
        crate::link::adopt_key_for_tests(&d, &key);
        let account = hex::encode(key.public().as_bytes());
        with_book(&d, &account, |b| { b.removed_devices.insert("ipad".into(), 1_000); });
        // A roster from a device that already saw the relink (and one with a wild clock).
        let roster = Roster { linked: HashMap::from([("ipad".into(), 2_000), ("evil".into(), u64::MAX)]), ..Default::default() };
        let book = merge_roster_times(&d, &account, &roster, &HashMap::new());
        assert!(!book.is_removed("ipad"), "relink learned before the removed check");
        assert!(book.linked["evil"] <= chat::now_ms() + SKEW_MS, "far-future times are clamped");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Older builds send no `blocked` field (and ignore ours): both directions parse.
    #[test]
    fn meta_blocks_are_backward_compatible() {
        let old: Meta = serde_json::from_value(json!({"roster": {"me": {"eid": "x"}}, "friends": [], "removed_friends": {}})).unwrap();
        assert!(old.blocked.is_empty());
        assert!(serde_json::to_value(&old).unwrap().get("blocked").is_none(), "nothing extra when nobody is blocked");
        let mut new = Meta::default();
        new.blocked.insert("e".into(), crate::block::BlockRec { name: "X".into(), account: None, blocked: true, at: 5 });
        let v = serde_json::to_value(&new).unwrap();
        assert_eq!(v["blocked"]["e"]["blocked"], true);
        // An older build's Meta (no `blocked`) still reads a newer one.
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldMeta { roster: Roster, #[serde(default)] friends: Vec<FriendRec> }
        assert!(serde_json::from_value::<OldMeta>(v).is_ok());
    }

    #[test]
    fn a_removed_friend_is_not_re_added_by_a_stray_hello() {
        let d = dir();
        crate::link::adopt_key_for_tests(&d, &iroh::SecretKey::generate());
        let f1 = eid();
        let f = friends::upsert_by_endpoint(&d, &f1, "Mong");
        record_friend_removed(&d, &f);
        friends::remove(&d, &f.id).unwrap();
        friends::apply_hello(&d, "", &f1, "Mong");
        assert!(friends::load(&d).is_empty());
        // Adding them again on purpose still works.
        friends::upsert_by_endpoint(&d, &f1, "Mong");
        assert_eq!(friends::load(&d).len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn edits_win_by_rev_and_status_only_moves_forward() {
        let d = dir();
        let mut m = text("x", "p", true, 5);
        m.status = Some("read".into());
        chat::append(&d, &m);
        let mut older = m.clone();
        older.status = Some("delivered".into());
        older.text = "stale".into();
        assert_eq!(chat::merge_synced(&d, "p", vec![older]), 0);
        let mut newer = m.clone();
        newer.text = "edited".into();
        newer.edited = true;
        newer.rev = 99;
        assert_eq!(chat::merge_synced(&d, "p", vec![newer]), 1);
        let got = chat::messages(&d, "p").remove(0);
        assert_eq!(got.text, "edited");
        assert_eq!(got.status.as_deref(), Some("read"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn removed_friend_and_removed_device_propagate() {
        let (a, b) = (dir(), dir());
        let key = iroh::SecretKey::generate();
        let account = hex::encode(key.public().as_bytes());
        for d in [&a, &b] { crate::link::adopt_key_for_tests(d, &key); }
        let (me_a, me_b, f1, phone) = (eid(), eid(), eid(), eid());
        let fa = friends::upsert_by_endpoint(&a, &f1, "Mong");
        friends::upsert_by_endpoint(&b, &f1, "Mong");
        friends::upsert_own_device(&a, &phone, "Phone", Some("phone"), Some("ios"), &account, 1, true);
        friends::upsert_own_device(&b, &phone, "Phone", Some("phone"), Some("ios"), &account, 1, true);
        // A removes the friend and the phone.
        std::thread::sleep(Duration::from_millis(5));
        record_friend_removed(&a, &fa);
        friends::remove(&a, &fa.id).unwrap();
        with_book(&a, &account, |bk| { bk.removed_devices.insert(phone.clone(), chat::now_ms()); });
        let la = local(&a, &account, &me_a);
        let lb = local(&b, &account, &me_b);
        apply_meta(&b, &lb, &me_a, &la.meta);
        let all = friends::load(&b);
        assert!(!all.iter().any(|f| f.endpoint_id.as_deref() == Some(f1.as_str())), "friend removal propagates");
        assert!(!all.iter().any(|f| f.endpoint_id.as_deref() == Some(phone.as_str())), "device removal propagates");
        // The removed phone itself learns it left the account.
        let p = dir();
        crate::link::adopt_key_for_tests(&p, &key);
        let lp = local(&p, &account, &phone);
        let applied = apply_meta(&p, &lp, &me_a, &la.meta);
        assert!(applied.left_account);
        assert!(my_pub(&p).is_none());
        for d in [a, b, p] { let _ = std::fs::remove_dir_all(d); }
    }

    /// The whole 4-frame exchange over a real (loopback) iroh connection.
    #[tokio::test]
    async fn wire_exchange_converges_over_loopback() {
        use iroh::endpoint::presets;
        let (a, b) = (dir(), dir());
        let key = iroh::SecretKey::generate();
        for d in [&a, &b] { crate::link::adopt_key_for_tests(d, &key); }
        let server = iroh::Endpoint::builder(presets::N0).alpns(vec![iroh_net::ALPN.to_vec()]).bind().await.unwrap();
        let client = iroh::Endpoint::bind(presets::N0).await.unwrap();
        let (me_a, me_b) = (client.id().to_string(), server.id().to_string());
        let (f1, f2) = (eid(), eid());
        let fa = friends::upsert_by_endpoint(&a, &f1, "Mong");
        for i in 0..30 { chat::append(&a, &text(&format!("a{i}"), &fa.id, i % 2 == 0, i)); }
        let fb = friends::upsert_by_endpoint(&b, &f2, "Ethan");
        chat::append(&b, &text("b1", &fb.id, false, 3));
        let dev = |me: &str, name: &str| DeviceRec { eid: me.to_owned(), name: name.into(), kind: Some("laptop".into()), os: Some("macos".into()) };
        let ctx_a = Ctx { dir: a.clone(), me: me_a.clone(), device: dev(&me_a, "Mac"), st: None, signer: None };
        let ctx_b = Ctx { dir: b.clone(), me: me_b.clone(), device: dev(&me_b, "iPhone"), st: None, signer: None };
        let addr = server.addr();
        let srv = server.clone();
        let served = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            assert_eq!(req["kind"], "account-sync");
            let out = server_exchange(&ctx_b, &who, &req, &mut send, &mut recv).await.unwrap().unwrap();
            out.chats
        });
        let conn = client.connect(addr, iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let out = client_exchange(&ctx_a, &me_b, &mut send, &mut recv).await.unwrap();
        let server_chats = served.await.unwrap();
        assert_eq!(server_chats, 30, "B received A's whole thread");
        assert_eq!(out.chats, 1, "A received B's thread");
        let account = my_pub(&a).unwrap();
        let (la, lb) = (local(&a, &account, &me_a), local(&b, &account, &me_b));
        assert_eq!(summaries(&a, &la), summaries(&b, &lb));
        // Each side lists the other as an own device, labelled from the roster.
        let mac_on_b = friends::load(&b).into_iter().find(|f| f.endpoint_id.as_deref() == Some(me_a.as_str())).unwrap();
        assert_eq!(mac_on_b.account_pub.as_deref(), Some(account.as_str()));
        assert_eq!(mac_on_b.device_os.as_deref(), Some("macos"));
        // A second exchange has nothing left to move.
        let srv = server.clone();
        let ctx_b2 = Ctx { dir: b.clone(), me: me_b.clone(), device: dev(&me_b, "iPhone"), st: None, signer: None };
        let served = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            server_exchange(&ctx_b2, &who, &req, &mut send, &mut recv).await.unwrap().unwrap().chats
        });
        let conn = client.connect(server.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        assert_eq!(client_exchange(&ctx_a, &me_b, &mut send, &mut recv).await.unwrap().chats, 0);
        assert_eq!(served.await.unwrap(), 0);
        // A device with a different account key is refused.
        let c = dir();
        crate::link::adopt_key_for_tests(&c, &iroh::SecretKey::generate());
        let ctx_c = Ctx { dir: c.clone(), me: me_a.clone(), device: dev(&me_a, "Stranger"), st: None, signer: None };
        let srv = server.clone();
        let ctx_b3 = Ctx { dir: b.clone(), me: me_b.clone(), device: dev(&me_b, "iPhone"), st: None, signer: None };
        let served = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = iroh_net::read_frame(&mut recv).await.unwrap();
            server_exchange(&ctx_b3, &who, &req, &mut send, &mut recv).await.unwrap().is_none()
        });
        let conn = client.connect(server.addr(), iroh_net::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        assert!(client_exchange(&ctx_c, &me_b, &mut send, &mut recv).await.is_err());
        assert!(served.await.unwrap(), "stranger refused");
        for d in [a, b, c] { let _ = std::fs::remove_dir_all(d); }
    }
}

/// Real devices for tests: a config dir, an iroh endpoint on loopback and an
/// AppState, plus one full account-sync exchange between two of them.
#[cfg(test)]
mod stale_name_tests {
    #[test]
    fn a_stale_window_cannot_undo_a_synced_rename() {
        super::note_replaced_name("Old Name", "New Name");
        assert!(super::stale_display_name("Old Name", "New Name"));
        assert!(!super::stale_display_name("Something Else", "New Name"), "a real rename goes through");
        assert!(!super::stale_display_name("Old Name", "Third"), "only right after that sync");
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use iroh::endpoint::presets;

    pub(crate) struct Dev {
        pub dir: std::path::PathBuf,
        pub ep: iroh::Endpoint,
        pub st: Arc<AppState>,
        pub label: String,
    }

    impl Dev {
        /// A device named `label` (its device name), optionally in `key`'s account.
        pub(crate) async fn new(label: &str, kind: &str, key: Option<&iroh::SecretKey>) -> Dev {
            let dir = std::env::temp_dir().join(format!("db-acct-dev-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(k) = key { crate::link::adopt_key_for_tests(&dir, k); }
            let ep = iroh::Endpoint::builder(presets::N0).alpns(vec![iroh_net::ALPN.to_vec()]).bind().await.unwrap();
            let st = Arc::new(AppState::for_tests(dir.clone()));
            { let mut s = st.settings.lock().unwrap(); s.display_name = label.to_owned(); s.device_kind = kind.to_owned(); }
            Dev { dir, ep, st, label: label.to_owned() }
        }
        pub(crate) fn eid(&self) -> String {
            self.ep.id().to_string()
        }
        pub(crate) fn ctx(&self) -> Ctx {
            let kind = self.st.settings.lock().unwrap().device_kind.clone();
            Ctx { dir: self.dir.clone(), me: self.eid(), st: Some(self.st.clone()), signer: Some(self.ep.secret_key().clone()),
                device: DeviceRec { eid: self.eid(), name: self.label.clone(), kind: Some(kind), os: Some("macos".into()) } }
        }
        pub(crate) fn friend(&self, eid: &str) -> Option<Friend> {
            friends::load(&self.dir).into_iter().find(|f| f.endpoint_id.as_deref() == Some(eid))
        }
        pub(crate) fn thread(&self, eid: &str) -> Vec<chat::ChatMessage> {
            let Some(f) = self.friend(eid) else { return vec![] };
            let owner = friends::thread_owner(&self.dir, &f.id).map_or(f.id, |o| o.id);
            chat::messages(&self.dir, &owner)
        }
        pub(crate) fn own_device_ids(&self) -> Vec<String> {
            let mut v: Vec<String> = own_devices(&self.dir).into_iter().filter_map(|f| f.endpoint_id).collect();
            v.sort();
            v
        }
    }

    impl Drop for Dev {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// What one exchange did on each side (None on the server = it refused).
    pub(crate) struct Synced {
        pub client: anyhow::Result<(usize, bool, bool)>,
        pub server: Option<(usize, bool, bool)>,
    }

    /// `a` dials `b` and they run one full account-sync exchange.
    /// Each side reports (messages merged, profile changed, left the account).
    pub(crate) async fn sync(a: &Dev, b: &Dev) -> Synced {
        // Two test devices made with the same key were "linked" before: each
        // vouches for the other the first time they meet (a real link does
        // this in link.rs). Never for a device the book already knows of —
        // removals and relinks stay exactly as the test set them up.
        vouch(a, b);
        vouch(b, a);
        sync_unvouched(a, b).await
    }

    /// `sync` without the test-only vouching: what a device that was never
    /// linked (it just holds the account key) gets.
    pub(crate) async fn sync_unvouched(a: &Dev, b: &Dev) -> Synced {
        let (srv, ctx_b) = (b.ep.clone(), b.ctx());
        let served = tokio::spawn(async move {
            let conn = srv.accept().await.ok_or_else(|| anyhow::anyhow!("closed"))?.await?;
            let who = conn.remote_id().to_string();
            let (mut send, mut recv) = conn.accept_bi().await?;
            let req = iroh_net::read_frame(&mut recv).await?;
            let out = server_exchange(&ctx_b, &who, &req, &mut send, &mut recv).await?;
            // Keep the connection up until the client has read everything.
            let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
            anyhow::Ok((out.map(|o| (o.chats, o.profile_changed, o.applied.left_account)), conn))
        });
        let client = async {
            let conn = a.ep.connect(b.ep.addr(), iroh_net::ALPN).await?;
            let (mut send, mut recv) = conn.open_bi().await?;
            let out = client_exchange(&a.ctx(), &b.eid(), &mut send, &mut recv).await?;
            anyhow::Ok((out.chats, out.profile_changed, out.applied.left_account))
        }.await;
        let server = served.await.ok().and_then(|r| r.ok()).and_then(|(o, _conn)| o);
        Synced { client, server }
    }

    pub(crate) fn vouch(d: &Dev, other: &Dev) {
        let Some(account) = my_pub(&d.dir) else { return };
        if my_pub(&other.dir).as_deref() != Some(account.as_str()) { return; }
        let known = { let _g = BOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner()); let b = read_book(&d.dir, &account);
            b.linked.contains_key(&other.eid()) || b.removed_devices.contains_key(&other.eid()) };
        if !known { mark_linked(&d.dir, &other.eid()); }
    }

    pub(crate) fn text(id: &str, peer: &str, from_me: bool, ts: u64) -> chat::ChatMessage {
        serde_json::from_value(json!({"id": id, "peerId": peer, "fromMe": from_me, "kind": "text",
            "text": format!("m{id}"), "files": [], "bytes": 0, "status": if from_me { Some("delivered") } else { None },
            "ts": ts, "seq": ts})).unwrap()
    }

    pub(crate) fn eid() -> String {
        iroh::SecretKey::generate().public().to_string()
    }
}

/// Edge cases of the multi-device account, over real loopback exchanges.
#[cfg(test)]
mod edge_tests {
    use super::testkit::*;
    use super::*;

    fn key() -> iroh::SecretKey {
        iroh::SecretKey::generate()
    }
    fn names(d: &Dev) -> Vec<(String, String)> {
        let mut v: Vec<_> = friends::load(&d.dir).into_iter()
            .filter(|f| f.account_pub.as_deref() != my_pub(&d.dir).as_deref())
            .map(|f| (f.endpoint_id.unwrap_or_default(), f.name)).collect();
        v.sort();
        v
    }
    fn ids(thread: &[chat::ChatMessage]) -> Vec<String> {
        thread.iter().map(|m| m.id.clone()).collect()
    }

    /// A and C never talk directly; B relays devices, friends and chats.
    #[tokio::test]
    async fn three_devices_converge_through_the_middle_one() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("iPad", "tablet", Some(&k)).await);
        let (mong, ethan) = (eid(), eid());
        let fa = friends::upsert_by_endpoint(&a.dir, &mong, "Mong");
        chat::append(&a.dir, &text("a1", &fa.id, true, 10));
        let fc = friends::upsert_by_endpoint(&c.dir, &ethan, "Ethan");
        chat::append(&c.dir, &text("c1", &fc.id, false, 20));
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(sync(&c, &b).await.client.is_ok());
        assert!(sync(&a, &b).await.client.is_ok());
        for d in [&a, &b, &c] {
            assert_eq!(names(d), { let mut v = vec![(mong.clone(), "Mong".to_string()), (ethan.clone(), "Ethan".to_string())]; v.sort(); v }, "{}", d.label);
            assert_eq!(ids(&d.thread(&mong)), ["a1"], "{}", d.label);
            assert_eq!(ids(&d.thread(&ethan)), ["c1"], "{}", d.label);
        }
        // A and C know each other as own devices without ever connecting.
        let mut want = vec![b.eid(), c.eid()]; want.sort();
        assert_eq!(a.own_device_ids(), want);
        let mut want = vec![a.eid(), b.eid()]; want.sort();
        assert_eq!(c.own_device_ids(), want);
        // Nothing left to move: another round is quiet.
        let again = sync(&a, &b).await;
        assert_eq!(again.client.unwrap().0, 0);
        assert_eq!(again.server.unwrap().0, 0);
    }

    /// One device offline for days: removals, a removal undone by a re-add,
    /// renames and new chats on both sides all land correctly when it returns.
    #[tokio::test]
    async fn a_device_back_after_days_offline_catches_up_both_ways() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let (gone, back, zed, newbie) = (eid(), eid(), eid(), eid());
        for (e, n) in [(&gone, "Gone"), (&back, "Back"), (&zed, "Zed")] { friends::upsert_by_endpoint(&a.dir, e, n); }
        assert!(sync(&a, &b).await.client.is_ok());
        assert_eq!(names(&b).len(), 3);
        // --- B goes offline. On A: ---
        std::thread::sleep(Duration::from_millis(5));
        let g = a.friend(&gone).unwrap();
        record_friend_removed(&a.dir, &g); friends::remove(&a.dir, &g.id).unwrap();
        let bk = a.friend(&back).unwrap();
        record_friend_removed(&a.dir, &bk); friends::remove(&a.dir, &bk.id).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        friends::add_by_code(&a.dir, &friends::my_code("Back", &back)).unwrap();
        let z = a.friend(&zed).unwrap();
        friends::rename(&a.dir, &z.id, "Zeddy".into()).unwrap();
        chat::append(&a.dir, &text("a-new", &z.id, true, 1_000));
        // --- On B, meanwhile: a message from the soon-removed friend, a new friend. ---
        let gb = b.friend(&gone).unwrap();
        chat::append(&b.dir, &text("from-gone", &gb.id, false, 900));
        let nb = friends::upsert_by_endpoint(&b.dir, &newbie, "Newbie");
        chat::append(&b.dir, &text("b-new", &nb.id, true, 950));
        // --- B returns. ---
        assert!(sync(&b, &a).await.client.is_ok());
        assert!(sync(&a, &b).await.client.is_ok());
        for d in [&a, &b] {
            let n = names(d);
            assert!(!n.iter().any(|(e, _)| *e == gone), "{}: removed friend stays removed", d.label);
            assert!(n.contains(&(back.clone(), "Back".into())), "{}: re-added friend survives the older removal", d.label);
            assert!(n.contains(&(zed.clone(), "Zeddy".into())), "{}: rename made while offline", d.label);
            assert!(n.contains(&(newbie.clone(), "Newbie".into())), "{}: friend added while offline", d.label);
            assert_eq!(ids(&d.thread(&zed)), ["a-new"], "{}", d.label);
            assert_eq!(ids(&d.thread(&newbie)), ["b-new"], "{}", d.label);
        }
        // The removed friend's message stays in B's history (detached), and a
        // stray hello from them doesn't bring them back.
        friends::apply_hello(&b.dir, "", &gone, "Gone");
        assert!(b.friend(&gone).is_none());
    }

    /// Two devices rename the same friend: the newest rename wins everywhere,
    /// and a later rename still travels (it used to stick after the first).
    #[tokio::test]
    async fn concurrent_renames_resolve_to_the_newest_everywhere() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let mong = eid();
        friends::upsert_by_endpoint(&a.dir, &mong, "Mong");
        assert!(sync(&a, &b).await.client.is_ok());
        friends::rename(&a.dir, &a.friend(&mong).unwrap().id, "Mongo".into()).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        friends::rename(&b.dir, &b.friend(&mong).unwrap().id, "M".into()).unwrap();
        assert!(sync(&a, &b).await.client.is_ok());
        assert_eq!(a.friend(&mong).unwrap().name, "M");
        assert_eq!(b.friend(&mong).unwrap().name, "M");
        std::thread::sleep(Duration::from_millis(5));
        friends::rename(&a.dir, &a.friend(&mong).unwrap().id, "Mong B".into()).unwrap();
        assert!(sync(&b, &a).await.client.is_ok());
        assert_eq!(b.friend(&mong).unwrap().name, "Mong B");
        // A friend's own broadcast name never undoes the rename.
        friends::apply_hello(&b.dir, "", &mong, "Mong's MacBook");
        assert_eq!(b.friend(&mong).unwrap().name, "Mong B");
    }

    /// A friend removed on B while their message lands on A: the removal wins,
    /// the message isn't lost from A's history, and nothing resurrects them.
    #[tokio::test]
    async fn removal_on_one_device_beats_a_message_arriving_on_another() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        friends::upsert_by_endpoint(&a.dir, &f, "Spammer");
        assert!(sync(&a, &b).await.client.is_ok());
        let on_b = b.friend(&f).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        record_friend_removed(&b.dir, &on_b); friends::remove(&b.dir, &on_b.id).unwrap();
        // B no longer accepts chat from them (the handler answers applied:false,
        // so their app tries the person's next device instead).
        assert!(friends::chat_sender(&b.dir, &f).is_none());
        let on_a = a.friend(&f).unwrap();
        chat::append(&a.dir, &text("late", &on_a.id, false, 5));
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(a.friend(&f).is_none() && b.friend(&f).is_none());
        assert_eq!(chat::messages(&a.dir, &on_a.id).len(), 1, "history kept, just detached");
        // Adding them back on purpose restores the thread.
        let again = friends::add_by_code(&a.dir, &friends::my_code("Spammer", &f)).unwrap();
        assert_eq!(chat::messages(&a.dir, &again.id).len(), 1);
    }

    /// Messages from each device slot in by time on the other, both ways.
    #[tokio::test]
    async fn messages_from_both_devices_interleave_in_time_order() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        let fa = friends::upsert_by_endpoint(&a.dir, &f, "Mong");
        let fb = friends::upsert_by_endpoint(&b.dir, &f, "Mong");
        let mut m = text("m1", &fa.id, true, 100); m.seq = chat::next_seq(&a.dir, &fa.id); chat::append(&a.dir, &m);
        let mut m = text("f1", &fb.id, false, 150); m.seq = chat::next_seq(&b.dir, &fb.id); chat::append(&b.dir, &m);
        let mut m = text("m2", &fa.id, true, 200); m.seq = chat::next_seq(&a.dir, &fa.id); chat::append(&a.dir, &m);
        let mut m = text("b1", &fb.id, true, 250); m.seq = chat::next_seq(&b.dir, &fb.id); chat::append(&b.dir, &m);
        assert!(sync(&a, &b).await.client.is_ok());
        assert_eq!(ids(&a.thread(&f)), ["m1", "f1", "m2", "b1"]);
        assert_eq!(ids(&b.thread(&f)), ["m1", "f1", "m2", "b1"]);
        // The next message on either device sorts after all of them.
        assert!(chat::next_seq(&b.dir, &fb.id) > b.thread(&f).iter().map(|m| m.seq).max().unwrap());
    }

    /// Reactions, edits and unsends made on B reach A; a copy still "sending"
    /// on A is never re-sent by B; an unsent-before-delivery message is never
    /// delivered at all.
    #[tokio::test]
    async fn changes_on_one_device_propagate_and_nothing_is_sent_twice() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        let fa = friends::upsert_by_endpoint(&a.dir, &f, "Mong");
        let mut pending = text("pending", &fa.id, true, 1); pending.status = Some("sending".into());
        chat::append(&a.dir, &pending);
        chat::append(&a.dir, &text("mine", &fa.id, true, 2));
        chat::append(&a.dir, &text("theirs", &fa.id, false, 3));
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(chat::outbox(&b.dir).is_empty(), "B never re-sends A's pending message");
        assert_eq!(chat::outbox(&a.dir).len(), 1, "A still owns its retry");
        let fb = b.friend(&f).unwrap();
        chat::apply_reaction(&b.dir, &fb.id, "theirs", "❤️", true, true);
        chat::apply_edit(&b.dir, &fb.id, "mine", "edited on B", true);
        chat::apply_delete(&b.dir, &fb.id, "pending", true);
        assert!(sync(&b, &a).await.client.is_ok());
        let got = a.thread(&f);
        assert_eq!(got.iter().find(|m| m.id == "theirs").unwrap().reactions, vec![chat::Reaction { emoji: "❤️".into(), from_me: true }]);
        let mine = got.iter().find(|m| m.id == "mine").unwrap();
        assert!(mine.edited && mine.text == "edited on B");
        assert!(got.iter().find(|m| m.id == "pending").unwrap().deleted);
        assert!(chat::outbox(&a.dir).is_empty(), "unsent before delivery: never delivered");
    }

    /// Full threads: the newest window syncs, and the exchange then goes quiet.
    #[tokio::test]
    async fn a_full_thread_syncs_its_recent_window_and_stops() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        let fa = friends::upsert_by_endpoint(&a.dir, &f, "Chatty");
        chat::merge_synced(&a.dir, &fa.id, (0..2100u64).map(|i| text(&format!("x{i}"), &fa.id, i % 3 == 0, i + 1)).collect());
        assert_eq!(chat::messages(&a.dir, &fa.id).len(), 2000, "capped at 2000");
        let first = sync(&a, &b).await;
        assert_eq!(first.server.unwrap().0, 1000);
        let got = b.thread(&f);
        assert_eq!(got.len(), 1000);
        assert_eq!(got.last().unwrap().id, "x2099");
        let again = sync(&a, &b).await;
        assert_eq!((again.client.unwrap().0, again.server.unwrap().0), (0, 0));
    }

    /// GIF/file messages arrive without this device's paths but keep what's
    /// needed to show them (names, sizes, the GIF's link).
    #[tokio::test]
    async fn file_and_gif_messages_arrive_without_local_paths() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        let fa = friends::upsert_by_endpoint(&a.dir, &f, "Mong");
        let file: chat::ChatMessage = serde_json::from_value(json!({"id":"file","peerId":fa.id,"fromMe":false,"kind":"file",
            "text":"look","files":["photo.jpg"],"bytes":1234,"path":"/Users/me/Downloads/photo.jpg","ts":1,"seq":1,
            "fileXferId": uuid::Uuid::new_v4().to_string()})).unwrap();
        let gif: chat::ChatMessage = serde_json::from_value(json!({"id":"gif","peerId":fa.id,"fromMe":true,"kind":"file",
            "text":"","files":["giphy-x.gif"],"bytes":99,"path":"/tmp/giphy-x.gif","status":"delivered","ts":2,"seq":2,
            "gif":{"provider":"giphy","id":"x","url":"https://media.giphy.com/x.gif"}})).unwrap();
        chat::append(&a.dir, &file);
        chat::append(&a.dir, &gif);
        assert!(sync(&a, &b).await.client.is_ok());
        let got = b.thread(&f);
        let (f2, g2) = (got.iter().find(|m| m.id == "file").unwrap(), got.iter().find(|m| m.id == "gif").unwrap());
        assert!(f2.path.is_none() && g2.path.is_none(), "no other device's paths");
        assert_eq!((f2.files.clone(), f2.bytes, f2.text.as_str()), (vec!["photo.jpg".to_string()], 1234, "look"));
        assert_eq!(f2.file_xfer_id, file.file_xfer_id);
        assert_eq!(g2.gif.as_ref().unwrap().url, "https://media.giphy.com/x.gif");
    }

    /// A chat with one of your OWN devices stays between those two devices.
    #[tokio::test]
    async fn a_chat_with_your_own_device_stays_on_those_two() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("iPad", "tablet", Some(&k)).await);
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(sync(&c, &b).await.client.is_ok());
        // A message from the iPhone lands on the Mac in the iPhone's own thread.
        let phone = friends::chat_sender(&a.dir, &b.eid()).expect("own device can chat");
        assert_eq!(phone.account_pub.as_deref(), my_pub(&a.dir).as_deref());
        assert_eq!(friends::person_endpoints(&a.dir, &phone.id), [b.eid()], "never grouped with anyone");
        chat::append(&a.dir, &text("note-to-self", &phone.id, false, 1));
        assert!(sync(&a, &c).await.client.is_ok());
        assert!(chat::overview(&c.dir).is_empty(), "the iPad doesn't get the Mac↔iPhone thread");
    }

    /// A friend with several devices is one person with one thread; a device
    /// with a smaller id joining them moves the thread to it (nothing hidden);
    /// a device that leaves their account splits off cleanly.
    #[tokio::test]
    async fn a_friends_devices_are_one_person_until_one_leaves() {
        let a = Dev::new("Mac", "laptop", Some(&key())).await;
        let theirs = key();
        let account = hex::encode(theirs.public().as_bytes());
        let mut devs: Vec<String> = (0..3).map(|_| eid()).collect();
        devs.sort();
        let (low, mid, high) = (devs[0].clone(), devs[1].clone(), devs[2].clone());
        let hello = |e: &str| json!({"device_kind":"phone","device_os":"ios","account_pub":account,
            "account_sig":hex::encode(theirs.sign(e.as_bytes()).to_bytes())});
        let fm = friends::upsert_by_endpoint(&a.dir, &mid, "Mong");
        let fh = friends::upsert_by_endpoint(&a.dir, &high, "Mong's iPad");
        chat::append(&a.dir, &text("1", &fm.id, false, 10));
        chat::append(&a.dir, &text("2", &fh.id, false, 20));
        friends::apply_device_hello(&a.dir, &mid, &hello(&mid));
        friends::apply_device_hello(&a.dir, &high, &hello(&high));
        assert_eq!(friends::chat_sender(&a.dir, &high).unwrap().id, fm.id, "one person");
        assert_eq!(ids(&chat::messages(&a.dir, &fm.id)), ["1", "2"]);
        // Their new phone happens to sort first: it becomes the owner and the
        // whole conversation moves with it.
        let fl = friends::upsert_by_endpoint(&a.dir, &low, "Mong's iPhone");
        friends::apply_device_hello(&a.dir, &low, &hello(&low));
        assert_eq!(friends::thread_owner(&a.dir, &fm.id).unwrap().id, fl.id);
        assert_eq!(ids(&chat::messages(&a.dir, &fl.id)), ["1", "2"]);
        assert!(chat::messages(&a.dir, &fm.id).is_empty());
        assert_eq!(friends::person_endpoints(&a.dir, &fl.id), [low.clone(), mid.clone(), high.clone()]);
        // The iPad leaves their account: it's its own contact again.
        friends::apply_device_hello(&a.dir, &high, &json!({"device_kind":"tablet","device_os":"ios"}));
        assert_eq!(friends::chat_sender(&a.dir, &high).unwrap().id, fh.id);
        assert_eq!(friends::person_endpoints(&a.dir, &fl.id), [low.clone(), mid.clone()]);
        assert_eq!(ids(&chat::messages(&a.dir, &fl.id)), ["1", "2"], "their shared history stays with the person");
    }

    /// S4: a removed (or stolen) device still holds the account key, but it
    /// can't re-link itself, can't remove the others, and a brand-new endpoint
    /// made with the key alone gets nothing.
    #[tokio::test]
    async fn a_removed_device_or_a_key_holder_cannot_get_back_in_or_kick_others() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("Stolen iPad", "tablet", Some(&k)).await);
        let pal = eid();
        friends::upsert_by_endpoint(&a.dir, &pal, "Pal");
        assert!(sync(&c, &a).await.client.is_ok());
        assert!(sync(&b, &a).await.client.is_ok());
        let account = my_pub(&a.dir).unwrap();
        // The Mac removes the iPad; the iPad hasn't heard.
        std::thread::sleep(Duration::from_millis(5));
        with_book(&a.dir, &account, |bk| { bk.removed_devices.insert(c.eid(), chat::now_ms()); });
        // The thief forges: a far-future link for itself (unsigned and
        // self-signed) and a removal of the iPhone.
        let ck = c.ep.secret_key().clone();
        with_book(&c.dir, &account, |bk| {
            bk.linked.insert(c.eid(), u64::MAX / 2);
            bk.link_proofs.insert(c.eid(), vec![sign_stamp(&ck, &account, &c.eid(), u64::MAX / 2)]);
            bk.removed_devices.insert(b.eid(), u64::MAX / 2);
        });
        let out = sync_unvouched(&c, &a).await;
        assert!(out.server.is_none(), "refused before anything merged");
        with_book(&a.dir, &account, |bk| {
            assert!(bk.is_removed(&c.eid()), "still removed");
            assert!(bk.is_member(&b.eid()), "the iPhone wasn't kicked out");
        });
        assert_eq!(a.own_device_ids(), [b.eid()]);
        // A fresh endpoint holding the account key, never linked by anyone.
        let w = Dev::new("Fake", "laptop", Some(&k)).await;
        let out = sync_unvouched(&w, &a).await;
        assert!(out.server.is_none() && out.client.is_err(), "the key alone gets nothing");
        assert!(a.friend(&pal).is_some() && w.friend(&pal).is_none(), "no friends leaked");
        // …and its signed hello doesn't make it one of our devices either.
        let hello = json!({"device_kind": "laptop", "device_os": "macos", "account_pub": account,
            "account_sig": crate::link::sign_endpoint(&w.dir, &w.eid()).unwrap()});
        friends::apply_device_hello(&a.dir, &w.eid(), &hello);
        assert!(!is_own_device(&a.dir, &w.eid()));
        // A proof for the fake device signed by the removed iPad doesn't help.
        let forged = HashMap::from([(w.eid(), vec![sign_stamp(&ck, &account, &w.eid(), chat::now_ms())])]);
        with_book(&a.dir, &account, |bk| { assert!(!bk.merge_proofs(&account, &a.eid(), &forged)); });
        // But the iPhone (still in) vouching for a real new device works.
        let bk_key = b.ep.secret_key().clone();
        let real = HashMap::from([(w.eid(), vec![sign_stamp(&bk_key, &account, &w.eid(), chat::now_ms())])]);
        with_book(&a.dir, &account, |bk| { assert!(bk.merge_proofs(&account, &a.eid(), &real)); assert!(bk.is_member(&w.eid())); });
    }

    /// S4: removing a device also drops every device only IT vouched for
    /// (until a remaining device approves it again), and removals only count
    /// when signed by a device still in the account.
    #[tokio::test]
    async fn removal_is_transitive_and_must_be_signed() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("iPad", "tablet", Some(&k)).await);
        assert!(sync(&b, &a).await.client.is_ok());
        assert!(sync(&c, &a).await.client.is_ok());
        assert!(sync(&b, &a).await.client.is_ok());
        let account = my_pub(&a.dir).unwrap();
        // The iPad vouches for a device W (as if it linked it) — the Mac learns it.
        let w = eid();
        let ck = c.ep.secret_key().clone();
        let voucher = HashMap::from([(w.clone(), vec![sign_stamp(&ck, &account, &w, chat::now_ms())])]);
        with_book(&a.dir, &account, |bk| { assert!(bk.merge_proofs(&account, &a.eid(), &voucher)); assert!(bk.is_member(&w)); });
        // An unsigned removal, or one signed by a non-member, changes nothing.
        let stranger = iroh::SecretKey::generate();
        let fake = HashMap::from([(b.eid(), sign_removal(&stranger, &account, &b.eid(), chat::now_ms()))]);
        with_book(&a.dir, &account, |bk| { assert!(!bk.merge_removals(&account, &a.eid(), &fake)); assert!(bk.is_member(&b.eid())); });
        // The Mac removes the iPad: W, vouched for only by the iPad, is out too…
        remove_device(&a.dir, &c.eid(), Some(a.ep.secret_key())).unwrap();
        with_book(&a.dir, &account, |bk| { assert!(!bk.is_member(&c.eid())); assert!(!bk.is_member(&w), "transitively revoked"); });
        // …and ensure_proofs doesn't quietly vouch for it again.
        ensure_proofs(&a.dir, &account, a.ep.secret_key());
        with_book(&a.dir, &account, |bk| assert!(!bk.is_member(&w)));
        // The iPhone learns the signed removal; the removed iPad's own removal of
        // the iPhone (signed after it was removed) is refused there.
        assert!(sync(&b, &a).await.client.is_ok());
        let late = HashMap::from([(a.eid(), sign_removal(&ck, &account, &a.eid(), chat::now_ms()))]);
        with_book(&b.dir, &account, |bk| {
            assert!(bk.is_removed(&c.eid()));
            assert!(!bk.merge_removals(&account, &b.eid(), &late));
            assert!(bk.is_member(&a.eid()));
        });
        // A device that proves the key but isn't vouched for waits for approval.
        let hello = json!({"device_kind": "tablet", "device_os": "ios", "device_name": "Old iPad", "account_pub": account,
            "account_sig": crate::link::sign_endpoint(&a.dir, &w).unwrap()});
        friends::apply_device_hello(&b.dir, &w, &hello);
        let views = device_views(&b.st, &b.eid());
        assert!(views.iter().any(|v| v.endpoint_id == w && v.needs_approval));
        approve_device(&b.dir, &w, b.ep.secret_key()).unwrap();
        assert!(is_own_device(&b.dir, &w), "approved");
        assert!(!device_views(&b.st, &b.eid()).iter().any(|v| v.needs_approval));
    }

    /// S4 toward friends: a friend's removed device stops being that friend.
    #[tokio::test]
    async fn friends_drop_a_device_its_owner_removed() {
        let k = key();
        let account = hex::encode(k.public().as_bytes());
        let (mac, phone) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("Stolen", "phone", Some(&k)).await);
        let friend = Dev::new("Friend", "laptop", None).await;
        let sig = |d: &Dev| crate::link::sign_endpoint(&d.dir, &d.eid()).unwrap();
        let f_mac = friends::upsert_by_endpoint(&friend.dir, &mac.eid(), "Mong");
        friends::apply_device_hello(&friend.dir, &mac.eid(), &json!({"account_pub": account, "account_sig": sig(&mac)}));
        // The phone joined the person as a proven device.
        let hello = json!({"account_pub": account, "account_sig": sig(&phone)});
        assert_eq!(friends::apply_hello_from(&friend.dir, "", &phone.eid(), "Mong", &hello), friends::HelloOutcome::AddedDevice);
        friends::apply_device_hello(&friend.dir, &phone.eid(), &hello);
        let p_rec = friend.friend(&phone.eid()).unwrap();
        chat::append(&friend.dir, &text("p1", &p_rec.id, false, 5));
        // The Mac removes the phone and says so in its hello.
        let at = chat::now_ms();
        let rm = sign_removal(mac.ep.secret_key(), &account, &phone.eid(), at);
        let mac_hello = json!({"account_pub": account, "account_sig": sig(&mac),
            "revoked": [{"eid": phone.eid(), "at": at, "by": rm.by, "sig": rm.sig}]});
        // A forged revocation (signed by a stranger) is ignored first.
        let stranger = iroh::SecretKey::generate();
        let bad = sign_removal(&stranger, &account, &phone.eid(), at);
        assert!(!friends::apply_revocations(&friend.dir, &mac.eid(), &json!({"account_pub": account, "account_sig": sig(&mac),
            "revoked": [{"eid": phone.eid(), "at": at, "by": bad.by, "sig": bad.sig}]})));
        assert!(friends::apply_revocations(&friend.dir, &mac.eid(), &mac_hello));
        assert!(friend.friend(&phone.eid()).is_none(), "no longer poses as Mong");
        assert!(friend.thread(&mac.eid()).iter().any(|m| m.id == "p1"), "the conversation stays with the person");
        // Its next hello (still signed with the account key) is just a request.
        assert_eq!(friends::apply_hello_from(&friend.dir, "", &phone.eid(), "Mong", &hello), friends::HelloOutcome::Requested);
        friends::apply_device_hello(&friend.dir, &phone.eid(), &hello);
        assert!(friends::chat_sender(&friend.dir, &phone.eid()).is_none());
        // And it can't revoke the Mac in turn.
        let back = sign_removal(phone.ep.secret_key(), &account, &mac.eid(), chat::now_ms());
        assert!(!friends::apply_revocations(&friend.dir, &phone.eid(), &json!({"account_pub": account, "account_sig": sig(&phone),
            "revoked": [{"eid": mac.eid(), "at": back.at, "by": back.by, "sig": back.sig}]})));
        assert_eq!(friends::chat_sender(&friend.dir, &mac.eid()).unwrap().id, f_mac.id);
    }

    /// Removing a device: every device drops it, it learns it left, and it
    /// can be linked again later (the relink beats the old removal).
    #[tokio::test]
    async fn remove_a_device_then_link_it_again() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("iPad", "tablet", Some(&k)).await);
        assert!(sync(&c, &a).await.client.is_ok());
        assert!(sync(&b, &a).await.client.is_ok());
        assert_eq!(b.own_device_ids().len(), 2);
        // The Mac removes the iPad.
        std::thread::sleep(Duration::from_millis(5));
        let account = my_pub(&a.dir).unwrap();
        remove_device(&a.dir, &c.eid(), Some(a.ep.secret_key())).unwrap();
        assert!(a.friend(&c.eid()).is_none());
        assert!(sync(&b, &a).await.client.is_ok());
        assert_eq!(b.own_device_ids(), [a.eid()], "the iPhone drops it too");
        // The iPad reaches the iPhone: it's told, and leaves (keeping its data).
        let out = sync(&c, &b).await;
        assert!(out.client.unwrap().2, "the iPad learned it was removed");
        assert!(my_pub(&c.dir).is_none());
        // A stray hello from it doesn't make it a contact again.
        friends::apply_hello(&b.dir, "", &c.eid(), "iPad");
        assert!(b.friend(&c.eid()).is_none());
        // Linked again (through the Mac): the relink wins everywhere.
        crate::link::adopt_key_for_tests(&c.dir, &k);
        mark_linked(&c.dir, &c.eid());
        mark_linked(&a.dir, &c.eid());
        assert!(sync(&c, &a).await.client.is_ok());
        assert!(sync(&c, &b).await.client.is_ok(), "the iPhone accepts it: it learns the relink first");
        let mut want = vec![a.eid(), c.eid()]; want.sort();
        assert_eq!(b.own_device_ids(), want);
    }

    /// Leaving while every other device is offline: the device announces it in
    /// its next hello, and its former devices drop it from then on.
    #[tokio::test]
    async fn leaving_while_others_are_offline_still_reaches_them() {
        let k = key();
        let (a, b, c) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await,
            Dev::new("iPad", "tablet", Some(&k)).await);
        let f = eid();
        friends::upsert_by_endpoint(&c.dir, &f, "Mong");
        assert!(sync(&c, &a).await.client.is_ok());
        assert!(sync(&b, &a).await.client.is_ok());
        // The iPad leaves with nobody reachable.
        let account = my_pub(&c.dir).unwrap();
        let ck = c.ep.secret_key().clone();
        with_book(&c.dir, &account, |bk| {
            let at = chat::now_ms();
            bk.removed_devices.insert(c.eid(), at);
            bk.removal_proofs.insert(c.eid(), sign_removal(&ck, &account, &c.eid(), at));
        });
        leave_account(&c.dir, &account, &c.eid());
        assert!(my_pub(&c.dir).is_none());
        assert!(c.friend(&f).is_some(), "it keeps its friends");
        assert!(c.friend(&a.eid()).is_none(), "former devices don't linger as auto-accepting contacts");
        // Its next hello to the iPhone carries the notice.
        let notice = left_notice(&c.dir);
        assert!(apply_left_notice(&b.dir, &c.eid(), &notice));
        assert_eq!(b.own_device_ids(), [a.eid()]);
        friends::apply_hello(&b.dir, "", &c.eid(), "iPad");
        assert!(b.friend(&c.eid()).is_none(), "not re-added as a contact");
        // …and the Mac hears it from the iPhone.
        assert!(sync(&b, &a).await.client.is_ok());
        assert_eq!(a.own_device_ids(), [b.eid()]);
        // Someone who was never one of our devices can't "leave" (and so can't
        // hide themselves from our other devices).
        let stranger = eid();
        friends::upsert_by_endpoint(&a.dir, &stranger, "Stranger");
        assert!(!apply_left_notice(&a.dir, &stranger, &json!({account.clone(): {"at": chat::now_ms()}})));
        assert!(!device_was_removed(&a.dir, &stranger));
        // A notice about an older link doesn't undo a newer one.
        mark_linked(&a.dir, &c.eid());
        assert!(!apply_left_notice(&a.dir, &c.eid(), &json!({account.clone(): {"at": 1, "since": 1}})));
    }

    /// The person's name and picture are account-wide: a change on one device
    /// reaches the others (friends then see it from every device), while each
    /// device keeps its own device name. A default computer name never spreads.
    #[tokio::test]
    async fn the_profile_name_and_picture_follow_the_user() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        a.st.settings.lock().unwrap().display_name = device_name("laptop");
        b.st.settings.lock().unwrap().display_name = "Ashton".into();
        let out = sync(&a, &b).await;
        assert!(out.client.unwrap().1, "the Mac's default name gave way to the chosen one");
        assert_eq!(a.st.settings.lock().unwrap().display_name, "Ashton");
        assert_eq!(crate::settings::load(&a.dir, "", "").display_name, "Ashton", "persisted");
        // The device list keeps device names.
        assert_eq!(b.friend(&a.eid()).unwrap().name, "Mac");
        assert_eq!(a.friend(&b.eid()).unwrap().name, "iPhone");
        // A rename on the Mac later wins on the iPhone.
        std::thread::sleep(Duration::from_millis(5));
        a.st.settings.lock().unwrap().display_name = "Ashton M".into();
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30]));
        let pic = a.dir.join("avatar-1.png");
        img.save(&pic).unwrap();
        a.st.settings.lock().unwrap().avatar = pic.to_string_lossy().into_owned();
        let out = sync(&b, &a).await;
        assert!(out.client.unwrap().1);
        let s = b.st.settings.lock().unwrap().clone();
        assert_eq!(s.display_name, "Ashton M");
        assert_eq!(std::fs::read(&s.avatar).unwrap(), std::fs::read(&pic).unwrap(), "the picture came over");
        // Removing the picture on the iPhone removes it on the Mac.
        std::thread::sleep(Duration::from_millis(5));
        b.st.settings.lock().unwrap().avatar.clear();
        assert!(sync(&b, &a).await.server.unwrap().1);
        assert!(a.st.settings.lock().unwrap().avatar.is_empty());
        // Nothing changed: quiet.
        let again = sync(&a, &b).await;
        assert!(!again.client.unwrap().1 && !again.server.unwrap().1);
    }

    /// A message to a person goes to whichever of their devices knows us: a
    /// device that answers "don't know you" (applied:false) isn't delivery.
    #[tokio::test]
    async fn chat_is_delivered_to_the_first_device_that_accepts_it() {
        use iroh::endpoint::presets;
        let answer = |applied: Option<bool>| async move {
            let ep = iroh::Endpoint::builder(presets::N0).alpns(vec![iroh_net::ALPN.to_vec()]).bind().await.unwrap();
            let srv = ep.clone();
            let task = tokio::spawn(async move {
                let conn = srv.accept().await.unwrap().await.unwrap();
                let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                let req = iroh_net::read_frame(&mut recv).await.unwrap();
                let mut ack = json!({"kind": "ok"});
                if let Some(a) = applied { ack["applied"] = json!(a); }
                iroh_net::write_frame(&mut send, &ack).await.unwrap();
                send.finish().unwrap();
                let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
                req["id"].as_str().map(str::to_owned)
            });
            (ep, task)
        };
        let (stranger, t1) = answer(Some(false)).await;
        let (friend, t2) = answer(None).await; // an older build: no "applied" field
        let me = iroh::Endpoint::bind(presets::N0).await.unwrap();
        let state = IrohState::default();
        let eids = [stranger.id().to_string(), friend.id().to_string()];
        let payload = json!({"kind": "chat", "v": 2, "id": "hello", "text": "hi"});
        iroh_net::remember_addrs_for_tests(&stranger.addr());
        iroh_net::remember_addrs_for_tests(&friend.addr());
        let got = iroh_net::send_chat_any(&state, &me, &eids, payload).await;
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(t1.await.unwrap().as_deref(), Some("hello"));
        assert_eq!(t2.await.unwrap().as_deref(), Some("hello"));
    }

    /// Clock skew: a far-future removal can't pin a friend forever, and a
    /// re-add on a device whose clock runs behind still beats the removal.
    #[tokio::test]
    async fn clock_skew_does_not_undo_a_re_add() {
        let k = key();
        let (a, b) = (Dev::new("Mac", "laptop", Some(&k)).await, Dev::new("iPhone", "phone", Some(&k)).await);
        let f = eid();
        friends::upsert_by_endpoint(&a.dir, &f, "Mong");
        assert!(sync(&a, &b).await.client.is_ok());
        // B's clock runs an hour fast when it removes them…
        let account = my_pub(&b.dir).unwrap();
        let fb = b.friend(&f).unwrap();
        friends::remove(&b.dir, &fb.id).unwrap();
        with_book(&b.dir, &account, |bk| { bk.removed_friends.insert(f.clone(), chat::now_ms() + 3_600_000); });
        assert!(sync(&b, &a).await.client.is_ok());
        assert!(a.friend(&f).is_none());
        // …and A (correct clock) re-adds them right away: the re-add sticks.
        friends::add_by_code(&a.dir, &friends::my_code("Mong", &f)).unwrap();
        assert!(sync(&a, &b).await.client.is_ok());
        assert!(a.friend(&f).is_some() && b.friend(&f).is_some());
    }

    #[test]
    fn device_like_names_are_case_insensitive_and_pre_link_names_never_overwrite() {
        assert!(is_default_name("Ashton’s Iphone", "phone"));
        assert!(is_default_name("ashtons macbook pro", "laptop"));
        assert!(is_default_name("Ashton laptop", "laptop"));
        assert!(!is_default_name("Ashton", "laptop"));
        assert!(!is_default_name("Mong", "phone"));
    }
}
