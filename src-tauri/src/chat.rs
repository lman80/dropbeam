//! Peer-to-peer chat with friends, riding the same iroh endpoint as transfers.
//!
//! Messages travel over the shared `dropbeam/1` ALPN as `{kind:"chat", ...}`
//! frames (dial-by-EndpointId, exactly like the folder control beacon). Each
//! conversation is persisted per friend in `chats.json` so it survives restarts.
//!
//! A message to an offline friend is stored locally and retried by the outbox;
//! when a Transfer Server is available it is sealed and held there instead
//! (status "held", see `mailbox`) and delivered when the friend comes back.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::write_atomic;

/// In-memory cache of every conversation store, keyed by config dir — the
/// real app has exactly one entry; tests (which pass throwaway temp dirs) each
/// get their own, so they can't poison each other or the app. chats.json is
/// parsed ONCE per dir on first touch; every mutation updates memory and
/// persists write-through, so the per-op re-parse of the (multi-MB) file is
/// gone. The mutex is also the serialization lock the old `LOCK` provided —
/// and now covers the read paths (`messages`/`outbox`/`overview`) too, which
/// previously re-parsed the file unlocked.
static CACHE: Mutex<Option<HashMap<PathBuf, HashMap<String, Vec<ChatMessage>>>>> =
    Mutex::new(None);

/// Keep each conversation bounded so chats.json can't grow without limit.
const MAX_PER_PEER: usize = 2000;

// S8: bounds on every field a peer controls in an incoming chat frame (the
// frame itself may be up to 1 MiB, and chats.json is rewritten per message).
pub const MAX_TEXT_CHARS: usize = 4000;
pub const MAX_PREVIEW_CHARS: usize = 160;
pub const MAX_EMOJI_CHARS: usize = 16;
pub const MAX_ID_LEN: usize = 128;
pub const MAX_FILE_NAMES: usize = 1000;
const MAX_FILE_NAME_CHARS: usize = 255;

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
/// A peer's Lamport `seq`, bounded: at most a million ahead of ours, so a
/// forged u64::MAX can't pin (or overflow) the thread's ordering clock.
pub fn cap_seq(theirs: u64, next_local: u64) -> u64 {
    theirs.min(next_local.saturating_add(1_000_000)).max(next_local)
}
/// A peer's wall-clock `ts`, bounded to at most a day in the future (it orders
/// the thread and read receipts compare against it).
pub fn cap_ts(theirs: u64) -> u64 {
    theirs.min(now_ms().saturating_add(24 * 3600 * 1000))
}

/// A message's text, bounded.
pub fn cap_text(s: &str) -> String {
    take_chars(s, MAX_TEXT_CHARS)
}
/// A reply's one-line quote, bounded.
pub fn cap_preview(s: &str) -> String {
    take_chars(s, MAX_PREVIEW_CHARS)
}
/// A shared file's display name, bounded.
pub fn cap_file_name(s: &str) -> String {
    take_chars(s, MAX_FILE_NAME_CHARS)
}
/// A GIF attachment whose fields are a sane size (else it's dropped).
pub fn gif_within_limits(g: &GifMeta) -> bool {
    g.provider.len() <= 32 && g.id.len() <= MAX_ID_LEN && g.url.len() <= 2048 && g.page.len() <= 2048
        && g.w <= 10_000 && g.h <= 10_000
}

/// A GIF attached to a chat message (Giphy). Optional metadata that rides the
/// wire frame so an updated peer can render a dedicated GIF bubble; older peers
/// ignore it and just see the `.gif` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GifMeta {
    pub provider: String,
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
}

/// One reaction on a message. For a 1:1 chat there are only two reactors, so we
/// key the set by `(from_me, emoji)` — applying the same reaction twice is a
/// no-op (idempotent, survives store-and-forward re-delivery).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reaction {
    pub emoji: String,
    pub from_me: bool,
}

/// One message in a conversation with a friend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// Stable file-transfer link; optional for old history and peers.
    #[serde(default)]
    pub file_xfer_id: Option<String>,
    pub id: String,
    /// The friend id this conversation belongs to.
    pub peer_id: String,
    /// True if we sent it, false if the friend did.
    pub from_me: bool,
    /// "text" or "file".
    pub kind: String,
    pub text: String,
    /// For file messages: the names of the files shared.
    #[serde(default)]
    pub files: Vec<String>,
    /// For file messages: total bytes (for display).
    #[serde(default)]
    pub bytes: u64,
    /// For file messages: the local path to the (first) file ON THIS device — the
    /// sender's source path, or the receiver's saved path. Lets the UI show a
    /// preview and open it. Device-local, so it never travels in the wire frame.
    #[serde(default)]
    pub path: Option<String>,
    /// Delivery state for messages WE sent: "sending" (queued/in-flight), "delivered"
    /// (the peer's app received + stored it), "read" (they've viewed it), or "failed"
    /// (couldn't reach them — the outbox keeps retrying). None for received messages.
    /// Device-local; never sent on the wire. ("sent" is tolerated from older builds.)
    #[serde(default)]
    pub status: Option<String>,
    pub ts: u64,
    /// Logical ordering clock (Lamport-style): each message takes `max(seq in
    /// thread) + 1` at creation, on BOTH send and receive. Sorting by `seq` (then
    /// ts, then id) keeps the thread in causal order even when the two devices'
    /// wall-clocks disagree. Old messages default to 0 and sort by ts among
    /// themselves, before any new (seq >= 1) message — so history order is kept.
    #[serde(default)]
    pub seq: u64,
    /// Reply/quote: the id of the message this one replies to (if any), plus a
    /// cached one-line preview of it so the quote renders even if the original
    /// isn't on this device yet.
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub reply_preview: Option<String>,
    /// Emoji reactions on this message (a set keyed by (from_me, emoji)).
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// True once the author edited the text (shows an "Edited" marker).
    #[serde(default)]
    pub edited: bool,
    /// True once the author unsent it (renders a "deleted" tombstone, text cleared).
    #[serde(default)]
    pub deleted: bool,
    /// A GIF attachment (Giphy) — when present, the UI renders a GIF bubble.
    #[serde(default)]
    pub gif: Option<GifMeta>,
    /// Mutation clock (ms) bumped on every local reaction/edit/unsend, so a
    /// user's own devices can tell which copy of a message is newer when they
    /// sync (last writer wins). 0 = never changed since it was stored.
    #[serde(default)]
    pub rev: u64,
    /// Sender side: the Transfer Server now holding this message for an offline
    /// friend (its display name), while status is "held". Device-local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_on: Option<String>,
    /// Sender side: why a server couldn't hold/deliver it ("expired", "full",
    /// "unreachable", "needs_update", …) — drives the bubble's short note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_note: Option<String>,
    /// Receiver side: the Transfer Server it arrived through ("via Linux Box").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Sender side, files sent to a friend with several devices: how far they
    /// got on each device ("Delivered to Alex's Mac · iPhone: waiting").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliveries: Vec<crate::fanout::Delivery>,
    /// A link preview (#47) the SENDER's device fetched: title, description,
    /// site and a small thumbnail travel with the message, so the receiver never
    /// contacts the site. Older builds ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_preview: Option<crate::link_preview::LinkPreview>,
    /// Own-device sync clock for the TEXT alone (edit/unsend), so a reaction
    /// made on one device and an edit made on another both survive the merge
    /// instead of the newer whole record overwriting the other (D17).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub text_rev: u64,
    /// Own-device sync clocks per reaction — including removed ones — so
    /// concurrent reactions on two devices merge per reaction (D17).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaction_revs: Vec<ReactionRev>,
}

