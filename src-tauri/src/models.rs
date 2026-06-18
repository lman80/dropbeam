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

/// Live detail of HOW two peers are connected right now — the data behind the
/// "connection inspector" so the user can see exactly what path their files take,
/// not just a vague Direct/Relay pill.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnDetail {
    /// "local" (same LAN) | "direct" (hole-punched p2p) | "relay" (via a relay
    /// server) | "connecting" (no path selected yet).
    pub path: String,
    /// Round-trip latency in ms for the active path (None if not measured yet).
    pub rtt_ms: Option<u64>,
    /// True when we're on the relay BUT a direct path is actively forming (a
    /// hole-punch is in progress) — drives the live "upgrading to direct…" hint.
    pub upgrading: bool,
    /// Short relay region/host (e.g. "use1") when relayed, else None.
    pub relay: Option<String>,
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
    /// Live connection detail (path kind, rtt, relay, upgrading) for the inspector.
    #[serde(default)]
    pub conn_detail: Option<ConnDetail>,
    /// A short human reason for a PARKED/waiting state — e.g. "Waiting for a direct
    /// connection". Drives the "wait for direct" parked card + "Send over relay anyway".
    #[serde(default)]
    pub detail: Option<String>,
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
            conn_detail: None,
            detail: None,
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
    /// "Wait for a direct connection": hold a transfer until a fast DIRECT/LAN path
    /// forms instead of falling back to the slow relay — but WAIT (park + keep
    /// hole-punching), never fail outright like `require_direct` does. The transfer
    /// card shows "Waiting for a direct connection" with a "Send over relay anyway"
    /// escape. Off by default. Applies to friend sends AND folder sync.
    #[serde(default)]
    pub wait_for_direct: bool,
    /// Absolute path to the user's chosen profile picture (copied into the app
    /// config dir). Empty = no picture (we render initials instead). Local-only.
    #[serde(default)]
    pub avatar: String,
    /// Pop a native OS notification when a chat message arrives while the app
    /// isn't focused (like any messaging app). On by default.
    #[serde(default = "default_true")]
    pub notify_on_message: bool,
    /// Send read receipts: let friends see when you've read their message (like
    /// iMessage/WhatsApp). On by default; turning it off stops you sending them.
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,
    /// A free Giphy API key (developers.giphy.com) that powers GIF search in chat.
    /// Empty by default → the GIF picker shows a one-line setup prompt instead.
    /// Giphy sanctions client-side keys, so this rides in the client safely.
    #[serde(default)]
    pub giphy_api_key: String,
    /// Detailed diagnostics: when on, the file log captures DEBUG-level app
    /// breadcrumbs PLUS iroh's connection internals (hole-punch, relay-vs-direct,
    /// chunk timing). Off by default (keeps logs small). Applied at startup, so
    /// flipping it needs an app restart. Used to diagnose hard-to-reproduce
    /// transfer issues from a tester's machine via Export Diagnostics.
    #[serde(default)]
    pub verbose_logging: bool,
    /// Show the floating "syncing folder…" popup (HUD) during shared-folder
    /// transfers. On by default; turn off if it's distracting. Live (no restart).
    #[serde(default = "default_true")]
    pub show_sync_popup: bool,
    /// Share anonymous background diagnostics (errors + performance metadata, never
    /// file names or contents) so the developer can find and fix issues users never
    /// see. A redacted digest uploads ~once a day. On by default; opt out anytime.
    #[serde(default = "default_true")]
    pub share_diagnostics: bool,
    /// Where diagnostics digests are uploaded — the operator's own collector URL
    /// (e.g. a Cloudflare Worker). EMPTY by default: with no URL set, nothing is
    /// ever sent anywhere. This keeps the destination operator-controlled, never a
    /// baked-in endpoint.
    #[serde(default)]
    pub diagnostics_url: String,
    /// How long a mirror folder keeps deleted/replaced copies in its recovery
    /// history before auto-removing them. 0 = keep forever. Default 30 days —
    /// mirrors macOS "Recently Deleted" so old copies can't pile up unbounded.
    #[serde(default = "default_keep_days")]
    pub folder_history_keep_days: u32,
    /// Per-folder disk budget for the recovery history (bytes). Once a folder's
    /// saved copies exceed this, the oldest are evicted first. 0 = no limit.
    /// Default 2 GiB — caps worst-case disk instead of the old unbounded growth.
    #[serde(default = "default_history_budget")]
    pub folder_history_budget_bytes: u64,
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
            wait_for_direct: false,
            avatar: String::new(),
            notify_on_message: true,
            send_read_receipts: true,
            giphy_api_key: String::new(),
            verbose_logging: false,
            show_sync_popup: true,
            share_diagnostics: true,
            diagnostics_url: String::new(),
            folder_history_keep_days: default_keep_days(),
            folder_history_budget_bytes: default_history_budget(),
        }
    }
}

