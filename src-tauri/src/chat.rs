//! Peer-to-peer chat with friends, riding the same iroh endpoint as transfers.
//!
//! Messages travel over the shared `dropbeam/1` ALPN as `{kind:"chat", ...}`
//! frames (dial-by-EndpointId, exactly like the folder control beacon). Each
//! conversation is persisted per friend in `chats.json` so it survives restarts.
//!
//! Delivery is online-only for now: a message to an offline friend is stored
//! locally and shown in your own thread, but there's no store-and-forward yet.

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
    match fs::read_to_string(chats_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => HashMap::new(),
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
            if let Err(e) = write_atomic(&chats_path(config_dir), txt.as_bytes()) {
                log::error!("chat::save_all failed to persist chats.json: {e}");
            }
        }
        Err(e) => log::error!("chat::save_all failed to serialize chats: {e}"),
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
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let thread = all.entry(msg.peer_id.clone()).or_default();
    // Dedup scoped by direction: an incoming (peer-chosen) id can never collide
    // with one of OUR outgoing ids and silently suppress a real message.
    if thread.iter().any(|m| m.id == msg.id && m.from_me == msg.from_me) {
        return false;
    }
    thread.push(msg.clone());
    thread.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    if thread.len() > MAX_PER_PEER {
        let drop = thread.len() - MAX_PER_PEER;
        thread.drain(0..drop);
    }
    save_all(config_dir, all);
    true
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
    msg.status = Some(status.to_string());
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
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
        .map(|t| t.iter().map(|m| m.seq).max().unwrap_or(0) + 1)
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
    match (add, existing) {
        (true, None) => msg.reactions.push(Reaction { emoji: emoji.to_string(), from_me }),
        (false, Some(i)) => {
            msg.reactions.remove(i);
        }
        _ => {} // already in the desired state — idempotent no-op
    }
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
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
    msg.edited = true;
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
    msg.files.clear();
    msg.path = None;
    msg.gif = None;
    msg.reactions.clear();
    let out = msg.clone();
    save_all(config_dir, &all);
    Some(out)
}

/// Mark every message WE sent with `ts <= up_to` as "read" (a read receipt from
/// the peer covers them). Returns the messages whose status actually changed so
/// the caller can re-emit just those to the UI.
pub fn mark_read_up_to(config_dir: &Path, peer_id: &str, up_to: u64) -> Vec<ChatMessage> {
    let mut cache = CACHE.lock().unwrap();
    let all = store_mut(&mut cache, config_dir);
    let mut changed = Vec::new();
    if let Some(thread) = all.get_mut(peer_id) {
        for m in thread.iter_mut() {
            if m.from_me && m.ts <= up_to && m.status.as_deref() != Some("read") {
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
    match fs::read_to_string(ops_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => Vec::new(),
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
            m.from_me && matches!(m.status.as_deref(), Some("sending") | Some("failed"))
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    out
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
        }
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
        let changed = mark_read_up_to(&dir, p, 200);
        assert_eq!(changed.len(), 2);
        let all = messages(&dir, p);
        assert_eq!(all[0].status.as_deref(), Some("read"));
        assert_eq!(all[1].status.as_deref(), Some("read"));
        assert_eq!(all[2].status, None);
        let _ = std::fs::remove_dir_all(&dir);
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
}