/// When a reaction was last added or removed on this user's devices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReactionRev {
    pub emoji: String,
    pub from_me: bool,
    pub at: u64,
    /// false = removed (a tombstone, so an older copy can't bring it back).
    pub on: bool,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// Stable causal order: logical seq first, then wall-clock, then id as a final
/// tiebreak so two messages in the same millisecond never swap on re-sort.
fn order_key(m: &ChatMessage) -> (u64, u64, String) {
    (m.seq, m.ts, m.id.clone())
}

fn chats_path(config_dir: &Path) -> PathBuf {
    config_dir.join("chats.json")
}

/// Disk read — used only by `store_mut` to fill the cache on a dir's first touch.
fn load_all(config_dir: &Path) -> HashMap<String, Vec<ChatMessage>> {
    // A locked/torn chats.json must not read as "no chats": the next save would
    // overwrite every conversation. read_json_store retries and keeps a copy.
    match crate::settings::read_json_store(&chats_path(config_dir)) {
        crate::settings::StoreRead::Loaded(all) => all,
        _ => HashMap::new(),
    }
}

/// The cached store for `config_dir`, loading it from chats.json on first use.
/// Callers hold the CACHE lock (the guard derefs to the Option), so every read
/// and mutation of a store is serialized.
fn store_mut<'a>(
    cache: &'a mut Option<HashMap<PathBuf, HashMap<String, Vec<ChatMessage>>>>,
    config_dir: &Path,
) -> &'a mut HashMap<String, Vec<ChatMessage>> {
    cache
        .get_or_insert_with(HashMap::new)
        .entry(config_dir.to_path_buf())
        .or_insert_with(|| load_all(config_dir))
}

fn save_all(config_dir: &Path, all: &HashMap<String, Vec<ChatMessage>>) {
    if let Err(e) = try_save_all(config_dir, all) {
        log::error!("chat::save_all failed to persist chats.json: {e}");
    }
}

/// Persist the store, reporting failure (D11: an incoming message is only
/// acknowledged once it is on disk — a Transfer Server deletes its copy on ack).
fn try_save_all(config_dir: &Path, all: &HashMap<String, Vec<ChatMessage>>) -> Result<(), String> {
    crate::account::note_change();
    let _ = fs::create_dir_all(config_dir);
    // Compact JSON, not pretty: chats.json is machine-read only and can reach MBs
    // (2000 msgs/peer); pretty-printing roughly doubles the serialize+write cost on
    // a file rewritten on every message/status/reaction.
    match serde_json::to_string(all) {
        Ok(txt) => {
            // Don't swallow a failed write: a message can be emitted to the UI and
            // acked over the wire yet silently lost from chats.json (Windows handle
            // contention), so the thread looks complete in-session but is missing
            // after a restart. Log it so the diagnostics digest catches the loss.
            write_atomic(&chats_path(config_dir), txt.as_bytes()).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("cannot serialize chats: {e}")),
    }
}

/// Every message in the conversation with `peer_id`, oldest first.
pub fn messages(config_dir: &Path, peer_id: &str) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir)
        .get(peer_id)
        .cloned()
        .unwrap_or_default()
}

/// Append a message (dedup by id), bound the history, and persist. Returns
/// `false` if it was a duplicate we'd already stored (so callers can skip the
/// live event and avoid double-rendering).
pub fn append(config_dir: &Path, msg: &ChatMessage) -> bool {
    match append_inner(config_dir, msg, false) {
        Ok(new) => new,
        Err(e) => {
            log::error!("chat::append failed to persist chats.json: {e}");
            true
        }
    }
}

/// `append` for a message someone else sent us: Err (and the message is NOT
/// kept) when it couldn't be written to disk, so the caller doesn't
/// acknowledge it — the sender (or the Transfer Server holding it) keeps its
/// copy and tries again instead of deleting the only one (D11).
pub fn append_durable(config_dir: &Path, msg: &ChatMessage) -> Result<bool, String> {
    append_inner(config_dir, msg, true)
}

fn append_inner(config_dir: &Path, msg: &ChatMessage, durable: bool) -> Result<bool, String> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.entry(msg.peer_id.clone()).or_default();
    // Dedup scoped by direction: an incoming (peer-chosen) id can never collide
    // with one of OUR outgoing ids and silently suppress a real message.
    if thread.iter().any(|m| m.id == msg.id && m.from_me == msg.from_me) {
        return Ok(false);
    }
    // A linked manifest is immutable, including across differently named notes.
    if let Some(link) = &msg.file_xfer_id {
        if thread.iter().any(|m| m.from_me == msg.from_me && m.file_xfer_id.as_ref() == Some(link)) {
            return Ok(false);
        }
    }
    thread.push(msg.clone());
    thread.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    let mut trimmed = Vec::new();
    if thread.len() > MAX_PER_PEER {
        let drop = thread.len() - MAX_PER_PEER;
        trimmed = thread.drain(0..drop).collect();
    }
    if let Err(e) = try_save_all(config_dir, all) {
        if durable {
            // Undo in memory too, so memory never claims what disk lacks.
            let thread = all.entry(msg.peer_id.clone()).or_default();
            thread.retain(|m| !(m.id == msg.id && m.from_me == msg.from_me));
            thread.splice(0..0, trimmed);
        }
        return Err(e);
    }
    Ok(true)
}

/// Drop a whole conversation (e.g. when a friend is removed).
pub fn clear(config_dir: &Path, peer_id: &str) {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    if all.remove(peer_id).is_some() {
        save_all(config_dir, all);
    }
}

/// Fold the conversation under `from_id` INTO `into_id` (used when two friend
/// records turn out to be the same person and we collapse them — see
/// `friends::reconcile`). Non-destructive: messages are UNIONED (dedup by id),
/// re-keyed to the surviving peer, re-sorted, and bounded. The old thread is
/// removed. Safe to call when `from_id` has no history (a no-op). Returns how
/// many messages moved.
pub fn merge_threads(config_dir: &Path, from_id: &str, into_id: &str) -> usize {
    if from_id == into_id {
        return 0;
    }
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let Some(mut moving) = all.remove(from_id) else {
        return 0;
    };
    if moving.is_empty() {
        save_all(config_dir, all);
        return 0;
    }
    let dest = all.entry(into_id.to_string()).or_default();
    let mut moved = 0;
    for mut m in moving.drain(..) {
        if dest.iter().any(|x| x.id == m.id) {
            continue;
        }
        m.peer_id = into_id.to_string();
        dest.push(m);
        moved += 1;
    }
    dest.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    if dest.len() > MAX_PER_PEER {
        let drop = dest.len() - MAX_PER_PEER;
        dest.drain(0..drop);
    }
    save_all(config_dir, all);
    moved
}

