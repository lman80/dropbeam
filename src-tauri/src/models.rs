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

/// Which channel the active connection is using — so the UI can tell the user
/// exactly how their files are flowing (and roughly how fast to expect).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Locality {
    /// Still figuring out the route (connecting / hole-punching).
    Unknown,
    /// Same local network — a direct path to a private/link-local address. Fastest.
    Local,
    /// Hole-punched DIRECT peer-to-peer over the internet, no relay. Fast + private.
    Direct,
    /// Relayed through a public relay (the slow fallback when no direct path forms).
    /// Serialized "internet" for backward-compat with old clients + history; the UI
    /// labels it "Relay".
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
    /// Use the direct peer-to-peer engine (iroh) — the default transport. On by
    /// default; can be turned off to fall back to the relay engine (croc).
    #[serde(default = "default_true")]
    pub direct_mode: bool,
    /// Cap the OUTGOING transfer speed over the internet, in megabits/sec, to
    /// leave headroom for the rest of your connection (0 = unlimited). Only
    /// applies to internet/relay transfers — local-network sends stay full speed.
    #[serde(default)]
    pub upload_limit_mbps: u32,
    /// Show transfer speeds in megaBITS/sec (Mbps) instead of megaBYTES/sec
    /// (MB/s). Off by default (MB/s, what most file tools show).
    #[serde(default)]
    pub show_megabits: bool,
    /// Only send over a DIRECT path (local network or hole-punched peer-to-peer).
    /// If no direct path forms, the send fails instead of falling back to the slow
    /// relay. Off by default. Applies to Quick Send + friend sends, not the
    /// background folder sync (which always uses the best available path).
    #[serde(default)]
    pub require_direct: bool,
    /// Absolute path to the user's chosen profile picture (copied into the app
    /// config dir). Empty = no picture (we render initials instead). Local-only.
    #[serde(default)]
    pub avatar: String,
    /// Pop a native OS notification when a chat message arrives while the app
    /// isn't focused (like any messaging app). On by default.
    #[serde(default = "default_true")]
    pub notify_on_message: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: String::new(),
            display_name: String::new(),
            theme: "system".into(),
            minimize_to_tray: true,
            // Always ready: new installs auto-start (silently, in the menu bar) so
            // a friend's file can land without the app being open first.
            launch_at_login: true,
            prefer_direct_p2p: true,
            custom_relay: String::new(),
            custom_relay_pass: String::new(),
            notify_on_complete: true,
            play_sounds: true,
            direct_mode: true,
            upload_limit_mbps: 0,
            show_megabits: false,
            require_direct: false,
            avatar: String::new(),
            notify_on_message: true,
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
    /// Total-sync / mirror: a shared source of truth. Implies two-way, and
    /// additionally propagates deletes and replaces edited files (instead of
    /// keeping a duplicate). Deleted/overwritten files go to the folder's history
    /// so nothing is ever lost.
    #[serde(default)]
    pub mirror: bool,
    /// Delete the local copy after delivery is confirmed.
    pub auto_delete: bool,
    pub delete_mode: DeleteMode,
    pub created_at: u64,
    /// The folder peer's iroh EndpointId, learned at pairing — so folder syncs
    /// go directly over iroh (dial-by-key) instead of croc. None if paired before
    /// this or not yet exchanged.
    #[serde(default)]
    pub endpoint_id: Option<String>,
    /// Multi-person folders: all the pairwise links that share ONE folder among a
    /// group carry the same `group_id`. None = a classic 1:1 folder (every
    /// existing folder). A group of N people is N-1 of these links per member,
    /// all pointing at the same local `folder`; the roster propagates over the
    /// control beacon so everyone meshes with everyone.
    #[serde(default)]
    pub group_id: Option<String>,
}

/// One restorable entry in a folder's history (a deleted or overwritten file).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    /// Path relative to the shared folder (what it was before it left).
    pub rel_path: String,
    pub size: u64,
    /// "deleted" or "replaced".
    pub reason: String,
    pub timestamp_ms: u64,
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
    /// The friend's iroh EndpointId (their stable device key), learned during
    /// pairing. When present + Direct mode is on, we send to them directly over
    /// iroh (dial-by-key) instead of croc. None for friends paired before this.
    #[serde(default)]
    pub endpoint_id: Option<String>,
    /// Local path to the friend's profile picture (received over the wire and
    /// cached in the config dir). None = render their initials.
    #[serde(default)]
    pub avatar: Option<String>,
    /// True once the user has renamed this friend locally — so an incoming profile
    /// broadcast never overwrites a name the user deliberately chose.
    #[serde(default)]
    pub name_custom: bool,
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
    /// Whether the paired peer is currently reachable (their control channel
    /// answered recently). Drives the online/offline dot + clears the stale
    /// "waiting for someone to accept" state once we've actually heard from them.
    pub peer_online: bool,
    /// The peer's display name, learned over the control channel (the creator
    /// side has no name until the accepter says hello).
    pub peer_name: Option<String>,
    /// Whether the active transfer is going over the LAN or the internet relay —
    /// the same signal Quick Send shows, so folder speed is explainable.
    pub locality: Locality,
    /// The peer removed/stopped sharing this folder on their side. The link is
    /// effectively dead; the UI surfaces "no longer shared by ___".
    #[serde(default)]
    pub peer_unshared: bool,
    /// File names still waiting in the send queue (the active one excluded), so a
    /// dropped batch shows as a list with per-file rows instead of popping up one
    /// file at a time. Capped to keep the event payload small.
    #[serde(default)]
    pub queued_files: Vec<String>,
    /// How many files the peer reported in its last reconcile — lets the UI show
    /// "both have N files, in sync" so the user can confirm the folders match.
    #[serde(default)]
    pub peer_files: u32,
    /// Aggregate progress for the current send burst (a folder drop). Drives ONE
    /// "12 of 50 files" progress bar instead of a card flashing per file.
    #[serde(default)]
    pub session_total_files: u32,
    #[serde(default)]
    pub session_done_files: u32,
}

