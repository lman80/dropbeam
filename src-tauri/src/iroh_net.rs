//! DropBeam's next-generation transport, built on **iroh** (direct P2P over QUIC
//! with NAT hole-punching + an encrypted relay fallback that can't read your
//! data). This replaces croc's relay-only model.
//!
//! Phase 1 (this file) stands up the foundation only — it does NOT yet carry any
//! user-facing flow, so croc keeps doing all real transfers (dual-stack). What
//! lives here now:
//!   * a persistent ed25519 device identity (stable `EndpointId` across restarts),
//!   * one long-lived `Endpoint` shared by the whole app,
//!   * an accept loop with a tiny protocol dispatcher (echo, for self-test),
//!   * a self-test that proves iroh works *inside the real app* on this machine.
//!
//! Later phases add the real protocols (Quick Send, Friends, Shared Folders) as
//! additional ALPN-tagged stream handlers on this same endpoint.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, SecretKey};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::OnceCell;

use crate::models::{Direction, TransferState, TransferUpdate};

/// Application-layer protocol id. Bumped if the wire format changes.
pub const ALPN: &[u8] = b"dropbeam/1";
/// How long a direct "send to a friend" keeps re-dialing an unreachable friend
/// before giving up — lets a friend who's just opening their app still receive,
/// mirroring croc's old parked-send window (without the hours-long hang).
const FRIEND_SEND_RETRY_SECS: u64 = 90;

/// A Quick Send staged on this node, waiting for a receiver to pull it.
struct PendingSend {
    transfer_id: String,
    paths: Vec<PathBuf>,
    names: Vec<String>,
    total: u64,
    cancel: Arc<AtomicBool>,
}

/// Shared iroh state, managed by Tauri as `Arc<IrohState>`. The boot task fills
/// `endpoint` once the node is up; commands and the accept loop read from here.
#[derive(Default)]
pub struct IrohState {
    pub endpoint: OnceCell<Endpoint>,
    /// Set at startup so the accept loop can emit `transfer://update` events.
    pub app: OnceCell<AppHandle>,
    /// Quick Sends awaiting a puller, keyed by the ticket's one-time token.
    pending: Mutex<HashMap<String, PendingSend>>,
    /// Cancellation flags for in-flight transfers, keyed by transfer id.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl IrohState {
    /// Signal cancellation for a transfer id. Drops a still-staged send so it
    /// can't be pulled, and flips the in-flight flag for a running transfer.
    pub fn cancel(&self, id: &str) -> CancelKind {
        let was_staged = {
            let mut p = self.pending.lock().unwrap();
            let before = p.len();
            p.retain(|_, ps| ps.transfer_id != id);
            before != p.len()
        };
        let active = if let Some(flag) = self.cancels.lock().unwrap().get(id) {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        };
        if was_staged {
            CancelKind::Staged
        } else if active {
            CancelKind::Active
        } else {
            CancelKind::Unknown
        }
    }
}

impl IrohState {
    /// The endpoint once it's ready (None during the brief startup window).
    pub fn get(&self) -> Option<&Endpoint> {
        self.endpoint.get()
    }
}

fn emit(app: &AppHandle, u: &TransferUpdate) {
    let _ = app.emit("transfer://update", u);
}