/// Update a sent message's delivery status (sending → sent/failed). Returns the
/// updated message so the caller can re-emit it to the UI.
pub fn set_status(config_dir: &Path, peer_id: &str, msg_id: &str, status: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.get_mut(peer_id)?;
    let msg = thread.iter_mut().find(|m| m.id == msg_id)?;
    // Already in this state → no rewrite, no UI re-emit. The outbox retry loop calls
    // this every failed round for an offline peer; without the check it rewrote the
    // entire (multi-MB) chats.json every ~12s all night.
    if msg.status.as_deref() == Some(status) {
        return None;
    }
    // Never move backwards: a late "failed" from a racing direct attempt must not
    // undo "delivered"/"read", and must not un-hold a message a Transfer Server
    // already has (it's no longer ours to retry).
    let allowed = match msg.status.as_deref() {
        Some("read") => false,
        Some("delivered") => status == "read",
        Some("held") => matches!(status, "delivered" | "read"),
        _ => true,
    };
    if !allowed {
        return None;
    }
    if matches!(status, "delivered" | "read") {
        msg.server_note = None;
    }
    msg.status = Some(status.to_string());
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
}

/// A Transfer Server now holds this undelivered message: "held" (only from
/// sending/failed — never over a real delivery).
pub fn set_held(config_dir: &Path, peer_id: &str, msg_id: &str, server_name: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut().find(|m| m.id == msg_id && m.from_me)?;
    if !matches!(msg.status.as_deref(), Some("sending") | Some("failed")) {
        return None;
    }
    msg.status = Some("held".into());
    msg.held_on = Some(server_name.to_owned());
    msg.server_note = None;
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// A held message didn't make it through its server (expired / refused /
/// lost): back to "failed" with a short reason, so the bubble can say so and
/// the outbox keeps trying directly.
pub fn set_server_failed(config_dir: &Path, peer_id: &str, msg_id: &str, note: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut().find(|m| m.id == msg_id && m.from_me)?;
    if !matches!(msg.status.as_deref(), Some("held") | Some("sending") | Some("failed")) {
        return None;
    }
    if msg.status.as_deref() == Some("failed") && msg.server_note.as_deref() == Some(note) {
        return None;
    }
    msg.status = Some("failed".into());
    msg.server_note = Some(note.to_owned());
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// Attach the link preview our device fetched to a message we sent (#47).
pub fn set_link_preview(config_dir: &Path, peer_id: &str, msg_id: &str, preview: crate::link_preview::LinkPreview) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut().find(|m| m.id == msg_id && m.from_me && !m.deleted)?;
    msg.link_preview = Some(preview);
    // Own devices merge by rev: make the copy WITH the preview the newer one.
    msg.rev = bump_rev(msg.rev);
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// One stored message, by id.
pub fn message(config_dir: &Path, peer_id: &str, msg_id: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir).get(peer_id)?.iter().find(|m| m.id == msg_id).cloned()
}

/// Record why a server couldn't take a still-undelivered message (no status change).
pub fn set_server_note(config_dir: &Path, peer_id: &str, msg_id: &str, note: Option<&str>) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut().find(|m| m.id == msg_id && m.from_me)?;
    if msg.server_note.as_deref() == note || matches!(msg.status.as_deref(), Some("delivered") | Some("read")) {
        return None;
    }
    msg.server_note = note.map(String::from);
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// Our messages a Transfer Server is holding (candidates for a cheap direct
/// resend when the friend shows up first), oldest first.
pub fn held(config_dir: &Path) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let mut out: Vec<ChatMessage> = store_mut(&mut cache, config_dir)
        .values()
        .flatten()
        .filter(|m| m.from_me && !m.deleted && m.status.as_deref() == Some("held"))
        .cloned()
        .collect();
    out.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    out
}

/// Set the landed path of a received file card (server delivery), if unset.
pub fn set_received_path(config_dir: &Path, peer_id: &str, file_xfer_id: &str, path: &str, via: Option<&str>) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut().find(|m| !m.from_me && m.file_xfer_id.as_deref() == Some(file_xfer_id))?;
    let mut changed = false;
    if msg.path.is_none() {
        msg.path = Some(path.to_owned());
        changed = true;
    }
    if msg.via.is_none() && via.is_some() {
        msg.via = via.map(String::from);
        changed = true;
    }
    if !changed {
        return None;
    }
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// A linked file send finished landing: give its received card (if it has no
/// path yet — e.g. its note came through a Transfer Server) the landed path.
pub fn set_path_by_link(config_dir: &Path, file_xfer_id: &str, path: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.values_mut().flatten()
        .find(|m| !m.from_me && m.path.is_none() && m.file_xfer_id.as_deref() == Some(file_xfer_id))?;
    msg.path = Some(path.to_owned());
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// Record where a multi-device file send got to on each device, on OUR card
/// for it (`file_xfer_id` = the send's chat link id). None when unchanged or
/// the card isn't there (yet).
pub fn set_deliveries(config_dir: &Path, peer_id: &str, file_xfer_id: &str, deliveries: &[crate::fanout::Delivery]) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let msg = all.get_mut(peer_id)?.iter_mut()
        .find(|m| m.from_me && m.file_xfer_id.as_deref() == Some(file_xfer_id))?;
    if msg.deliveries == deliveries {
        return None;
    }
    msg.deliveries = deliveries.to_vec();
    let out = msg.clone();
    save_all(config_dir, all);
    Some(out)
}

/// A received file card for this transfer link, if we have one.
pub fn received_file(config_dir: &Path, peer_id: &str, file_xfer_id: &str) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir).get(peer_id)?
        .iter().find(|m| !m.from_me && m.file_xfer_id.as_deref() == Some(file_xfer_id)).cloned()
}

/// The next logical sequence number for a conversation: one past the highest
/// `seq` we've stored for it (counting BOTH directions). Used to stamp a new
/// message so the thread stays causally ordered across clock skew. A Lamport
/// clock: since received messages are stored with the sender's seq, taking
/// max+1 here advances our clock past anything we've seen.
pub fn next_seq(config_dir: &Path, peer_id: &str) -> u64 {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir)
        .get(peer_id)
        .map(|t| t.iter().map(|m| m.seq).max().unwrap_or(0).saturating_add(1))
        .unwrap_or(1)
}

/// Add or remove a reaction on a stored message. The reaction set is keyed by
/// `(from_me, emoji)`, so adding the same one twice is idempotent (safe against
/// re-delivery) and removing toggles it off. Returns the updated message.
pub fn apply_reaction(
    config_dir: &Path,
    peer_id: &str,
    target_id: &str,
    emoji: &str,
    from_me: bool,
    add: bool,
) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.get_mut(peer_id)?;
    let msg = thread.iter_mut().find(|m| m.id == target_id)?;
    let existing = msg
        .reactions
        .iter()
        .position(|r| r.from_me == from_me && r.emoji == emoji);
    let changed = match (add, existing) {
        (true, None) => { msg.reactions.push(Reaction { emoji: emoji.to_string(), from_me }); true }
        (false, Some(i)) => {
            msg.reactions.remove(i);
            true
        }
        _ => false, // already in the desired state — idempotent no-op
    };
    if changed {
        msg.rev = bump_rev(msg.rev);
        let at = msg.rev;
        note_reaction_rev(msg, emoji, from_me, at, add);
    }
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
}

fn note_reaction_rev(msg: &mut ChatMessage, emoji: &str, from_me: bool, at: u64, on: bool) {
    match msg.reaction_revs.iter_mut().find(|r| r.from_me == from_me && r.emoji == emoji) {
        Some(r) => { r.at = r.at.max(at); r.on = on; }
        None => msg.reaction_revs.push(ReactionRev { emoji: emoji.to_owned(), from_me, at, on }),
    }
}