/// Default recovery-history retention: keep deleted/replaced copies for 30 days.
fn default_keep_days() -> u32 {
    30
}

/// Default per-folder recovery-history budget: 2 GiB.
fn default_history_budget() -> u64 {
    2 * 1024 * 1024 * 1024
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
    /// Per-member access roles (owner-authoritative). A "viewer" gets a read-only
    /// copy: they RECEIVE the folder but never push their own changes back.
    ///   • `i_am_viewer`  — THIS device is a viewer on this folder → we never send
    ///     (no watcher/sender, no reconcile push, no delete propagation).
    ///   • `peer_is_viewer` — the PEER on this link is a viewer → we never accept
    ///     their pushes/deletes (they shouldn't be changing the folder).
    /// Both default false = full editor, so every existing folder is unchanged.
    /// The folder owner assigns roles; they ride the roster beacon so the whole
    /// mesh converges. See pairing::runs_sender/runs_listener.
    #[serde(default)]
    pub i_am_viewer: bool,
    #[serde(default)]
    pub peer_is_viewer: bool,
    /// Role authority + convergence: the folder OWNER's (creator's) endpoint_id —
    /// only role assignments from the owner are trusted. `role_epoch` is the
    /// owner's monotonically-increasing version of the role assignment; we only
    /// apply a roster whose epoch is newer than ours (kills role flapping and the
    /// "any member can silence an editor" issue). Owner bumps it on each change;
    /// every group link shares the same owner_eid + the latest epoch we've seen.
    #[serde(default)]
    pub owner_eid: Option<String>,
    #[serde(default)]
    pub role_epoch: u64,
    /// Sync paused for this folder. A SHARED switch: either member can pause/resume,
    /// the state rides the control beacon, and the NEWEST toggle (by `pause_epoch`,
    /// epoch-ms) wins so both sides converge. While paused, nothing uploads, no
    /// deletes/renames propagate, and incoming changes aren't applied — each person
    /// edits their own copy freely; Resume runs the normal reconcile to merge both.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub pause_epoch: u64,
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

/// A per-folder rollup of recovery-history disk usage, for the storage view.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderHistorySummary {
    /// A pair id that points at this folder (the UI uses it for restore/clear).
    pub pair_id: String,
    /// Friendly folder name (its last path component).
    pub folder_name: String,
    /// The local folder path (deduped on — group folders share one path).
    pub folder: String,
    /// Total bytes the saved copies occupy.
    pub bytes: u64,
    /// How many saved copies are kept.
    pub item_count: u64,
    /// Timestamp (ms) of the oldest saved copy, if any.
    pub oldest_ms: Option<u64>,
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
    /// Sync is paused for this folder (a shared switch — either member set it). The
    /// UI shows a Paused badge + a Resume button instead of live sync.
    #[serde(default)]
    pub paused: bool,
    /// Live connection detail for the active folder transfer (inspector data).
    #[serde(default)]
    pub conn_detail: Option<ConnDetail>,
}