/// A throttled progress callback that emits `Transferring` updates (with live
/// speed) for a transfer id — shared by the send (serve) and receive (pull) sides.
fn progress_cb(
    app: AppHandle,
    id: String,
    dir: Direction,
    names: Vec<String>,
    friend: Option<String>,
    conn: Connection,
) -> impl Fn(u64, u64) {
    let start = Instant::now();
    let ticks = AtomicU64::new(0);
    move |done: u64, total: u64| {
        let last = total > 0 && done >= total;
        // Emit ~every 16 chunks (a few MB) plus always the final tick.
        if ticks.fetch_add(1, Ordering::Relaxed) % 16 != 0 && !last {
            return;
        }
        let secs = start.elapsed().as_secs_f64().max(0.001);
        let mut u = TransferUpdate::new(id.clone(), dir, names.clone());
        u.state = TransferState::Transferring;
        u.bytes_done = done;
        u.bytes_total = total;
        u.percent = if total > 0 {
            (done as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        u.speed_bps = done as f64 / secs;
        u.locality = conn_locality(&conn); // live Direct/Relay badge
        u.friend_name = friend.clone();
        emit(&app, &u);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_completed(
    app: &AppHandle,
    id: &str,
    dir: Direction,
    names: Vec<String>,
    total: u64,
    locality: crate::models::Locality,
    friend: Option<String>,
    out_dir: Option<String>,
) {
    let mut u = TransferUpdate::new(id.to_string(), dir, names.clone());
    u.state = TransferState::Completed;
    u.bytes_done = total;
    u.bytes_total = total;
    u.percent = 100.0;
    u.locality = locality;
    u.friend_name = friend.clone();
    u.out_dir = out_dir.clone();
    emit(app, &u);

    // Persist to History so iroh transfers survive a restart. Folder receives log
    // via note_received; this covers Quick Send + friend sends/receives.
    if let Some(st) = app.try_state::<std::sync::Arc<crate::AppState>>() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        crate::history::append(
            &st.config_dir,
            crate::models::HistoryEntry {
                id: id.to_string(),
                direction: dir,
                file_names: names,
                bytes_total: total,
                peer: friend,
                locality,
                code: None,
                state: TransferState::Completed,
                timestamp_ms: ts,
                error: None,
                out_dir,
            },
        );
        let _ = app.emit("history://changed", ());
    }
}

fn emit_failed(app: &AppHandle, id: &str, dir: Direction, err: &str) {
    let mut u = TransferUpdate::new(id.to_string(), dir, Vec::new());
    u.state = TransferState::Failed;
    u.error = Some(err.to_string());
    emit(app, &u);
}

fn emit_canceled(app: &AppHandle, id: &str, dir: Direction) {
    let mut u = TransferUpdate::new(id.to_string(), dir, Vec::new());
    u.state = TransferState::Canceled;
    emit(app, &u);
}

/// Tell the UI a staged (not-yet-pulled) send was canceled.
pub fn emit_canceled_send(app: &AppHandle, id: &str) {
    emit_canceled(app, id, Direction::Send);
}

/// What `IrohState::cancel` found for an id.
pub enum CancelKind {
    /// A still-staged send (removed; the UI should show Canceled).
    Staged,
    /// An in-flight transfer (its loop will report Canceled itself).
    Active,
    /// Not an iroh transfer (caller falls back to the croc path).
    Unknown,
}

/// Is this connection going DIRECT (peer-to-peer, fast) or via the RELAY (slow
/// fallback)? Surfaced as the Local/Internet badge so a slow transfer is
/// explainable. Cheap + synchronous — reads the currently selected path.
/// (The selected path can upgrade relay→direct mid-transfer, so we re-read it
/// on every progress tick for a live badge.)
fn conn_locality(conn: &Connection) -> crate::models::Locality {
    use crate::models::Locality;
    use iroh::Watcher as _; // brings `.get()` into scope for the path watcher
    let mut watcher = conn.paths();
    let paths = watcher.get();
    let relayed = paths
        .iter()
        .find(|p| p.is_selected())
        .map(|p| p.is_relay());
    match relayed {
        Some(true) => Locality::Internet, // relayed = the slow path
        Some(false) => Locality::Local,   // direct peer-to-peer
        None => Locality::Unknown,
    }
}

/// One-line performance summary for a finished transfer, written to the always-on
/// log. Captures the signals that tell us whether a transfer ran optimally: actual
/// throughput, whether the connection was DIRECT (hole-punched) or RELAYED (the
/// slow internet fallback), round-trip latency, and the peer address. The user can
/// send a file then hand back the log so we can see exactly where the headroom is.
fn log_transfer_perf(
    conn: &Connection,
    tag: &str,
    direction: &str,
    bytes: u64,
    elapsed: std::time::Duration,
) {
    use iroh::Watcher as _;
    let secs = elapsed.as_secs_f64();
    let mb = bytes as f64 / 1_000_000.0;
    let mbps = if secs > 0.0 { mb / secs } else { 0.0 };
    let mut watcher = conn.paths();
    let paths = watcher.get();
    let selected = paths.iter().find(|p| p.is_selected());
    let (path_kind, rtt, addr) = match selected {
        Some(p) => (
            if p.is_relay() { "RELAY/internet" } else { "DIRECT/p2p" },
            p.rtt()
                .map(|r| format!("{}ms", r.as_millis()))
                .unwrap_or_else(|| "?".into()),
            format!("{:?}", p.remote_addr()),
        ),
        None => ("unknown", "?".into(), "?".into()),
    };
    log::info!(
        "PERF[{tag}] {direction}: {mbps:.1} MB/s ({mb:.1} MB in {secs:.2}s) · {path_kind} · rtt={rtt} · peer={addr}"
    );
}

/// Load the device's secret key from disk, or generate + persist a fresh one.
/// The 32-byte ed25519 seed IS the device identity — its public half is the
/// `EndpointId` peers dial, so it must be stable across restarts.
fn load_or_create_secret(config_dir: &Path) -> SecretKey {
    let path = config_dir.join("iroh-identity.key");
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return SecretKey::from_bytes(&seed);
        }
    }
    let seed: [u8; 32] = rand::random();
    let sk = SecretKey::from_bytes(&seed);
    if let Err(e) = write_private(&path, &seed) {
        log::warn!("could not persist iroh identity: {e}");
    }
    sk
}

/// Write the secret seed with owner-only permissions where the OS supports it.
fn write_private(path: &Path, seed: &[u8; 32]) -> std::io::Result<()> {
    std::fs::write(path, seed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Build and bind the endpoint with our persistent identity. Uses iroh's default
/// (n0) relays + discovery for now; Phase 5 swaps in self-hosted infrastructure.
pub async fn start(config_dir: &Path) -> Result<Endpoint> {
    let secret = load_or_create_secret(config_dir);
    // Throughput tuning. BBR congestion control replaces quinn's CUBIC default:
    // iroh's own benchmark (n0-computer/iroh#4286) shows single-stream throughput
    // up to ~30x higher with BBR, and it removes the "fill the 32 MB window, stall,
    // repeat" pattern that makes big transfers start fast then crawl. The larger
    // flow-control windows (32/64 MB) let BBR fill the link on higher-latency
    // paths. (noq-proto is iroh's quinn fork — pinned to match iroh.)
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.congestion_controller_factory(std::sync::Arc::new(
        noq_proto::congestion::BbrConfig::default(),
    ));
    tcfg = tcfg.stream_receive_window((32u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(64 * 1024 * 1024);

    let ep = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .transport_config(tcfg.build())
        .bind()
        .await
        .context("bind iroh endpoint")?;

    // Local-network (mDNS) discovery: lets two machines on the same Wi-Fi/LAN
    // find each other's local addresses and connect DIRECTLY, instead of bouncing
    // through the relay. Best-effort — a failure here just means we fall back to
    // the default relay+DNS discovery.
    {
        use iroh::address_lookup::MdnsAddressLookup;
        match MdnsAddressLookup::builder().build(ep.id()) {
            Ok(mdns) => match ep.address_lookup() {
                Ok(al) => {
                    al.add(mdns);
                    log::info!("iroh: local-network (mDNS) discovery enabled");
                }
                Err(e) => log::warn!("iroh: address_lookup() unavailable: {e}"),
            },
            Err(e) => log::warn!("iroh: mDNS discovery unavailable: {e}"),
        }
    }
    Ok(ep)
}

/// Accept incoming connections forever, dispatching each to the protocol handler.
/// Runs for the life of the app. Errors on a single connection are logged, never
/// fatal.
pub async fn accept_loop(ep: Endpoint, state: Arc<IrohState>) {
    while let Some(incoming) = ep.accept().await {
        let st = state.clone();
        tauri::async_runtime::spawn(async move {
            match incoming.await {
                Ok(conn) => handle_conn(conn, st).await,
                Err(e) => log::debug!("iroh: incoming connection failed: {e}"),
            }
        });
    }
}

/// Per-connection handler: each incoming stream is dispatched by its first frame.
async fn handle_conn(conn: Connection, state: Arc<IrohState>) {
    let who = conn.remote_id();
    loop {
        match conn.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let st = state.clone();
                let c = conn.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = serve_stream(&c, &mut send, &mut recv, &st).await {
                        log::debug!("iroh stream error: {e:#}");
                    }
                });
            }
            Err(_) => {
                log::debug!("iroh: connection from {who} closed");
                break;
            }
        }
    }
}

/// Handle one incoming stream by its header `kind`:
///   "ping" → reply "pong" (self-test); "pull" → serve a staged Quick Send.
async fn serve_stream(
    conn: &Connection,
    send: &mut SendStream,
    recv: &mut RecvStream,
    state: &IrohState,
) -> Result<()> {
    let req = read_frame(recv).await?;
    match req.get("kind").and_then(|k| k.as_str()) {
        Some("ping") => {
            write_frame(send, &serde_json::json!({ "kind": "pong" })).await?;
            send.finish()?;
        }
        Some("pull") => {
            let token = req.get("token").and_then(|t| t.as_str()).unwrap_or("");
            let pending = state.pending.lock().unwrap().remove(token);
            let p = pending.ok_or_else(|| anyhow::anyhow!("no pending send for token"))?;
            match state.app.get().cloned() {
                Some(app) => {
                    let cb = progress_cb(
                        app.clone(),
                        p.transfer_id.clone(),
                        Direction::Send,
                        p.names.clone(),
                        None,
                        conn.clone(),
                    );
                    let __t0 = std::time::Instant::now();
                    let __total = p.total;
                    match serve_pull(send, &p.paths, &p.cancel, cb).await {
                        Ok(_) => {
                            log_transfer_perf(conn, "quick-send", "send", __total, __t0.elapsed());
                            emit_completed(
                                &app,
                                &p.transfer_id,
                                Direction::Send,
                                p.names,
                                p.total,
                                conn_locality(conn),
                                None,
                                None,
                            )
                        }
                        Err(e) if e.to_string().contains("canceled") => {
                            emit_canceled(&app, &p.transfer_id, Direction::Send)
                        }
                        Err(e) => emit_failed(&app, &p.transfer_id, Direction::Send, &e.to_string()),
                    }
                }
                None => {
                    serve_pull(send, &p.paths, &p.cancel, |_, _| {}).await?;
                }
            }
            state.cancels.lock().unwrap().remove(&p.transfer_id);
        }
        Some("files") => {
            // A friend pushed files straight to us. Receive into the download
            // folder and surface it like any other receive.
            let Some(app) = state.app.get().cloned() else {
                let never = AtomicBool::new(false);
                let _ = read_body(recv, &req, &std::env::temp_dir(), &never, |_, _| {}).await;
                return Ok(());
            };
            let (config_dir, configured) = app
                .try_state::<Arc<crate::AppState>>()
                .map(|st| {
                    let dl = st.settings.lock().unwrap().download_dir.clone();
                    (st.config_dir.clone(), dl)
                })
                .unwrap_or_else(|| (std::env::temp_dir(), String::new()));
            let dest = if configured.trim().is_empty() {
                app.path().download_dir().unwrap_or_else(|_| std::env::temp_dir())
            } else {
                PathBuf::from(configured)
            };
            // Identify the sender by matching their endpoint id to a friend.
            let who = conn.remote_id().to_string();
            let friend = crate::friends::load(&config_dir)
                .into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(who.as_str()));
            let sender = friend.as_ref().map(|f| f.name.clone());
            // A friend with manual-accept on must approve before we receive. An
            // unknown sender (no friend record) defaults to auto-accept, same as
            // the prior behavior.
            let auto_accept = friend.as_ref().map(|f| f.auto_accept).unwrap_or(true);
            let total = req["total"].as_u64().unwrap_or(0);
            let names: Vec<String> = req["items"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|it| it.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let id = uuid::Uuid::new_v4().to_string();
            let cancel = Arc::new(AtomicBool::new(false));
            state.cancels.lock().unwrap().insert(id.clone(), cancel.clone());

            // Manual accept/decline — the iroh twin of run_croc_receive_interactive.
            // Pause before reading the body, surface the offer, and wait for the
            // user's decision (resolved by the respond_to_offer command). On
            // decline we drop the streams, which stops the sender's push.
            if !auto_accept {
                let mut wu = TransferUpdate::new(id.clone(), Direction::Receive, names.clone());
                wu.state = TransferState::WaitingForAccept;
                wu.friend_name = sender.clone();
                wu.bytes_total = total;
                emit(&app, &wu);
                let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                if let Some(st) = app.try_state::<Arc<crate::AppState>>() {
                    st.offers.lock().unwrap().insert(id.clone(), tx);
                }
                // Don't park forever: resolve on the user's choice, on the sender
                // giving up (connection closed), or after a TTL — so a declined or
                // ignored offer always cleans up instead of leaking the task and
                // leaving the sender blocked.
                let accept = tokio::select! {
                    r = rx => r.unwrap_or(false),
                    _ = conn.closed() => false,
                    _ = tokio::time::sleep(Duration::from_secs(300)) => false,
                };
                if let Some(st) = app.try_state::<Arc<crate::AppState>>() {
                    st.offers.lock().unwrap().remove(&id);
                }
                if !accept {
                    emit_canceled(&app, &id, Direction::Receive);
                    state.cancels.lock().unwrap().remove(&id);
                    return Ok(());
                }
            }

            let mut u0 = TransferUpdate::new(id.clone(), Direction::Receive, names.clone());
            u0.state = TransferState::Transferring;
            u0.friend_name = sender.clone();
            u0.bytes_total = total;
            emit(&app, &u0);
            let cb = progress_cb(
                app.clone(),
                id.clone(),
                Direction::Receive,
                names.clone(),
                sender.clone(),
                conn.clone(),
            );
            let __t0 = std::time::Instant::now();
            match read_body(recv, &req, &dest, &cancel, cb).await {
                Ok(paths) => {
                    log_transfer_perf(conn, "friend-recv", "recv", total, __t0.elapsed());
                    let names: Vec<String> = paths
                        .iter()
                        .map(|p| {
                            p.file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .collect();
                    emit_completed(
                        &app,
                        &id,
                        Direction::Receive,
                        names,
                        total,
                        conn_locality(conn),
                        sender,
                        Some(dest.to_string_lossy().to_string()),
                    );
                    let _ = send.write_all(b"ok").await;
                    let _ = send.finish();
                }
                Err(e) if e.to_string().contains("canceled") => {
                    emit_canceled(&app, &id, Direction::Receive)
                }
                Err(e) => emit_failed(&app, &id, Direction::Receive, &e.to_string()),
            }
            state.cancels.lock().unwrap().remove(&id);
        }
        Some("friend-hello") => {
            // The accepter is telling us their iroh id for our shared friend
            // record, so we can send to them directly later.
            let friend_id = req.get("friend_id").and_then(|v| v.as_str()).unwrap_or("");
            let their_id = req.get("endpoint_id").and_then(|v| v.as_str()).unwrap_or("");
            if !friend_id.is_empty() && !their_id.is_empty() {
                if let Some(app) = state.app.get() {
                    if let Some(st) = app.try_state::<Arc<crate::AppState>>() {
                        if crate::friends::set_endpoint_id(
                            &st.config_dir,
                            friend_id,
                            their_id.to_string(),
                        ) {
                            let _ = app.emit("pairs://changed", ());
                        }
                    }
                }
            }
            write_frame(send, &serde_json::json!({ "kind": "ok" })).await?;
            send.finish()?;
        }
        Some("folder-hello") => {
            // A folder accepter is handing us their iroh id for our shared pair,
            // so we (the creator) can also push this folder directly over iroh.
            let pair_id = req.get("pair_id").and_then(|v| v.as_str()).unwrap_or("");
            let their_id = req.get("endpoint_id").and_then(|v| v.as_str()).unwrap_or("");
            if !pair_id.is_empty() && !their_id.is_empty() {
                if let Some(app) = state.app.get() {
                    if let Some(st) = app.try_state::<Arc<crate::AppState>>() {
                        if crate::pairing::set_endpoint_id(
                            &st.config_dir,
                            pair_id,
                            their_id.to_string(),
                        ) {
                            // Refresh the running folder workers so the sender
                            // picks up the new key and the UI updates.
                            if let Some(sm) = app.try_state::<Arc<crate::sync::SyncManager>>() {
                                sm.inner().clone().reconcile();
                            }
                            let _ = app.emit("pairs://changed", ());
                        }
                    }
                }
            }
            write_frame(send, &serde_json::json!({ "kind": "ok" })).await?;
            send.finish()?;
        }
        Some("folder-files") => {
            // The folder peer pushed files straight to us. Receive into a private
            // staging dir, then hand them to the SyncManager to land in the shared
            // folder with the exact same loop-protection / mirror / history rules
            // the croc receive path uses.
            let pair_id = req
                .get("pair_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let Some(app) = state.app.get().cloned() else {
                let never = AtomicBool::new(false);
                let _ = read_folder_body(recv, &req, &std::env::temp_dir(), &never, |_, _| {}).await;
                return Ok(());
            };
            let sm = app
                .try_state::<Arc<crate::sync::SyncManager>>()
                .map(|s| s.inner().clone());
            let config_dir = app
                .try_state::<Arc<crate::AppState>>()
                .map(|st| st.config_dir.clone())
                .unwrap_or_else(std::env::temp_dir);
            let Some(sm) = sm else {
                anyhow::bail!("sync manager not ready");
            };
            if !sm.has_pair(&pair_id) {
                anyhow::bail!("push for unknown folder pair {pair_id}");
            }
            let staging = config_dir.join(format!(".iroh-folder-{pair_id}"));
            let _ = std::fs::remove_dir_all(&staging);
            let total = req["total"].as_u64().unwrap_or(0);
            let cancel = AtomicBool::new(false);
            let loc = conn_locality(conn);
            let last = AtomicU64::new(0);
            let cb = |done: u64, _t: u64| {
                // Throttle to ~1% steps so the folder status bar updates smoothly
                // without flooding the UI with events.
                let permille = if total > 0 { done * 1000 / total } else { 0 };
                if done < total && permille <= last.load(Ordering::Relaxed) {
                    return;
                }
                last.store(permille, Ordering::Relaxed);
                sm.note_folder_receiving(&pair_id, done, total, loc);
            };
            let __t0 = std::time::Instant::now();
            let result = read_folder_body(recv, &req, &staging, &cancel, cb).await;
            match result {
                Ok(_) => {
                    log_transfer_perf(conn, "folder-recv", "recv", total, __t0.elapsed());
                    sm.ingest_iroh_folder_files(&pair_id, &staging);
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = send.write_all(b"ok").await;
                    let _ = send.finish();
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    anyhow::bail!("folder receive failed: {e}");
                }
            }
        }
        Some("folder-ctrl") => {
            // The folder peer's presence + control beacon, over iroh. Carries their
            // display name and (mirror mode) pending deletes — the same payload the
            // croc control channel used to deliver. Hand it to the SyncManager to
            // apply (learn name, link friend, propagate deletes) and ack so the
            // sender knows we're online and received it.
            let pair_id = req
                .get("pair_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = req
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let deletes: Vec<(String, u64)> = req
                .get("deletes")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|d| {
                            let rel = d.get("rel").and_then(|r| r.as_str())?.to_string();
                            let ts = d.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
                            Some((rel, ts))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !pair_id.is_empty() {
                if let Some(app) = state.app.get() {
                    if let Some(sm) = app.try_state::<Arc<crate::sync::SyncManager>>() {
                        let sm = sm.inner().clone();
                        sm.apply_remote_control(&pair_id, &name, &deletes);
                    }
                }
            }
            write_frame(send, &serde_json::json!({ "kind": "ok" })).await?;
            send.finish()?;
        }
        Some("chat") => {
            // A friend sent us a chat message. Identify them by their endpoint
            // id, persist it to the conversation, and surface it live to the UI.
            if let Some(app) = state.app.get().cloned() {
                let config_dir = app
                    .try_state::<Arc<crate::AppState>>()
                    .map(|st| st.config_dir.clone())
                    .unwrap_or_else(std::env::temp_dir);
                let who = conn.remote_id().to_string();
                // Identify the sender by their endpoint id; fall back to the
                // friend id they carry in the frame (invite friends share an id
                // on both sides) so chat still lands if we haven't learned their
                // key yet. Either way, remember the key for future sends.
                let friends = crate::friends::load(&config_dir);
                let claimed = req.get("friendId").and_then(|v| v.as_str());
                let friend = friends
                    .iter()
                    .find(|f| f.endpoint_id.as_deref() == Some(who.as_str()))
                    .or_else(|| claimed.and_then(|id| friends.iter().find(|f| f.id == id)))
                    .cloned();
                if let Some(friend) = friend {
                    if friend.endpoint_id.as_deref() != Some(who.as_str()) {
                        crate::friends::set_endpoint_id(&config_dir, &friend.id, who.clone());
                    }
                    let msg_kind = req
                        .get("msgKind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("text")
                        .to_string();
                    let text = req
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let files: Vec<String> = req
                        .get("files")
                        .and_then(|f| f.as_array())
                        .map(|arr| {
                            arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                        })
                        .unwrap_or_default();
                    let bytes = req.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
                    let id = req
                        .get("id")
                        .and_then(|i| i.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let ts = req
                        .get("ts")
                        .and_then(|t| t.as_u64())
                        .unwrap_or_else(crate::chat::now_ms);
                    // Best-effort local path so the receiver can preview/open the
                    // file once it lands: a friend's files save into the download
                    // dir under the same name (collision-renames are the rare miss).
                    let path = if msg_kind == "file" {
                        files.first().map(|name| {
                            let configured = app
                                .try_state::<Arc<crate::AppState>>()
                                .map(|st| st.settings.lock().unwrap().download_dir.clone())
                                .unwrap_or_default();
                            let dir = if configured.trim().is_empty() {
                                app.path().download_dir().unwrap_or_else(|_| std::env::temp_dir())
                            } else {
                                PathBuf::from(configured)
                            };
                            dir.join(name).to_string_lossy().to_string()
                        })
                    } else {
                        None
                    };
                    let msg = crate::chat::ChatMessage {
                        id,
                        peer_id: friend.id.clone(),
                        from_me: false,
                        kind: msg_kind,
                        text,
                        files,
                        bytes,
                        path,
                        ts,
                    };
                    if crate::chat::append(&config_dir, &msg) {
                        let _ = app.emit("chat://message", &msg);
                    }
                }
            }
            write_frame(send, &serde_json::json!({ "kind": "ok" })).await?;
            send.finish()?;
        }
        other => anyhow::bail!("unknown stream kind: {other:?}"),
    }
    Ok(())
}

/// Prove iroh works inside the running app: spin up a throwaway client endpoint,
/// dial our own node, round-trip a ping over a real QUIC stream. Returns a short
/// human summary (shown in Settings / logs).
pub async fn self_test(main: &Endpoint) -> Result<String> {
    main.online().await;
    let addr = main.addr();
    let client = Endpoint::bind(presets::N0)
        .await
        .context("bind self-test client")?;
    let conn = client.connect(addr, ALPN).await.context("dial self")?;
    let (mut send, mut recv) = conn.open_bi().await.context("open_bi")?;
    write_frame(&mut send, &serde_json::json!({ "kind": "ping" })).await?;
    send.finish()?;
    let resp = read_frame(&mut recv).await.context("read pong")?;
    let ok = resp.get("kind").and_then(|k| k.as_str()) == Some("pong");
    let id = main.id().to_string();
    client.close().await;
    if ok {
        Ok(format!("ok · node {}…{}", &id[..6], &id[id.len() - 4..]))
    } else {
        anyhow::bail!("unexpected self-test response: {resp}")
    }
}

/// Spawn the endpoint at app startup and keep accepting connections. Fills
/// `state.endpoint` once bound. Safe to fail — croc remains the live transport.
pub fn spawn(config_dir: std::path::PathBuf, state: Arc<IrohState>, app: AppHandle) {
    let _ = state.app.set(app);
    tauri::async_runtime::spawn(async move {
        match start(&config_dir).await {
            Ok(ep) => {
                log::info!(
                    "iroh endpoint up — BBR congestion control, 32/64MB windows — {}",
                    ep.id()
                );
                let _ = state.endpoint.set(ep.clone());
                accept_loop(ep, state).await;
            }
            Err(e) => log::warn!("iroh endpoint failed to start: {e:#}"),
        }
    });
}

/// Stage a Quick Send: register the files under a one-time token and return a
/// ticket-bearing update (state WaitingForPeer, `code` = ticket). The accept loop
/// serves the pull and emits the rest of the lifecycle for this transfer id.
pub fn start_send(
    app: AppHandle,
    state: Arc<IrohState>,
    paths: Vec<String>,
) -> Result<TransferUpdate, String> {
    let ep = state
        .get()
        .cloned()
        .ok_or("Direct mode is still starting up — try again in a moment.")?;
    let pathbufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let names: Vec<String> = pathbufs
        .iter()
        .map(|p| {
            p.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect();
    let total: u64 = pathbufs
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    let id = uuid::Uuid::new_v4().to_string();
    let token = uuid::Uuid::new_v4().to_string();
    let ticket = make_ticket(&ep, &token).map_err(|e| e.to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .cancels
        .lock()
        .unwrap()
        .insert(id.clone(), cancel.clone());
    state.pending.lock().unwrap().insert(
        token,
        PendingSend {
            transfer_id: id.clone(),
            paths: pathbufs,
            names: names.clone(),
            total,
            cancel,
        },
    );
    let mut update = TransferUpdate::new(id, Direction::Send, names);
    update.state = TransferState::WaitingForPeer;
    update.code = Some(ticket);
    update.bytes_total = total;
    emit(&app, &update);
    Ok(update)
}

/// Start an iroh receive: dial the ticket and pull files into `out_dir`, emitting
/// progress/complete/fail for the new transfer id.
pub fn start_receive(
    app: AppHandle,
    state: Arc<IrohState>,
    ticket: String,
    out_dir: String,
) -> Result<TransferUpdate, String> {
    let ep = state
        .get()
        .cloned()
        .ok_or("Direct mode is still starting up — try again in a moment.")?;
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .cancels
        .lock()
        .unwrap()
        .insert(id.clone(), cancel.clone());
    let cleanup = state.clone();
    let mut update = TransferUpdate::new(id.clone(), Direction::Receive, Vec::new());
    update.state = TransferState::Connecting;
    update.out_dir = Some(out_dir.clone());
    emit(&app, &update);
    let snapshot = update.clone();
    tauri::async_runtime::spawn(async move {
        let dest = PathBuf::from(&out_dir);
        // Inline the dial+pull so we own the Connection — that lets the progress
        // ticks read the live Direct/Relay path for the badge.
        let outcome: Result<(Vec<PathBuf>, crate::models::Locality)> = async {
            let (addr, token) = parse_ticket(&ticket)?;
            let conn = ep.connect(addr, ALPN).await.context("dial ticket")?;
            let (mut send, mut recv) = conn.open_bi().await?;
            write_frame(&mut send, &serde_json::json!({ "kind": "pull", "token": token })).await?;
            let cb = progress_cb(
                app.clone(),
                id.clone(),
                Direction::Receive,
                Vec::new(),
                None,
                conn.clone(),
            );
            let __t0 = std::time::Instant::now();
            let paths = read_files(&mut recv, &dest, &cancel, cb).await?;
            let loc = conn_locality(&conn);
            let __bytes: u64 = paths
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
                .sum();
            log_transfer_perf(&conn, "quick-recv", "recv", __bytes, __t0.elapsed());
            Ok((paths, loc))
        }
        .await;
        match outcome {
            Ok((paths, loc)) => {
                let names: Vec<String> = paths
                    .iter()
                    .map(|p| {
                        p.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    })
                    .collect();
                let total: u64 = paths
                    .iter()
                    .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
                    .sum();
                emit_completed(&app, &id, Direction::Receive, names, total, loc, None, Some(out_dir));
            }
            Err(e) if e.to_string().contains("canceled") => {
                emit_canceled(&app, &id, Direction::Receive)
            }
            Err(e) => emit_failed(&app, &id, Direction::Receive, &e.to_string()),
        }
        cleanup.cancels.lock().unwrap().remove(&id);
    });
    Ok(snapshot)
}

/// Send files straight to a friend over iroh — dial their EndpointId (discovery
/// resolves their current address), then push. The friend's accept loop receives
/// it. This is the direct, no-code "send by name" path.
pub fn send_to_friend(
    app: AppHandle,
    state: Arc<IrohState>,
    friend_name: String,
    endpoint_id: String,
    paths: Vec<String>,
) -> Result<TransferUpdate, String> {
    let ep = state
        .get()
        .cloned()
        .ok_or("Direct mode is still starting up — try again in a moment.")?;
    let parsed: iroh::EndpointId = endpoint_id
        .parse()
        .map_err(|_| "This friend's direct address is invalid.".to_string())?;
    let addr = iroh::EndpointAddr::from(parsed);
    let pathbufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let names: Vec<String> = pathbufs
        .iter()
        .map(|p| {
            p.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect();
    let total: u64 = pathbufs
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(id.clone(), cancel.clone());
    let cleanup = state.clone();
    let mut update = TransferUpdate::new(id.clone(), Direction::Send, names.clone());
    update.friend_name = Some(friend_name.clone());
    update.state = TransferState::Connecting;
    update.bytes_total = total;
    emit(&app, &update);
    let snapshot = update.clone();
    tauri::async_runtime::spawn(async move {
        let outcome: Result<crate::models::Locality> = async {
            // Bounded so an offline/unreachable friend FAILS clearly instead of
            // hanging forever (the exact thing that was wrong with the old path).
            // Bounded re-dial: a friend who's briefly offline (e.g. just opening
            // their app) still receives, mirroring croc's old parked-send window.
            // We keep re-dialing until connected or the budget runs out; the file
            // stays "Connecting" meanwhile, and a cancel breaks out immediately.
            let conn = {
                let started = std::time::Instant::now();
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        anyhow::bail!("canceled");
                    }
                    match tokio::time::timeout(
                        Duration::from_secs(20),
                        ep.connect(addr.clone(), ALPN),
                    )
                    .await
                    {
                        Ok(Ok(c)) => break c,
                        _ => {
                            if started.elapsed() > Duration::from_secs(FRIEND_SEND_RETRY_SECS) {
                                anyhow::bail!("Couldn't reach this friend — are they online with Direct mode on?");
                            }
                            tokio::time::sleep(Duration::from_secs(6)).await;
                        }
                    }
                }
            };
            let cb = progress_cb(
                app.clone(),
                id.clone(),
                Direction::Send,
                names.clone(),
                Some(friend_name.clone()),
                conn.clone(),
            );
            let __t0 = std::time::Instant::now();
            send_files(&conn, &pathbufs, &cancel, cb).await?;
            log_transfer_perf(&conn, "friend-send", "send", total, __t0.elapsed());
            Ok(conn_locality(&conn))
        }
        .await;
        match outcome {
            Ok(loc) => emit_completed(
                &app,
                &id,
                Direction::Send,
                names,
                total,
                loc,
                Some(friend_name),
                None,
            ),
            Err(e) if e.to_string().contains("canceled") => {
                emit_canceled(&app, &id, Direction::Send)
            }
            Err(e) => emit_failed(&app, &id, Direction::Send, &e.to_string()),
        }
        cleanup.cancels.lock().unwrap().remove(&id);
    });
    Ok(snapshot)
}

/// After accepting a friend invite, dial the inviter (whose EndpointId is in the
/// invite) and tell them our id for the shared friend record — so the reverse
/// direction (them → us) also works. Best-effort, fire-and-forget.
pub fn say_hello(state: Arc<IrohState>, friend_id: String, inviter_endpoint_id: String, my_name: String) {
    let Some(ep) = state.get().cloned() else {
        return;
    };
    let Ok(parsed) = inviter_endpoint_id.parse::<iroh::EndpointId>() else {
        return;
    };
    let my_id = ep.id().to_string();
    let addr = iroh::EndpointAddr::from(parsed);
    tauri::async_runtime::spawn(async move {
        let hello = serde_json::json!({
            "kind": "friend-hello", "friend_id": friend_id, "endpoint_id": my_id, "name": my_name,
        });
        if let Ok(Ok(conn)) =
            tokio::time::timeout(Duration::from_secs(20), ep.connect(addr, ALPN)).await
        {
            if let Ok((mut send, mut recv)) = conn.open_bi().await {
                let _ = write_frame(&mut send, &hello).await;
                let _ = send.finish();
                let _ = recv.read_to_end(64).await; // wait for their ok
            }
        }
    });
}

/// Folder analogue of `say_hello`: after accepting a Shared Drop Folder invite,
/// dial the creator (whose EndpointId rode the invite) and hand them our id for
/// this pair — so the creator → us direction also pushes directly over iroh.
pub fn say_hello_folder(
    state: Arc<IrohState>,
    pair_id: String,
    inviter_endpoint_id: String,
    my_name: String,
) {
    let Some(ep) = state.get().cloned() else {
        return;
    };
    let Ok(parsed) = inviter_endpoint_id.parse::<iroh::EndpointId>() else {
        return;
    };
    let my_id = ep.id().to_string();
    let addr = iroh::EndpointAddr::from(parsed);
    tauri::async_runtime::spawn(async move {
        let hello = serde_json::json!({
            "kind": "folder-hello", "pair_id": pair_id, "endpoint_id": my_id, "name": my_name,
        });
        if let Ok(Ok(conn)) =
            tokio::time::timeout(Duration::from_secs(20), ep.connect(addr, ALPN)).await
        {
            if let Ok((mut send, mut recv)) = conn.open_bi().await {
                let _ = write_frame(&mut send, &hello).await;
                let _ = send.finish();
                let _ = recv.read_to_end(64).await;
            }
        }
    });
}

/// Folder presence + control over iroh: dial the paired peer and hand them our
/// display name and any pending mirror deletes. This is the iroh replacement for
/// the croc control beacon — `Ok(())` means the peer received it (so they're
/// online), `Err` means "fall back to croc / mark offline" to the caller.
pub async fn send_folder_ctrl(
    ep: &Endpoint,
    endpoint_id: &str,
    pair_id: &str,
    name: &str,
    deletes: &[(String, u64)],
) -> Result<()> {
    let parsed: iroh::EndpointId = endpoint_id.parse().context("parse peer endpoint id")?;
    let addr = iroh::EndpointAddr::from(parsed);
    let dels: Vec<serde_json::Value> = deletes
        .iter()
        .map(|(rel, ts)| serde_json::json!({ "rel": rel, "ts": ts }))
        .collect();
    let msg = serde_json::json!({
        "kind": "folder-ctrl", "pair_id": pair_id, "name": name, "deletes": dels,
    });
    // Bounded dial so a perpetually-offline peer fails fast and the caller can
    // back off, exactly like the old croc control timeout.
    let conn = tokio::time::timeout(Duration::from_secs(12), ep.connect(addr, ALPN))
        .await
        .map_err(|_| anyhow::anyhow!("control dial timed out"))?
        .context("dial folder peer for control")?;
    let (mut send, mut recv) = conn.open_bi().await.context("open control stream")?;
    write_frame(&mut send, &msg).await?;
    send.finish()?;
    // Best-effort ack: success is connect+write+finish; the peer applies the
    // payload when it reads the finished stream. We wait briefly for the ok but
    // don't fail delivery if the ack is slow.
    let _ = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(64)).await;
    Ok(())
}

/// Deliver a chat message to a friend over iroh (dial-by-key). `payload` is the
/// full `{kind:"chat", ...}` frame. `Ok(())` means they received it; `Err` means
/// they were unreachable — the message is kept locally (no store-and-forward yet).
pub async fn send_chat(ep: &Endpoint, endpoint_id: &str, payload: serde_json::Value) -> Result<()> {
    let parsed: iroh::EndpointId = endpoint_id.parse().context("parse peer endpoint id")?;
    let addr = iroh::EndpointAddr::from(parsed);
    let conn = tokio::time::timeout(Duration::from_secs(12), ep.connect(addr, ALPN))
        .await
        .map_err(|_| anyhow::anyhow!("chat dial timed out"))?
        .context("dial friend for chat")?;
    let (mut send, mut recv) = conn.open_bi().await.context("open chat stream")?;
    write_frame(&mut send, &payload).await?;
    send.finish()?;
    // Best-effort ack; the peer applies the message when it reads the finished
    // stream, so we don't fail delivery just because the ok is slow.
    let _ = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(64)).await;
    Ok(())
}

/// Liveness check: dial a friend/peer's endpoint and round-trip a ping. Returns
/// true only if they answered with a pong (their app is running and reachable) —
/// the iroh replacement for the croc "ping_send" online check.
pub async fn ping_endpoint(ep: &Endpoint, endpoint_id: &str) -> bool {
    let Ok(parsed) = endpoint_id.parse::<iroh::EndpointId>() else {
        return false;
    };
    let addr = iroh::EndpointAddr::from(parsed);
    let fut = async {
        let conn = ep.connect(addr, ALPN).await.ok()?;
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        write_frame(&mut send, &serde_json::json!({ "kind": "ping" }))
            .await
            .ok()?;
        send.finish().ok()?;
        let reply = read_frame(&mut recv).await.ok()?;
        Some(reply.get("kind").and_then(|k| k.as_str()) == Some("pong"))
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(15), fut).await,
        Ok(Some(true))
    )
}

/// Push folder files to a paired peer over iroh (dial-by-key). Preserves each
/// file's path relative to `root` so subfolders survive. Returns the connection
/// locality on success. Any `Err` means "fall back to croc" to the caller — so a
/// peer on an old build or one that's offline never breaks folder sync.
pub async fn send_folder_file<F: Fn(u64, u64)>(
    ep: &Endpoint,
    endpoint_id: &str,
    pair_id: &str,
    root: &str,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<crate::models::Locality> {
    let parsed: iroh::EndpointId = endpoint_id
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid folder peer endpoint id"))?;
    let addr = iroh::EndpointAddr::from(parsed);
    let conn = tokio::time::timeout(Duration::from_secs(12), ep.connect(addr, ALPN))
        .await
        .map_err(|_| anyhow::anyhow!("folder peer unreachable over iroh"))?
        .context("dial folder peer")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let __t0 = std::time::Instant::now();
    write_folder_files(&mut send, pair_id, root, paths, cancel, on_progress).await?;
    send.finish()?;
    // Require the receiver's "ok" so "delivered" means the bytes actually landed
    // in their folder (not just that we finished writing to the socket).
    let ack = recv.read_to_end(16).await.unwrap_or_default();
    anyhow::ensure!(ack == b"ok", "folder peer did not confirm receipt");
    let __bytes: u64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    log_transfer_perf(&conn, "folder-send", "send", __bytes, __t0.elapsed());
    Ok(conn_locality(&conn))
}

/// Folder PUSH writer: like `write_files`, but tags the stream with `pair_id` and
/// sends each file's path RELATIVE to the folder root (so `sub/a.txt` lands in
/// `sub/a.txt`, not the folder root).
async fn write_folder_files<F: Fn(u64, u64)>(
    send: &mut SendStream,
    pair_id: &str,
    root: &str,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let mut items = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
        items.push((p.clone(), folder_rel(p, root), meta.len()));
    }
    let total: u64 = items.iter().map(|i| i.2).sum();
    let header = serde_json::json!({
        "kind": "folder-files",
        "pair_id": pair_id,
        "items": items
            .iter()
            .map(|(_, n, s)| serde_json::json!({ "name": n, "size": s }))
            .collect::<Vec<_>>(),
        "total": total,
    });
    write_frame(send, &header).await?;

    let mut sent = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (path, _, _) in &items {
        let mut f = tokio::fs::File::open(path).await?;
        loop {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("canceled");
            }
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            send.write_all(&buf[..n]).await?;
            sent += n as u64;
            on_progress(sent, total);
        }
    }
    Ok(sent)
}

/// Folder receive: like `read_body`, but PRESERVES each item's relative subpath
/// (sanitized — never an absolute path or a `..` escape) so subfolders are
/// recreated under `dest_dir`.
async fn read_folder_body<F: Fn(u64, u64)>(
    recv: &mut RecvStream,
    header: &serde_json::Value,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(header["kind"] == "folder-files", "unexpected stream kind");
    let items = header["items"].as_array().cloned().unwrap_or_default();
    let total = header["total"].as_u64().unwrap_or(0);
    std::fs::create_dir_all(dest_dir)?;

    let mut got = 0u64;
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    for item in &items {
        let raw = item["name"].as_str().unwrap_or("file");
        let rel = sanitize_rel(raw);
        let size = item["size"].as_u64().unwrap_or(0);
        let dest = dest_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Buffer disk writes: QUIC delivers data in small pieces, and one blocking
        // write syscall per piece throttles big receives. A 1 MiB buffer batches
        // them into far fewer, larger writes. Flushed before the file is finalized.
        let mut f =
            tokio::io::BufWriter::with_capacity(1 << 20, tokio::fs::File::create(&dest).await?);
        let mut remaining = size;
        while remaining > 0 {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("canceled");
            }
            let want = remaining.min(buf.len() as u64) as usize;
            match recv.read(&mut buf[..want]).await? {
                Some(n) if n > 0 => {
                    f.write_all(&buf[..n]).await?;
                    remaining -= n as u64;
                    got += n as u64;
                    on_progress(got, total);
                }
                _ => anyhow::bail!("stream ended before {} finished", dest.display()),
            }
        }
        f.flush().await?;
        out.push(dest);
    }
    Ok(out)
}

/// A file's path relative to the folder root, forward-slashed; falls back to the
/// bare file name if it isn't under `root`.
fn folder_rel(path: &Path, root: &str) -> String {
    let norm = |r: &Path| r.to_string_lossy().replace('\\', "/");
    if let Ok(rel) = path.strip_prefix(root) {
        return norm(rel);
    }
    if let Ok(canon) = std::fs::canonicalize(root) {
        if let Ok(rel) = path.strip_prefix(&canon) {
            return norm(rel);
        }
    }
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string())
}

/// Sanitize a peer-supplied relative path: drop empties, `.` and `..`, and any
/// drive/root prefix, so a malicious peer can't write outside the staging dir.
fn sanitize_rel(raw: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in raw.replace('\\', "/").split('/') {
        if comp.is_empty() || comp == "." || comp == ".." || comp.contains(':') {
            continue;
        }
        out.push(comp);
    }
    if out.as_os_str().is_empty() {
        out.push("file");
    }
    out
}

// ── File-transfer protocol (raw QUIC bi-stream) ──────────────────────────────
//
// Wire format on one bidirectional stream:
//   [u32 BE len][JSON header]            header = {kind:"files", items:[{name,size}], total}
//   <raw bytes of file 1><file 2>…       concatenated in `items` order
// then the receiver replies "ok" on its half once everything is safely written.
// This is the single primitive Quick Send / Friends / Folders will all reuse.

const MAX_HEADER: usize = 1 << 20; // 1 MiB — generous for a file list, abuse-safe
const CHUNK: usize = 1024 * 1024; // 1 MiB — fewer read/write/await cycles on big files

async fn write_frame(send: &mut SendStream, v: &serde_json::Value) -> Result<()> {
    let buf = serde_json::to_vec(v)?;
    send.write_all(&(buf.len() as u32).to_be_bytes()).await?;
    send.write_all(&buf).await?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> Result<serde_json::Value> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let n = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(n <= MAX_HEADER, "header too large ({n} bytes)");
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

/// A collision-free destination path inside `dir` for an incoming `name`.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let dest = dir.join(name);
    if !dest.exists() {
        return dest;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..100_000 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(format!("{stem}-dup{ext}"))
}

/// Core: write `[header][file bytes…]` to an already-open send stream.
async fn write_files<F: Fn(u64, u64)>(
    send: &mut SendStream,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let mut items = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| anyhow::anyhow!("missing file name for {}", p.display()))?;
        items.push((p.clone(), name, meta.len()));
    }
    let total: u64 = items.iter().map(|i| i.2).sum();
    let header = serde_json::json!({
        "kind": "files",
        "items": items
            .iter()
            .map(|(_, n, s)| serde_json::json!({ "name": n, "size": s }))
            .collect::<Vec<_>>(),
        "total": total,
    });
    write_frame(send, &header).await?;

    let mut sent = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (path, _, _) in &items {
        let mut f = tokio::fs::File::open(path).await?;
        loop {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("canceled");
            }
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            send.write_all(&buf[..n]).await?;
            sent += n as u64;
            on_progress(sent, total);
        }
    }
    Ok(sent)
}

/// Core: read `[header][file bytes…]` from a recv stream into `dest_dir`.
async fn read_files<F: Fn(u64, u64)>(
    recv: &mut RecvStream,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let header = read_frame(recv).await?;
    read_body(recv, &header, dest_dir, cancel, on_progress).await
}

/// Read just the file BYTES (the header was already read — e.g. the accept loop
/// peeked it to dispatch a friend push).
async fn read_body<F: Fn(u64, u64)>(
    recv: &mut RecvStream,
    header: &serde_json::Value,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(header["kind"] == "files", "unexpected stream kind");
    let items = header["items"].as_array().cloned().unwrap_or_default();
    let total = header["total"].as_u64().unwrap_or(0);
    std::fs::create_dir_all(dest_dir)?;

    let mut got = 0u64;
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    for item in &items {
        // Take only the file name — never honor an absolute/`..` path from a peer.
        let raw = item["name"].as_str().unwrap_or("file");
        let name = Path::new(raw)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "file".to_string());
        let size = item["size"].as_u64().unwrap_or(0);
        let dest = unique_path(dest_dir, &name);
        // Buffer disk writes: QUIC delivers data in small pieces, and one blocking
        // write syscall per piece throttles big receives. A 1 MiB buffer batches
        // them into far fewer, larger writes. Flushed before the file is finalized.
        let mut f =
            tokio::io::BufWriter::with_capacity(1 << 20, tokio::fs::File::create(&dest).await?);
        let mut remaining = size;
        while remaining > 0 {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("canceled");
            }
            let want = remaining.min(buf.len() as u64) as usize;
            match recv.read(&mut buf[..want]).await? {
                Some(n) if n > 0 => {
                    f.write_all(&buf[..n]).await?;
                    remaining -= n as u64;
                    got += n as u64;
                    on_progress(got, total);
                }
                _ => anyhow::bail!("stream ended before {} finished", dest.display()),
            }
        }
        f.flush().await?;
        out.push(dest);
    }
    Ok(out)
}