/// Edit a stored message's text (the author changed it). Marks it `edited`.
/// `author_is_me` must match the message's `from_me`: a LOCAL edit (true) only
/// touches our own message; a REMOTE edit (false) only touches the peer's. This
/// stops a peer from rewriting a message WE authored. Returns the updated message.
pub fn apply_edit(
    config_dir: &Path,
    peer_id: &str,
    target_id: &str,
    new_text: &str,
    author_is_me: bool,
) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.get_mut(peer_id)?;
    let msg = thread
        .iter_mut()
        .find(|m| m.id == target_id && !m.deleted && m.from_me == author_is_me)?;
    msg.text = new_text.to_string();
    // An edit that removes the link drops the preview of it.
    if msg.link_preview.is_some() && crate::link_preview::first_url(&msg.text).is_none() {
        msg.link_preview = None;
    }
    msg.edited = true;
    msg.rev = bump_rev(msg.rev);
    msg.text_rev = msg.rev;
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
}

/// Unsend a stored message: clear its content and tombstone it. Idempotent.
/// `author_is_me` gates by authorship exactly like `apply_edit`, so a peer can
/// only unsend messages THEY sent — never ours. Returns the updated message.
pub fn apply_delete(
    config_dir: &Path,
    peer_id: &str,
    target_id: &str,
    author_is_me: bool,
) -> Option<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.get_mut(peer_id)?;
    let msg = thread
        .iter_mut()
        .find(|m| m.id == target_id && m.from_me == author_is_me)?;
    msg.deleted = true;
    msg.text = String::new();
    msg.link_preview = None;
    msg.files.clear();
    msg.path = None;
    msg.gif = None;
    msg.reactions.clear();
    msg.rev = bump_rev(msg.rev);
    msg.text_rev = msg.rev;
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
}

/// Mark every message WE sent with `ts <= up_to` that reached them as "read"
/// (a read receipt from the peer covers them). Returns the messages whose status actually changed so
/// the caller can re-emit just those to the UI.
pub fn mark_read_up_to(config_dir: &Path, peer_id: &str, up_to: u64) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let mut changed = Vec::new();
    if let Some(thread) = all.get_mut(peer_id) {
        for m in thread.iter_mut() {
            // Only a message that actually reached them can be read: one still
            // "sending"/"failed" (e.g. a link message waiting on its preview
            // while a later one got through) must stay in the outbox (D12).
            let reached = matches!(m.status.as_deref(), Some("delivered") | Some("held") | Some("sent"));
            if m.from_me && m.ts <= up_to && reached {
                m.status = Some("read".to_string());
                changed.push(m.clone());
            }
        }
    }
    if !changed.is_empty() {
        save_all(config_dir, all);
    }
    changed
}

/// A pending edit/unsend/reaction op for a message WE authored, queued durably so it
/// survives the friend being offline — the mirror of the message outbox, but for ops.
/// Persisted to chat_ops.json (a NEW file old builds ignore). Flushed by the chat
/// outbox loop ONLY once the target message is delivered/read (the receiver drops an
/// op whose target it hasn't stored, so an op must never race ahead of its original),
/// and the receiver's apply_* are idempotent so an at-least-once retry is safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatOp {
    pub id: String,
    pub peer_id: String,
    pub target_id: String,
    /// "reaction" | "edit" | "delete".
    pub kind: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub add: bool,
    #[serde(default)]
    pub text: String,
    pub ts: u64,
}

const MAX_OPS: usize = 1000;
const OP_MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

fn ops_path(config_dir: &Path) -> PathBuf {
    config_dir.join("chat_ops.json")
}
fn load_ops(config_dir: &Path) -> Vec<ChatOp> {
    match crate::settings::read_json_store(&ops_path(config_dir)) {
        crate::settings::StoreRead::Loaded(ops) => ops,
        _ => Vec::new(),
    }
}
fn save_ops(config_dir: &Path, ops: &[ChatOp]) {
    let _ = fs::create_dir_all(config_dir);
    if let Ok(txt) = serde_json::to_string(ops) {
        if let Err(e) = write_atomic(&ops_path(config_dir), txt.as_bytes()) {
            log::error!("chat::save_ops failed to persist chat_ops.json: {e}");
        }
    }
}

/// Queue an edit/unsend/reaction op, COALESCING against what's already queued so the
/// queue mirrors the message's final LOCAL state (no divergence when it flushes):
///  - delete supersedes every queued op for that target (an unsent message needs no
///    edits/reactions sent);
///  - a newer edit replaces an older queued edit (latest-edit-wins);
///  - a reaction that toggles its queued opposite cancels it (an offline add+remove of
///    the same emoji nets to nothing); a same-direction repeat just replaces.
pub fn enqueue_op(config_dir: &Path, op: ChatOp) {
    let _guard = CACHE.lock().unwrap();
    let mut ops = load_ops(config_dir);
    match op.kind.as_str() {
        "delete" => ops.retain(|o| !(o.peer_id == op.peer_id && o.target_id == op.target_id)),
        "edit" => ops.retain(|o| {
            !(o.peer_id == op.peer_id && o.target_id == op.target_id && o.kind == "edit")
        }),
        "reaction" => {
            if let Some(pos) = ops.iter().position(|o| {
                o.peer_id == op.peer_id
                    && o.target_id == op.target_id
                    && o.kind == "reaction"
                    && o.emoji == op.emoji
            }) {
                let prev_add = ops[pos].add;
                ops.remove(pos);
                if prev_add != op.add {
                    // add then remove (or vice-versa) of the same emoji → no net change.
                    save_ops(config_dir, &ops);
                    return;
                }
            }
        }
        _ => {}
    }
    ops.push(op);
    if ops.len() > MAX_OPS {
        let drop = ops.len() - MAX_OPS;
        ops.drain(0..drop);
    }
    save_ops(config_dir, &ops);
}

/// Drop a delivered op by id.
pub fn ack_op(config_dir: &Path, op_id: &str) {
    let _guard = CACHE.lock().unwrap();
    let mut ops = load_ops(config_dir);
    let before = ops.len();
    ops.retain(|o| o.id != op_id);
    if ops.len() != before {
        save_ops(config_dir, &ops);
    }
}

/// Pending ops oldest-first, pruning any older than OP_MAX_AGE_MS (a peer who never
/// returns shouldn't pin the queue forever).
pub fn pending_ops(config_dir: &Path) -> Vec<ChatOp> {
    let _guard = CACHE.lock().unwrap();
    let mut ops = load_ops(config_dir);
    let now = now_ms();
    let before = ops.len();
    ops.retain(|o| now.saturating_sub(o.ts) < OP_MAX_AGE_MS);
    if ops.len() != before {
        save_ops(config_dir, &ops);
    }
    ops.sort_by(|a, b| (a.ts, &a.id).cmp(&(b.ts, &b.id)));
    ops
}

/// The delivery status of a message WE sent (the op ordering gate). None if we don't
/// have it — the target is the PEER's own message, or it aged out of our store.
pub fn message_status(config_dir: &Path, peer_id: &str, msg_id: &str) -> Option<String> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir)
        .get(peer_id)?
        .iter()
        .find(|m| m.id == msg_id && m.from_me)
        .and_then(|m| m.status.clone())
}

