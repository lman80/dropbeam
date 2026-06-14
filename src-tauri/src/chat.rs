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

static LOCK: Mutex<()> = Mutex::new(());

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

fn load_all(config_dir: &Path) -> HashMap<String, Vec<ChatMessage>> {
    match fs::read_to_string(chats_path(config_dir)) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

fn save_all(config_dir: &Path, all: &HashMap<String, Vec<ChatMessage>>) {
    let _ = fs::create_dir_all(config_dir);
    if let Ok(txt) = serde_json::to_string_pretty(all) {
        let _ = write_atomic(&chats_path(config_dir), txt.as_bytes());
    }
}

/// Every message in the conversation with `peer_id`, oldest first.
pub fn messages(config_dir: &Path, peer_id: &str) -> Vec<ChatMessage> {
    load_all(config_dir).remove(peer_id).unwrap_or_default()
}

/// Append a message (dedup by id), bound the history, and persist. Returns
/// `false` if it was a duplicate we'd already stored (so callers can skip the
/// live event and avoid double-rendering).
pub fn append(config_dir: &Path, msg: &ChatMessage) -> bool {
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
    let thread = all.entry(msg.peer_id.clone()).or_default();
    if thread.iter().any(|m| m.id == msg.id) {
        return false;
    }
    thread.push(msg.clone());
    thread.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    if thread.len() > MAX_PER_PEER {
        let drop = thread.len() - MAX_PER_PEER;
        thread.drain(0..drop);
    }
    save_all(config_dir, &all);
    true
}

/// Drop a whole conversation (e.g. when a friend is removed).
pub fn clear(config_dir: &Path, peer_id: &str) {
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
    if all.remove(peer_id).is_some() {
        save_all(config_dir, &all);
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
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
    let Some(mut moving) = all.remove(from_id) else {
        return 0;
    };
    if moving.is_empty() {
        save_all(config_dir, &all);
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
    save_all(config_dir, &all);
    moved
}

/// Update a sent message's delivery status (sending → sent/failed). Returns the
/// updated message so the caller can re-emit it to the UI.
pub fn set_status(config_dir: &Path, peer_id: &str, msg_id: &str, status: &str) -> Option<ChatMessage> {
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
    let thread = all.get_mut(peer_id)?;
    let msg = thread.iter_mut().find(|m| m.id == msg_id)?;
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
    let _guard = LOCK.lock().unwrap();
    let all = load_all(config_dir);
    all.get(peer_id)
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
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
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
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
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
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
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
    let _guard = LOCK.lock().unwrap();
    let mut all = load_all(config_dir);
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
        save_all(config_dir, &all);
    }
    changed
}

/// All messages WE sent that haven't been delivered yet ("sending"/"failed"),
/// oldest first — the outbox the retry loop flushes when a peer comes online.
pub fn outbox(config_dir: &Path) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = load_all(config_dir)
        .into_values()
        .flatten()
        .filter(|m| {
            m.from_me && matches!(m.status.as_deref(), Some("sending") | Some("failed"))
        })
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
    let mut out: Vec<ChatOverview> = load_all(config_dir)
        .into_iter()
        .filter_map(|(peer_id, msgs)| {
            let last = msgs.last()?;
            Some(ChatOverview {
                peer_id,
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
        let dir = std::env::temp_dir().join(format!("db-chat-test-{}", now_ms()));
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
        let dir = std::env::temp_dir().join(format!("db-chat-test-{}", now_ms() + 1));
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
        let dir = std::env::temp_dir().join(format!("db-chat-test-{}", now_ms() + 2));
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
        let dir = std::env::temp_dir().join(format!("db-chat-test-{}", now_ms() + 3));
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
}