/// PUSH: send files to an already-connected peer over a fresh stream (used by
/// Friends/Folders where we initiate). Waits for the receiver's ack.
pub async fn send_files<F: Fn(u64, u64)>(
    conn: &Connection,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let sent = write_files(&mut send, paths, cancel, on_progress).await?;
    send.finish()?;
    // Require the receiver's "ok" so we never report Completed for a transfer the
    // peer declined or failed to write (declined pushes stop the stream, surfacing
    // here as an error rather than a false success).
    let ack = recv.read_to_end(16).await.unwrap_or_default();
    anyhow::ensure!(
        ack == b"ok",
        "the transfer was interrupted before the recipient confirmed receipt"
    );
    Ok(sent)
}

/// PUSH receive: accept the peer's next stream and write its files to `dest_dir`.
/// (Used by tests + the upcoming folder sync.)
#[allow(dead_code)]
pub async fn recv_files<F: Fn(u64, u64)>(
    conn: &Connection,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let out = read_files(&mut recv, dest_dir, cancel, on_progress).await?;
    let _ = send.write_all(b"ok").await;
    let _ = send.finish();
    Ok(out)
}

// ── Quick Send (pull model) ──────────────────────────────────────────────────
//
// The sender publishes a *ticket* (its EndpointAddr + a one-time token). The
// receiver dials the ticket and asks for the files, the sender pushes them. This
// is the croc "share a code, they pull" flow, but direct + encrypted over iroh.