/// Cheap change signal for the outbox loop: the mtimes of chats.json + chat_ops.json.
/// Lets an IDLE tick skip re-parsing the (potentially multi-MB) store entirely when
/// nothing has been written since the last empty round.
pub fn store_mtimes(config_dir: &Path) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let m = |p: PathBuf| fs::metadata(p).and_then(|md| md.modified()).ok();
    (m(chats_path(config_dir)), m(ops_path(config_dir)))
}

/// All messages WE sent that haven't been delivered yet ("sending"/"failed"),
/// oldest first — the outbox the retry loop flushes when a peer comes online.
pub fn outbox(config_dir: &Path) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let mut out: Vec<ChatMessage> = store_mut(&mut cache, config_dir)
        .values()
        .flatten()
        .filter(|m| {
            // A message unsent before it was ever delivered is never delivered.
            m.from_me && !m.deleted && matches!(m.status.as_deref(), Some("sending") | Some("failed"))
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    out
}

/// Whether any of OUR undelivered messages is queued for one of these threads —
/// the cheap gate for waking the outbox when a friend is seen online.
pub fn has_outbox_for(config_dir: &Path, peer_ids: &[&str]) -> bool {
    let mut cache = CACHE.lock().unwrap();
    let store = store_mut(&mut cache, config_dir);
    peer_ids.iter().any(|peer| {
        store.get(*peer).is_some_and(|thread| {
            thread.iter().any(|m| {
                m.from_me && !m.deleted && matches!(m.status.as_deref(), Some("sending") | Some("failed") | Some("held"))
            })
        })
    })
}

/// A short preview of each conversation, for the chat list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatOverview {
    pub peer_id: String,
    pub last_text: String,
    pub last_ts: u64,
    pub last_from_me: bool,
    pub count: usize,
}

pub fn overview(config_dir: &Path) -> Vec<ChatOverview> {
    let mut cache = CACHE.lock().unwrap();
    let mut out: Vec<ChatOverview> = store_mut(&mut cache, config_dir)
        .iter()
        .filter_map(|(peer_id, msgs)| {
            let last = msgs.last()?;
            Some(ChatOverview {
                peer_id: peer_id.clone(),
                last_text: preview(last),
                last_ts: last.ts,
                last_from_me: last.from_me,
                count: msgs.len(),
            })
        })
        .collect();
    // Most recent conversation first.
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    out
}

fn preview(m: &ChatMessage) -> String {
    if m.deleted {
        return "Message deleted".to_string();
    }
    if m.gif.is_some() {
        return "🎞️ GIF".to_string();
    }
    if m.kind == "file" {
        match m.files.len() {
            0 => "📎 File".to_string(),
            1 => format!("📎 {}", m.files[0]),
            n => format!("📎 {n} files"),
        }
    } else {
        m.text.clone()
    }
}

fn bump_rev(rev: u64) -> u64 {
    now_ms().max(rev + 1)
}

/// Confirmed delivery progress of a message we sent, ranked so two copies merge
/// upward. Everything short of "delivered" ranks 0: a copy still "sending" on
/// the device that owns the outbox must never be overridden to stop retrying.
fn status_rank(status: Option<&str>) -> u8 {
    match status {
        Some("read") => 2,
        Some("delivered") => 1,
        _ => 0,
    }
}

/// Message identity across devices: direction + id (an incoming id can never
/// collide with one of ours — the same rule `append` dedups by).
pub(crate) fn sync_key(m: &ChatMessage) -> String {
    format!("{}{}", if m.from_me { "o:" } else { "i:" }, m.id)
}

/// Fingerprint of a message's mutable state, for own-device sync digests.
pub(crate) fn sync_hash(m: &ChatMessage) -> String {
    use sha2::{Digest, Sha256};
    let mut state = format!("{}|{}|{}|{}", m.rev, status_rank(m.status.as_deref()), m.deleted, m.edited);
    // Per-field clocks (D17): two copies with the same `rev` can still differ
    // in a reaction or the text, so those clocks are part of the state. (Kept
    // out of the hash when absent, so untouched messages hash as before.)
    if m.text_rev > 0 || !m.reaction_revs.is_empty() {
        let mut revs: Vec<String> = m.reaction_revs.iter().map(|r| format!("{}{}{}{}", u8::from(r.from_me), r.emoji, r.at, u8::from(r.on))).collect();
        revs.sort();
        state.push_str(&format!("|{}|{}", m.text_rev, revs.join(",")));
    }
    hex::encode(&Sha256::digest(state.as_bytes())[..6])
}

/// Own devices keep the newest messages of each thread in step; older history
/// (which each device may have trimmed differently) is left alone.
const SYNC_WINDOW: usize = 1000;

/// `(sync_key, sync_hash)` for the newest `SYNC_WINDOW` messages, oldest first.
pub(crate) fn sync_digest(config_dir: &Path, peer_id: &str) -> Vec<(String, String)> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir)
        .get(peer_id)
        .map(|t| t[t.len().saturating_sub(SYNC_WINDOW)..].iter().map(|m| (sync_key(m), sync_hash(m))).collect())
        .unwrap_or_default()
}

/// The messages of a thread whose `sync_key` is in `keys`, device-local paths stripped.
pub(crate) fn sync_messages(config_dir: &Path, peer_id: &str, keys: &std::collections::HashSet<String>) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    store_mut(&mut cache, config_dir)
        .get(peer_id)
        .map(|t| t.iter().filter(|m| keys.contains(&sync_key(m))).cloned().map(|mut m| { m.path = None; m }).collect())
        .unwrap_or_default()
}

/// Merge messages from one of the user's OTHER devices into the local thread
/// `peer_id`. New messages are inserted — a copy still "sending" on the other
/// device reads "sent" here, so this device's outbox never re-sends it — and a
/// message both have keeps the newer content (higher `rev`) and the furthest
/// delivery status. Returns how many messages changed.
pub(crate) fn merge_synced(config_dir: &Path, peer_id: &str, incoming: Vec<ChatMessage>) -> usize {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.entry(peer_id.to_owned()).or_default();
    let mut changed = 0;
    for mut m in incoming {
        m.peer_id = peer_id.to_owned();
        m.path = None;
        if m.from_me && status_rank(m.status.as_deref()) == 0 {
            // Not ours to deliver: the device that sent it owns the retry.
            m.status = Some("sent".into());
        }
        match thread.iter_mut().find(|x| x.id == m.id && x.from_me == m.from_me) {
            None => {
                // Older than everything a full thread keeps: it would be trimmed
                // at once — skip it rather than churn (and never converge).
                if thread.len() >= MAX_PER_PEER && thread.first().is_some_and(|f| m.ts < f.ts) {
                    continue;
                }
                // `seq` is a per-device Lamport clock, so the other device's
                // numbers mean nothing here: slot the message in by its time,
                // right after the latest local message that isn't newer.
                m.seq = slot_seq(thread, m.ts);
                thread.push(m);
                changed += 1;
            }
            Some(x) => {
                let before = sync_hash(x);
                merge_fields(x, m.clone());
                if x.from_me && status_rank(m.status.as_deref()) > status_rank(x.status.as_deref()) {
                    x.status = m.status;
                }
                if sync_hash(x) != before {
                    changed += 1;
                }
            }
        }
    }
    if changed > 0 {
        thread.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
        if thread.len() > MAX_PER_PEER {
            let drop = thread.len() - MAX_PER_PEER;
            thread.drain(0..drop);
        }
        save_all(config_dir, all);
    }
    changed
}

