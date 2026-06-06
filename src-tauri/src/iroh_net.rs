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
use std::time::Instant;

use anyhow::{Context, Result};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, SecretKey};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::OnceCell;

use crate::models::{Direction, TransferState, TransferUpdate};

/// Application-layer protocol id. Bumped if the wire format changes.
pub const ALPN: &[u8] = b"dropbeam/1";

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
    out_dir: Option<String>,
) {
    let mut u = TransferUpdate::new(id.to_string(), dir, names);
    u.state = TransferState::Completed;
    u.bytes_done = total;
    u.bytes_total = total;
    u.percent = 100.0;
    u.locality = locality;
    u.out_dir = out_dir;
    emit(app, &u);
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
    // Raise the per-stream flow-control window well above the 1.25 MB default
    // (which caps throughput at ~100 Mbit/s on a 100ms link) so high-latency
    // internet transfers aren't window-limited. Doesn't help a lossy LAN link.
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.stream_receive_window((16u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(32 * 1024 * 1024);

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
                        conn.clone(),
                    );
                    match serve_pull(send, &p.paths, &p.cancel, cb).await {
                        Ok(_) => emit_completed(
                            &app,
                            &p.transfer_id,
                            Direction::Send,
                            p.names,
                            p.total,
                            conn_locality(conn),
                            None,
                        ),
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
                log::info!("iroh endpoint up: {}", ep.id());
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
                conn.clone(),
            );
            let paths = read_files(&mut recv, &dest, &cancel, cb).await?;
            let loc = conn_locality(&conn);
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
                emit_completed(&app, &id, Direction::Receive, names, total, loc, Some(out_dir));
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

// ── File-transfer protocol (raw QUIC bi-stream) ──────────────────────────────
//
// Wire format on one bidirectional stream:
//   [u32 BE len][JSON header]            header = {kind:"files", items:[{name,size}], total}
//   <raw bytes of file 1><file 2>…       concatenated in `items` order
// then the receiver replies "ok" on its half once everything is safely written.
// This is the single primitive Quick Send / Friends / Folders will all reuse.

const MAX_HEADER: usize = 1 << 20; // 1 MiB — generous for a file list, abuse-safe
const CHUNK: usize = 256 * 1024;

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
        let mut f = tokio::fs::File::create(&dest).await?;
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
    let _ = recv.read_to_end(16).await;
    Ok(sent)
}

/// PUSH receive: accept the peer's next stream and write its files to `dest_dir`.
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