/// Encode a shareable ticket: where to reach us + a one-time token.
pub const TICKET_PREFIX: &str = "direct";

pub fn make_ticket(ep: &Endpoint, token: &str) -> Result<String> {
    use base64::Engine as _;
    let addr = ep.addr();
    let v = serde_json::json!({ "addr": addr, "token": token });
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&v)?);
    Ok(format!("{TICKET_PREFIX}{body}"))
}

fn parse_ticket(s: &str) -> Result<(iroh::EndpointAddr, String)> {
    use base64::Engine as _;
    let s = s.trim();
    let body = s.strip_prefix(TICKET_PREFIX).unwrap_or(s);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .context("ticket is not valid base64")?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).context("ticket is not valid")?;
    let addr: iroh::EndpointAddr =
        serde_json::from_value(v.get("addr").cloned().unwrap_or_default())
            .context("ticket has no address")?;
    let token = v
        .get("token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    Ok((addr, token))
}

/// Receiver side: dial a ticket, request its files, write them to `dest_dir`.
#[allow(dead_code)]
pub async fn pull_files<F: Fn(u64, u64)>(
    client: &Endpoint,
    ticket: &str,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let (addr, token) = parse_ticket(ticket)?;
    let conn = client.connect(addr, ALPN).await.context("dial ticket")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &serde_json::json!({ "kind": "pull", "token": token })).await?;
    let out = read_files(&mut recv, dest_dir, cancel, on_progress).await?;
    Ok(out)
}