/// Merge another own device's copy `m` into ours field by field (D17): an
/// unsend wins; the text follows its own clock (`text_rev`); each reaction
/// follows its own clock (`reaction_revs`, tombstones included). Copies from
/// older builds (no per-field clocks) fall back to the newer whole record.
/// A copy from an older build has no per-field clocks; when the other copy
/// does, read the legacy copy's state as of its whole-record `rev` (its last
/// change), so a newer edit or reaction change there still wins (review #13).
fn legacy_clocks(c: &mut ChatMessage, other: &ChatMessage) {
    if c.rev == 0 || (c.text_rev > 0 || !c.reaction_revs.is_empty()) {
        return;
    }
    if other.text_rev == 0 && other.reaction_revs.is_empty() {
        return; // both legacy: the whole-record rule below applies
    }
    if c.edited {
        c.text_rev = c.rev;
    }
    for r in &c.reactions {
        c.reaction_revs.push(ReactionRev { emoji: r.emoji.clone(), from_me: r.from_me, at: c.rev, on: true });
    }
    for o in &other.reaction_revs {
        if !c.reactions.iter().any(|r| r.from_me == o.from_me && r.emoji == o.emoji) {
            c.reaction_revs.push(ReactionRev { emoji: o.emoji.clone(), from_me: o.from_me, at: c.rev, on: false });
        }
    }
}

