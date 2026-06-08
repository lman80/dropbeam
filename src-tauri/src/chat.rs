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
    pub ts: u64,
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
    thread.sort_by_key(|m| m.ts);
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
