//! Shared data types exchanged between the Rust core and the React frontend.
//! All structs serialize with camelCase field names for idiomatic TS access.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Send,
    Receive,
}

/// Lifecycle of a single transfer, as surfaced to the UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransferState {
    /// Process spawned, nothing parsed yet.
    Starting,
    /// Sender is showing a code and waiting for the receiver to connect.
    WaitingForPeer,
    /// Peer found, securing channel.
    Connecting,
    /// A friend with manual-accept is offering files; waiting for the user to
    /// accept or decline before any bytes move.
    WaitingForAccept,
    /// Bytes are moving.
    Transferring,
    /// Finished successfully (sender: receiver confirmed full receipt).
    Completed,
    /// Ended with an error.
    Failed,
    /// User canceled.
    Canceled,
}

/// Whether the active connection is on the LAN or via the internet relay.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Locality {
    Unknown,
    Local,
    Internet,
}

/// Full snapshot of a transfer's progress, emitted on the `transfer://update`
/// event. The frontend keeps a map keyed by `id` and replaces the whole entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferUpdate {
    pub id: String,
    pub direction: Direction,
    pub state: TransferState,
    pub code: Option<String>,
    pub file_names: Vec<String>,
    pub file_count: usize,
    pub percent: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Instantaneous speed in bytes/second.
    pub speed_bps: f64,
    pub eta_seconds: Option<f64>,
    pub locality: Locality,
    pub peer: Option<String>,
    pub error: Option<String>,
    /// For receives, the directory files are landing in.
    pub out_dir: Option<String>,
    /// Set when sending directly to a friend (shows "Sending to {name}", no code).
    pub friend_name: Option<String>,
}

impl TransferUpdate {
    pub fn new(id: String, direction: Direction, file_names: Vec<String>) -> Self {
        let file_count = file_names.len();
        TransferUpdate {
            id,
            direction,
            state: TransferState::Starting,
            code: None,
            file_names,
            file_count,
            percent: 0.0,
            bytes_done: 0,
            bytes_total: 0,
            speed_bps: 0.0,
            eta_seconds: None,
            locality: Locality::Unknown,
            peer: None,
            error: None,
            out_dir: None,
            friend_name: None,
        }
    }
}

/// A persisted record of a past transfer (success or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub direction: Direction,
    pub file_names: Vec<String>,
    pub bytes_total: u64,
    pub peer: Option<String>,
    pub locality: Locality,
    pub code: Option<String>,
    pub state: TransferState,
    pub timestamp_ms: u64,
    pub error: Option<String>,
    pub out_dir: Option<String>,
}

/// User-configurable application settings, persisted to settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Where received Quick Send files are saved.
    pub download_dir: String,
    /// The name peers see you as.
    pub display_name: String,
    /// "system" | "light" | "dark".
    pub theme: String,
    /// Hide to tray instead of quitting when the window is closed.
    pub minimize_to_tray: bool,
    /// Start DropBeam automatically at login.
    pub launch_at_login: bool,
    /// Prefer direct peer-to-peer connections over the relay.
    pub prefer_direct_p2p: bool,
    /// Custom relay address (host:port). Empty = use the public relay.
    pub custom_relay: String,
    /// Custom relay password. Empty = default.
    pub custom_relay_pass: String,
    /// Show a native notification when a transfer completes.
    pub notify_on_complete: bool,
    /// Play short sounds on send/receive events.
    #[serde(default = "default_true")]
    pub play_sounds: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: String::new(),
            display_name: String::new(),
            theme: "system".into(),
            minimize_to_tray: true,
            launch_at_login: false,
            prefer_direct_p2p: true,
            custom_relay: String::new(),
            custom_relay_pass: String::new(),
            notify_on_complete: true,
            play_sounds: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared Drop Folders (pairing)
// ---------------------------------------------------------------------------

/// Which side of the pair this device is. The inviter is A, the accepter is B.
/// This determines the derived transfer-code channels so the two sides never
/// collide (A sends on "a2b", B sends on "b2a").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PairRole {
    A,
    B,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    Trash,
    Permanent,
}

/// A persisted Shared Drop Folder paired with exactly one peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub id: String,
    pub role: PairRole,
    pub peer_name: String,
    /// Shared high-entropy secret; transfer codes are derived from it.
    pub secret: String,
    /// The local watched folder.
    pub folder: String,
    /// Sync both directions (vs. one-way: A sends, B receives).
    pub two_way: bool,
    /// Delete the local copy after delivery is confirmed.
    pub auto_delete: bool,
    pub delete_mode: DeleteMode,
    pub created_at: u64,
}

/// A friend — a named peer you can send files to directly, no code needed.
/// Backed by the same shared-secret/derived-channel model as a pair. Each friend
/// runs a small inbox listener so files sent to you arrive automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    pub id: String,
    pub role: PairRole,
    pub name: String,
    pub secret: String,
    pub created_at: u64,
    /// When true, files this friend sends arrive automatically. When false, each
    /// incoming file waits for the user to accept or decline. Defaults to true so
    /// existing friends (and the common case) keep the frictionless behavior.
    #[serde(default = "default_true")]
    pub auto_accept: bool,
}

fn default_true() -> bool {
    true
}

/// Live sync status for a Shared Drop Folder, emitted on `folder://status`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FolderState {
    /// Everything delivered — in sync.
    Idle,
    Sending,
    Receiving,
    /// Files queued, waiting for the peer to come online.
    Waiting,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub pair_id: String,
    pub state: FolderState,
    pub queued: usize,
    pub sending_file: Option<String>,
    pub percent: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: f64,
    pub eta_seconds: Option<f64>,
    pub detail: Option<String>,
}