fn merge_fields(x: &mut ChatMessage, mut m: ChatMessage) {
    legacy_clocks(&mut m, x);
    legacy_clocks(x, &m.clone());
    if x.deleted {
        x.rev = x.rev.max(m.rev);
        return;
    }
    if m.deleted {
        x.deleted = true;
        x.text.clear();
        x.files.clear();
        x.gif = None;
        x.path = None;
        x.link_preview = None;
        x.reactions.clear();
        x.text_rev = x.text_rev.max(m.text_rev);
        x.rev = x.rev.max(m.rev);
        return;
    }
    let legacy = m.text_rev == 0 && x.text_rev == 0;
    let text_newer = if legacy {
        m.rev > x.rev || (m.rev == x.rev && m.edited && !x.edited)
    } else {
        m.text_rev > x.text_rev
    };
    if text_newer {
        x.text = m.text.clone();
        x.edited = m.edited;
        x.link_preview = m.link_preview.clone();
        x.text_rev = m.text_rev;
    }
    // Reactions: one clock per (author, emoji).
    let mut revs: Vec<ReactionRev> = x.reaction_revs.clone();
    for r in &m.reaction_revs {
        match revs.iter_mut().find(|o| o.from_me == r.from_me && o.emoji == r.emoji) {
            Some(o) if r.at > o.at || (r.at == o.at && !r.on) => *o = r.clone(),
            Some(_) => {}
            None => revs.push(r.clone()),
        }
    }
    let clocked = |from_me: bool, emoji: &str| revs.iter().find(|o| o.from_me == from_me && o.emoji == emoji).cloned();
    let mut live: Vec<Reaction> = Vec::new();
    let newer_copy = if m.rev > x.rev { &m.reactions } else { &x.reactions };
    for r in x.reactions.iter().chain(m.reactions.iter()) {
        if live.contains(r) {
            continue;
        }
        let keep = match clocked(r.from_me, &r.emoji) {
            Some(c) => c.on,
            // No clock (an older build's copy): the newer record decides.
            None => newer_copy.contains(r),
        };
        if keep {
            live.push(r.clone());
        }
    }
    for c in revs.iter().filter(|c| c.on) {
        let r = Reaction { emoji: c.emoji.clone(), from_me: c.from_me };
        if !live.contains(&r) {
            live.push(r);
        }
    }
    x.reactions = live;
    x.reaction_revs = revs;
    x.rev = x.rev.max(m.rev);
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("db-chat-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn msg(id: &str, peer: &str, ts: u64, seq: u64, from_me: bool) -> ChatMessage {
        ChatMessage {
            file_xfer_id: None,
            id: id.into(),
            peer_id: peer.into(),
            from_me,
            kind: "text".into(),
            text: format!("m{id}"),
            files: vec![],
            bytes: 0,
            path: None,
            status: if from_me { Some("sending".into()) } else { None },
            ts,
            seq,
            reply_to: None,
            reply_preview: None,
            reactions: vec![],
            edited: false,
            deleted: false,
            gif: None,
            rev: 0,
            held_on: None,
            server_note: None,
            via: None,
            deliveries: vec![],
            link_preview: None,
            text_rev: 0, reaction_revs: vec![],
        }
    }

    #[test]
    fn file_caption_survives_storage_and_wire_payload() {
        let dir = test_dir("caption");
        let mut file = msg("file", "peer", 1000, 1, true);
        file.kind = "file".into();
        file.text = "Here is the photo".into();
        file.files = vec!["photo.png".into()];
        file.path = Some("/private/photo.png".into());
        file.file_xfer_id = Some(uuid::Uuid::new_v4().to_string());
        append(&dir, &file);
        let restored = load_all(&dir);
        let payload = crate::iroh_net::chat_payload(&restored["peer"][0], "peer", "Sender");
        assert_eq!(payload["fileXferId"].as_str(), file.file_xfer_id.as_deref());
        assert_eq!(restored["peer"][0].file_xfer_id, file.file_xfer_id);
        let mut legacy = serde_json::to_value(&file).unwrap();
        legacy.as_object_mut().unwrap().remove("fileXferId");
        assert!(serde_json::from_value::<ChatMessage>(legacy).unwrap().file_xfer_id.is_none());
        assert_eq!(payload["msgKind"], "file");
        assert_eq!(payload["text"], "Here is the photo");
        assert_eq!(payload["files"][0], "photo.png");
        assert!(payload.get("path").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seq_orders_over_clock_skew() {
        let dir = test_dir("seq");
        let p = "peer1";
        // A later seq with an EARLIER wall-clock must still sort last (skew-proof).
        append(&dir, &msg("a", p, 1000, 1, true));
        append(&dir, &msg("b", p, 500, 2, false)); // peer's clock is behind
        let got = messages(&dir, p);
        assert_eq!(got.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // next_seq is one past the max seq, regardless of ts.
        assert_eq!(next_seq(&dir, p), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reactions_are_idempotent_and_toggle() {
        let dir = test_dir("react");
        let p = "peer2";
        append(&dir, &msg("x", p, 1, 1, true));
        // Same reaction applied twice = one entry (survives re-delivery).
        apply_reaction(&dir, p, "x", "👍", false, true);
        apply_reaction(&dir, p, "x", "👍", false, true);
        assert_eq!(messages(&dir, p)[0].reactions.len(), 1);
        // A different reactor's same emoji is a distinct entry.
        apply_reaction(&dir, p, "x", "👍", true, true);
        assert_eq!(messages(&dir, p)[0].reactions.len(), 2);
        // Removing toggles it off.
        apply_reaction(&dir, p, "x", "👍", false, false);
        let r = messages(&dir, p)[0].reactions.clone();
        assert_eq!(r.len(), 1);
        assert!(r[0].from_me);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_tombstones_and_edit_marks() {
        let dir = test_dir("edit");
        let p = "peer3";
        append(&dir, &msg("e", p, 1, 1, true));
        apply_edit(&dir, p, "e", "edited!", true);
        let m = messages(&dir, p)[0].clone();
        assert!(m.edited && m.text == "edited!");
        // A REMOTE edit (author_is_me=false) must NOT touch our own message.
        assert!(apply_edit(&dir, p, "e", "hacked", false).is_none());
        // A REMOTE delete must NOT tombstone our own message either.
        assert!(apply_delete(&dir, p, "e", false).is_none());
        apply_delete(&dir, p, "e", true);
        let m = messages(&dir, p)[0].clone();
        assert!(m.deleted && m.text.is_empty());
        // Editing a deleted message is refused.
        assert!(apply_edit(&dir, p, "e", "no", true).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_receipt_marks_only_own_up_to_ts() {
        let dir = test_dir("read");
        let p = "peer4";
        append(&dir, &msg("a", p, 100, 1, true));
        append(&dir, &msg("b", p, 200, 2, true));
        append(&dir, &msg("c", p, 300, 3, false)); // their message — never "read" by us
        set_status(&dir, p, "a", "delivered");
        set_status(&dir, p, "b", "delivered");
        let changed = mark_read_up_to(&dir, p, 200);
        assert_eq!(changed.len(), 2);
        let all = messages(&dir, p);
        assert_eq!(all[0].status.as_deref(), Some("read"));
        assert_eq!(all[1].status.as_deref(), Some("read"));
        assert_eq!(all[2].status, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_receipt_never_marks_an_undelivered_message_read() {
        // D12 repro: a link message still "sending" (waiting on its preview)
        // while a later message was delivered and read — the receipt's ts
        // covers both, but the first must stay in the outbox.
        let dir = test_dir("read-undelivered");
        let p = "peer-d12";
        append(&dir, &msg("link", p, 100, 1, true));
        append(&dir, &msg("failed", p, 110, 2, true));
        set_status(&dir, p, "failed", "failed");
        append(&dir, &msg("held", p, 120, 3, true));
        set_held(&dir, p, "held", "Box");
        append(&dir, &msg("later", p, 150, 4, true));
        set_status(&dir, p, "later", "delivered");
        let changed: Vec<String> = mark_read_up_to(&dir, p, 1_000).into_iter().map(|m| m.id).collect();
        assert_eq!(changed, vec!["held".to_string(), "later".to_string()]);
        let all = messages(&dir, p);
        assert_eq!(all[0].status.as_deref(), Some("sending"));
        assert_eq!(all[1].status.as_deref(), Some("failed"));
        let outbox: Vec<String> = outbox(&dir).into_iter().map(|m| m.id).collect();
        assert!(outbox.contains(&"link".to_string()) && outbox.contains(&"failed".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durable_append_is_not_kept_when_the_disk_write_fails() {
        // D11: chats.json can't be written (here: a directory squats its name).
        let dir = test_dir("durable");
        std::fs::create_dir_all(dir.join("chats.json").join("blocker")).unwrap();
        let m = msg("x", "peer-d11", 1, 1, false);
        assert!(append_durable(&dir, &m).is_err(), "a failed save is reported");
        assert!(messages(&dir, "peer-d11").is_empty(), "and not kept in memory either");
        std::fs::remove_dir_all(dir.join("chats.json")).unwrap();
        assert_eq!(append_durable(&dir, &m), Ok(true));
        assert_eq!(append_durable(&dir, &m), Ok(false), "dedup still works");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn own_device_merge_keeps_concurrent_reactions_and_edits() {
        // D17: device A reacts 👍 and device B (later clock) reacts ❤️ and
        // edits — every change survives on both, whichever order they merge.
        let (a, b) = (test_dir("d17a"), test_dir("d17b"));
        let p = "peer-d17";
        let mut base = msg("m", p, 1, 1, true);
        base.status = Some("delivered".into());
        append(&a, &base);
        append(&b, &base);
        apply_reaction(&a, p, "m", "👍", true, true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        apply_reaction(&b, p, "m", "❤️", true, true).unwrap();
        apply_edit(&b, p, "m", "edited on b", true).unwrap();
        let from_a = messages(&a, p);
        let from_b = messages(&b, p);
        merge_synced(&a, p, from_b);
        merge_synced(&b, p, from_a);
        for d in [&a, &b] {
            let m = messages(d, p).remove(0);
            let mut emojis: Vec<String> = m.reactions.iter().map(|r| r.emoji.clone()).collect();
            emojis.sort();
            assert_eq!(emojis, vec!["❤️".to_string(), "👍".to_string()]);
            assert_eq!(m.text, "edited on b");
        }
        assert_eq!(sync_digest(&a, p), sync_digest(&b, p), "the devices converge");
        // A removal later on A sticks on B (tombstone beats the older add).
        std::thread::sleep(std::time::Duration::from_millis(3));
        apply_reaction(&a, p, "m", "👍", true, false).unwrap();
        merge_synced(&b, p, messages(&a, p));
        merge_synced(&a, p, messages(&b, p));
        for d in [&a, &b] {
            let emojis: Vec<String> = messages(d, p)[0].reactions.iter().map(|r| r.emoji.clone()).collect();
            assert_eq!(emojis, vec!["❤️".to_string()]);
        }
        for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn an_older_builds_newer_edit_and_reaction_still_win() {
        let dir = test_dir("legacy-merge");
        let p = "peer-legacy";
        let mut base = msg("m", p, 1, 1, true);
        base.status = Some("delivered".into());
        append(&dir, &base);
        apply_reaction(&dir, p, "m", "👍", true, true).unwrap(); // clocked here
        let mine = messages(&dir, p).remove(0);
        // An older build edited it later and removed the 👍 (no per-field clocks).
        let mut old = mine.clone();
        old.text_rev = 0;
        old.reaction_revs.clear();
        old.reactions.clear();
        old.text = "edited on the old build".into();
        old.edited = true;
        old.rev = mine.rev + 10_000;
        merge_synced(&dir, p, vec![old]);
        let got = messages(&dir, p).remove(0);
        assert_eq!(got.text, "edited on the old build");
        assert!(got.reactions.is_empty(), "its newer removal wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_seq_and_ts_are_bounded() {
        assert_eq!(cap_seq(u64::MAX, 5), 1_000_005);
        assert_eq!(cap_seq(0, 5), 5);
        assert_eq!(cap_seq(7, 5), 7);
        assert!(cap_ts(u64::MAX) <= now_ms() + 24 * 3600 * 1000);
        let dir = test_dir("seqmax");
        let mut m = msg("big", "p", 1, u64::MAX, false);
        m.seq = u64::MAX;
        append(&dir, &m);
        assert_eq!(next_seq(&dir, "p"), u64::MAX, "saturates instead of overflowing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn incoming_field_caps() {
        assert_eq!(cap_text(&"x".repeat(10_000)).chars().count(), MAX_TEXT_CHARS);
        assert_eq!(cap_preview(&"é".repeat(500)).chars().count(), MAX_PREVIEW_CHARS);
        assert_eq!(cap_file_name(&"a".repeat(5000)).len(), 255);
        let ok = GifMeta { provider: "giphy".into(), id: "1".into(), url: "https://media.giphy.com/a.gif".into(), page: String::new(), w: 10, h: 10 };
        assert!(gif_within_limits(&ok));
        assert!(!gif_within_limits(&GifMeta { url: "h".repeat(100_000), ..ok.clone() }));
        assert!(!gif_within_limits(&GifMeta { provider: "p".repeat(500), ..ok }));
    }

    fn op(peer: &str, target: &str, kind: &str, emoji: &str, add: bool, ts: u64) -> ChatOp {
        ChatOp {
            id: format!("op-{kind}-{target}-{emoji}-{ts}-{}", std::process::id()),
            peer_id: peer.into(),
            target_id: target.into(),
            kind: kind.into(),
            emoji: emoji.into(),
            add,
            text: String::new(),
            ts,
        }
    }
    fn ops_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("db-chatops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); // start clean (process-id dirs can recur)
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn enqueue_op_coalesces_to_local_final_state() {
        let dir = ops_dir("coalesce");
        // Recent timestamps so pending_ops' 7-day age prune doesn't drop them.
        let t = now_ms();
        // Two edits → only the latest text survives.
        let mut e1 = op("p", "m", "edit", "", false, t);
        e1.text = "first".into();
        enqueue_op(&dir, e1);
        let mut e2 = op("p", "m", "edit", "", false, t + 1);
        e2.text = "second".into();
        enqueue_op(&dir, e2);
        let edits: Vec<_> = pending_ops(&dir).into_iter().filter(|o| o.kind == "edit").collect();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].text, "second");
        // Reaction add then remove of the SAME emoji nets to nothing.
        enqueue_op(&dir, op("p", "m", "reaction", "👍", true, t + 2));
        enqueue_op(&dir, op("p", "m", "reaction", "👍", false, t + 3));
        assert!(pending_ops(&dir).iter().all(|o| o.kind != "reaction"), "add+remove cancels");
        // A delete supersedes any queued op for that target.
        enqueue_op(&dir, op("p", "m", "reaction", "🎉", true, t + 4));
        enqueue_op(&dir, op("p", "m", "delete", "", false, t + 5));
        let after = pending_ops(&dir);
        assert_eq!(after.iter().filter(|o| o.target_id == "m").count(), 1);
        assert_eq!(after.iter().find(|o| o.target_id == "m").unwrap().kind, "delete");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ack_and_prune_ops() {
        let dir = ops_dir("ackprune");
        let _ = std::fs::create_dir_all(&dir);
        let mut keep = op("p", "m", "reaction", "👍", true, now_ms());
        keep.id = "keep".into();
        enqueue_op(&dir, keep);
        // An op older than the 7-day max age is pruned by pending_ops.
        let mut old = op("p", "m2", "edit", "", false, now_ms().saturating_sub(8 * 24 * 60 * 60 * 1000));
        old.id = "old".into();
        enqueue_op(&dir, old);
        let got = pending_ops(&dir);
        assert!(got.iter().any(|o| o.id == "keep"));
        assert!(!got.iter().any(|o| o.id == "old"), "aged-out op pruned");
        ack_op(&dir, "keep");
        assert!(pending_ops(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folding_a_devices_thread_slots_by_time_and_keeps_our_outbox() {
        let dir = test_dir("fold");
        // A year of conversation on the main thread (seq 1..3)…
        append(&dir, &msg("old1", "owner", 100, 1, false));
        append(&dir, &msg("old2", "owner", 200, 2, true));
        append(&dir, &msg("new", "owner", 900, 3, false));
        // …and a second device's thread with its own clock (seq 1..2).
        append(&dir, &msg("dev1", "device", 500, 1, false));
        append(&dir, &msg("dev2", "device", 950, 2, true)); // still "sending"
        append(&dir, &msg("old1", "device", 100, 1, false)); // the same message twice
        assert_eq!(fold_thread(&dir, "device", "owner"), 2);
        let order: Vec<String> = messages(&dir, "owner").into_iter().map(|m| m.id).collect();
        assert_eq!(order, ["old1", "old2", "dev1", "new", "dev2"]);
        assert!(messages(&dir, "device").is_empty());
        assert_eq!(outbox(&dir).len(), 2, "our unsent messages keep retrying (old2 + dev2)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn message_status_gate_distinguishes_ours_from_theirs() {
        let dir = ops_dir("status");
        let _ = std::fs::create_dir_all(&dir);
        let mut m = msg("mm", "p", 1, 1, true);
        m.status = Some("delivered".into());
        append(&dir, &m);
        append(&dir, &msg("theirs", "p", 2, 2, false)); // the peer's own message
        assert_eq!(message_status(&dir, "p", "mm").as_deref(), Some("delivered"));
        assert_eq!(message_status(&dir, "p", "theirs"), None); // not from_me → None
        assert_eq!(message_status(&dir, "p", "nope"), None); // unknown
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outbox_gate_is_per_thread_and_ignores_delivered_and_unsent() {
        let dir = test_dir("outbox-for");
        let mut done = msg("d", "alex", 1, 1, true);
        done.status = Some("delivered".into());
        append(&dir, &done);
        append(&dir, &msg("theirs", "alex", 2, 2, false));
        assert!(!has_outbox_for(&dir, &["alex"]), "nothing of ours is waiting");
        let mut gone = msg("u", "alex", 3, 3, true);
        gone.deleted = true;
        append(&dir, &gone);
        assert!(!has_outbox_for(&dir, &["alex"]), "an unsent message is never delivered");
        let mut queued = msg("q", "alex", 4, 4, true);
        queued.status = Some("failed".into());
        append(&dir, &queued);
        assert!(has_outbox_for(&dir, &["other", "alex"]));
        assert!(!has_outbox_for(&dir, &["sam"]), "another friend's thread is not woken");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Merge an authenticated link snapshot exactly like an own-device sync: no
/// duplicates, slotted by time among this device's own messages, and a copy
/// still "sending" on the other device is not this device's to deliver.
pub(crate) fn import_link_thread(config_dir: &Path, peer: &str, messages: Vec<ChatMessage>) {
    merge_synced(config_dir, peer, messages);
}

/// Where a message with wall-clock `ts` belongs in `thread`: right after the
/// latest message that isn't newer. `seq` is a per-thread Lamport clock, so a
/// message from another thread or device can't keep its own number.
fn slot_seq(thread: &[ChatMessage], ts: u64) -> u64 {
    thread.iter().filter(|x| x.ts <= ts).map(|x| x.seq).max().unwrap_or(0)
}

/// Move this device's thread `from` into `into` (both are the same person's
/// conversation). Unlike `merge_threads` the two threads kept separate seq
/// clocks, so messages are slotted in by time; statuses and local paths are
/// kept (these are this device's own copies). Returns how many moved.
pub(crate) fn fold_thread(config_dir: &Path, from: &str, into: &str) -> usize {
    if from == into {
        return 0;
    }
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let Some(moving) = all.remove(from) else { return 0 };
    let dest = all.entry(into.to_owned()).or_default();
    let mut moved = 0;
    for mut m in moving {
        if dest.iter().any(|x| x.id == m.id && x.from_me == m.from_me) {
            continue;
        }
        m.peer_id = into.to_owned();
        m.seq = slot_seq(dest, m.ts);
        dest.push(m);
        moved += 1;
    }
    dest.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    if dest.len() > MAX_PER_PEER {
        let drop = dest.len() - MAX_PER_PEER;
        dest.drain(0..drop);
    }
    save_all(config_dir, all);
    moved
}