/// Sender side: a pull request arrived on `send`/`recv`; push `paths`.
pub async fn serve_pull<F: Fn(u64, u64)>(
    send: &mut SendStream,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let sent = write_files(send, paths, cancel, on_progress).await?;
    send.finish()?;
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_across_loads() {
        let dir = std::env::temp_dir().join(format!("dropbeam-iroh-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = load_or_create_secret(&dir);
        let b = load_or_create_secret(&dir);
        // Same key the second time — identity persisted, not regenerated.
        assert_eq!(a.public(), b.public());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_rel_blocks_path_escapes() {
        use std::path::Path;
        // Subfolders are preserved…
        assert_eq!(sanitize_rel("sub/dir/a.txt"), Path::new("sub/dir/a.txt"));
        assert_eq!(sanitize_rel("a.txt"), Path::new("a.txt"));
        // …but traversal, absolute, and drive paths can never escape staging.
        assert_eq!(sanitize_rel("../../etc/passwd"), Path::new("etc/passwd"));
        assert_eq!(sanitize_rel("/etc/passwd"), Path::new("etc/passwd"));
        assert_eq!(sanitize_rel("..\\..\\Windows\\x"), Path::new("Windows/x"));
        assert_eq!(sanitize_rel("C:\\secret"), Path::new("secret"));
        // Degenerate input still yields a safe, non-empty name.
        assert_eq!(sanitize_rel(""), Path::new("file"));
        assert_eq!(sanitize_rel("../.."), Path::new("file"));
    }

    #[test]
    fn folder_rel_is_relative_and_forward_slashed() {
        use std::path::Path;
        assert_eq!(folder_rel(Path::new("/data/share/sub/a.txt"), "/data/share"), "sub/a.txt");
        // A path outside the root degrades to the bare file name (never absolute).
        assert_eq!(folder_rel(Path::new("/somewhere/else/b.txt"), "/data/share"), "b.txt");
    }

    // Real end-to-end file transfer over iroh. Ignored by default because it
    // touches iroh's relay/discovery network; run explicitly with:
    //   cargo test --lib iroh_net -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn transfers_a_file_over_iroh() {
        let pid = std::process::id();
        let server = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        server.online().await;
        let addr = server.addr();

        let dest = std::env::temp_dir().join(format!("dropbeam-iroh-rx-{pid}"));
        let dest_c = dest.clone();
        let srv = server.clone();
        let handle = tokio::spawn(async move {
            let incoming = srv.accept().await.unwrap();
            let conn = incoming.await.unwrap();
            recv_files(&conn, &dest_c, &AtomicBool::new(false), |_, _| {})
                .await
                .unwrap()
        });

        let src = std::env::temp_dir().join(format!("dropbeam-iroh-src-{pid}.bin"));
        let data = vec![0xABu8; 5 * 1024 * 1024];
        std::fs::write(&src, &data).unwrap();

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let conn = client.connect(addr, ALPN).await.unwrap();
        let sent = send_files(&conn, &[src.clone()], &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap();
        assert_eq!(sent, data.len() as u64);

        let received = handle.await.unwrap();
        assert_eq!(received.len(), 1);
        let got = std::fs::read(&received[0]).unwrap();
        assert_eq!(got.len(), data.len());
        assert_eq!(got, data, "received bytes must match what was sent");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest);
        println!("iroh file transfer OK: {} bytes verified", sent);
    }

    // Quick Send pull flow: sender publishes a ticket, receiver dials it and
    // pulls. Ignored (network); run with --ignored.
    #[tokio::test]
    #[ignore]
    async fn quick_send_pull_over_iroh() {
        let pid = std::process::id();
        let server = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        server.online().await;
        let token = "tok-abc-123".to_string();
        let ticket = make_ticket(&server, &token).unwrap();
        assert!(ticket.len() > 20, "ticket should be a real blob");

        let src = std::env::temp_dir().join(format!("dropbeam-pull-src-{pid}.bin"));
        let data = vec![0x5Au8; 4 * 1024 * 1024];
        std::fs::write(&src, &data).unwrap();
        let staged = vec![src.clone()];

        let srv = server.clone();
        let token_c = token.clone();
        let serve = tokio::spawn(async move {
            let incoming = srv.accept().await.unwrap();
            let conn = incoming.await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let req = read_frame(&mut recv).await.unwrap();
            assert_eq!(req["kind"], "pull");
            assert_eq!(req["token"], token_c);
            serve_pull(&mut send, &staged, &AtomicBool::new(false), |_, _| {})
                .await
                .unwrap();
        });

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let dest = std::env::temp_dir().join(format!("dropbeam-pull-rx-{pid}"));
        let got = pull_files(&client, &ticket, &dest, &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap();
        serve.await.unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), data);
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest);
        println!("iroh Quick Send (pull) OK: ticket {} chars", ticket.len());
    }

    // End-to-end through the REAL accept loop: register a pending send, run
    // accept_loop, and pull it like the live receive command would.
    #[tokio::test]
    #[ignore]
    async fn accept_loop_serves_a_quick_send() {
        let pid = std::process::id();
        let server = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        server.online().await;
        let state = Arc::new(IrohState::default());
        let _ = state.endpoint.set(server.clone());

        let src = std::env::temp_dir().join(format!("dropbeam-al-src-{pid}.bin"));
        let data = vec![0x33u8; 3 * 1024 * 1024];
        std::fs::write(&src, &data).unwrap();
        let token = "tok-accept-loop".to_string();
        state.pending.lock().unwrap().insert(
            token.clone(),
            PendingSend {
                transfer_id: "t1".into(),
                paths: vec![src.clone()],
                names: vec!["f.bin".into()],
                total: data.len() as u64,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        let ticket = make_ticket(&server, &token).unwrap();

        // Run the production accept loop in the background.
        let srv = server.clone();
        let st = state.clone();
        tokio::spawn(async move { accept_loop(srv, st).await });

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let dest = std::env::temp_dir().join(format!("dropbeam-al-rx-{pid}"));
        let got = pull_files(&client, &ticket, &dest, &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), data);
        // The pending entry was consumed by the serve.
        assert!(state.pending.lock().unwrap().is_empty());
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest);
        println!("iroh accept-loop Quick Send OK");
    }
}
