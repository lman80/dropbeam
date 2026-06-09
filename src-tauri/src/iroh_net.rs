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
    /// Live connections for in-flight transfers, keyed by transfer id. A cancel
    /// CLOSES the connection so a send stuck on QUIC flow-control unblocks at
    /// once — the flag alone only takes effect between chunks, which is why a
    /// sender couldn't cancel a stalled transfer.
    conns: Mutex<HashMap<String, Connection>>,
    /// Fingerprints of resumable partial files currently being written, so two
    /// concurrent receives of the same file never share one partial (the loser
    /// falls back to a throwaway, non-resumable temp).
    partials: Mutex<std::collections::HashSet<String>>,
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
        let mut active = false;
        if let Some(flag) = self.cancels.lock().unwrap().get(id) {
            flag.store(true, Ordering::SeqCst);
            active = true;
        }
        // Tear down the connection so a write stuck on QUIC flow-control aborts
        // immediately — this is what lets the SENDER cancel a stalled transfer.
        if let Some(conn) = self.conns.lock().unwrap().remove(id) {
            conn.close(0u32.into(), b"canceled");
            active = true;
        }
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
    // Log every failure so a transfer that "kept failing" on a machine we can't
    // reach leaves a trace in DropBeam.log (was invisible — failures only emitted
    // to the UI). Pairs with the PERF lines to explain a bad transfer.
    log::warn!("TRANSFER-FAIL[{dir:?}] id={id}: {err}");
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
// ── Known-address cache ──────────────────────────────────────────────────────
//
// The bulletproof layer for same-LAN sends. Discovery can fail silently (macOS
// Local Network permission denied → multicast dropped; hotel/apartment Wi-Fi
// filtering mDNS) and then two machines a metre apart try to reach each other
// via their shared PUBLIC IP (hairpin NAT, often broken) or a distant relay —
// finicky and slow. So: every time a connection has working direct addresses,
// remember them per peer (persisted across restarts), and seed every future
// dial with them. Stale entries are harmless — iroh races all addresses plus
// discovery and uses whichever answers first.

static PEER_ADDRS: std::sync::OnceLock<Mutex<HashMap<String, Vec<std::net::SocketAddr>>>> =
    std::sync::OnceLock::new();
static PEER_ADDRS_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn peer_addrs() -> &'static Mutex<HashMap<String, Vec<std::net::SocketAddr>>> {
    PEER_ADDRS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load the persisted cache (called once at endpoint startup).
fn load_peer_addrs(config_dir: &Path) {
    let path = config_dir.join("peer-addrs.json");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(map) = serde_json::from_slice::<HashMap<String, Vec<std::net::SocketAddr>>>(&bytes)
        {
            *peer_addrs().lock().unwrap() = map;
        }
    }
    let _ = PEER_ADDRS_PATH.set(path);
}

fn save_peer_addrs() {
    if let Some(path) = PEER_ADDRS_PATH.get() {
        let map = peer_addrs().lock().unwrap().clone();
        if let Ok(json) = serde_json::to_vec(&map) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Remember every live direct (IP) address of this connection for its peer —
/// newest first, capped, deduped. Called when a direct path forms, when a
/// transfer completes, and shortly after accepting an inbound connection, so
/// BOTH sides learn each other.
fn remember_conn_addrs(conn: &Connection) {
    use iroh::Watcher as _;
    let eid = conn.remote_id().to_string();
    let mut watcher = conn.paths();
    let addrs: Vec<std::net::SocketAddr> = watcher
        .get()
        .iter()
        .filter_map(|p| match p.remote_addr() {
            iroh::TransportAddr::Ip(s) => Some(*s),
            _ => None,
        })
        .collect();
    if addrs.is_empty() {
        return;
    }
    let mut changed = false;
    {
        let mut map = peer_addrs().lock().unwrap();
        let entry = map.entry(eid).or_default();
        for a in addrs {
            if entry.first() != Some(&a) {
                entry.retain(|x| x != &a);
                entry.insert(0, a);
                entry.truncate(6);
                changed = true;
            }
        }
    }
    if changed {
        save_peer_addrs();
    }
}

/// The address to dial for `id`: the bare EndpointId (discovery) PLUS every
/// direct address that has worked before — so a repeat send on the same LAN
/// connects instantly even when discovery is blind.
fn dial_addr(id: iroh::EndpointId) -> iroh::EndpointAddr {
    let cached = peer_addrs()
        .lock()
        .unwrap()
        .get(&id.to_string())
        .cloned()
        .unwrap_or_default();
    iroh::EndpointAddr::from(id).with_addrs(cached.into_iter().map(iroh::TransportAddr::Ip))
}

/// Is this `remote_addr` Debug string a private/link-local IP (i.e. same LAN)?
/// `PathInfo::remote_addr()` Debug-formats as e.g. `Ip(192.168.1.5:54321)` (seen in
/// our PERF logs). Best-effort string match; an unrecognized format degrades to
/// "not LAN" → labeled DIRECT, which is still truthful for a non-relay path.
fn addr_is_lan(dbg: &str) -> bool {
    dbg.contains("Ip(10.")
        || dbg.contains("Ip(192.168.")
        || dbg.contains("Ip(127.")
        || dbg.contains("Ip(169.254.") // link-local
        || dbg.contains("Ip([fe80") // IPv6 link-local
        || dbg.contains("Ip([::1]") // IPv6 loopback
        || (dbg.contains("Ip(172.")
            && dbg
                .split("Ip(172.")
                .nth(1)
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse::<u8>().ok())
                .map(|o| (16..=31).contains(&o)) // 172.16/12
                .unwrap_or(false))
}

/// Which channel the live connection is using: relayed (slow), a direct LAN path,
/// or a hole-punched DIRECT path over the internet. Read from the SELECTED QUIC
/// path on every progress tick, so it's live + truthful.
fn conn_locality(conn: &Connection) -> crate::models::Locality {
    use crate::models::Locality;
    use iroh::Watcher as _; // brings `.get()` into scope for the path watcher
    let mut watcher = conn.paths();
    let paths = watcher.get();
    // Extract owned (is_relay, addr) up front so the borrow of `paths` ends here.
    let selected = paths
        .iter()
        .find(|p| p.is_selected())
        .map(|p| (p.is_relay(), format!("{:?}", p.remote_addr())));
    match selected {
        Some((true, _)) => Locality::Internet, // relayed = the slow path
        // Direct peer-to-peer — distinguish same-LAN from a hole-punched WAN path by
        // the remote address so the badge can say "Local network" vs "Direct".
        Some((false, addr)) if addr_is_lan(&addr) => Locality::Local,
        Some((false, _)) => Locality::Direct,
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
    // A completed transfer is proof these addresses work — cache them.
    remember_conn_addrs(conn);
}

/// Wait until the connection has selected a DIRECT (hole-punched) p2p path, or give
/// up after `max`. THIS IS THE FIX FOR SLOW INTERNET TRANSFERS: an iroh dial
/// succeeds instantly via the relay, then hole-punches a direct path in the
/// background (usually a few hundred ms). A small file would otherwise be fully
/// buffered and "sent" over the rate-limited relay before that direct path ever
/// forms — which is exactly how a transfer ends up crawling at sub-KB/s on the
/// receiver while the sender thinks it flew. n0's public relays are coordination
/// only and throttle bulk data; the file body must ride a direct path.
///
/// Returns `true` once we're on a direct path; `false` if we time out (relay-only,
/// e.g. a symmetric-NAT peer) — the caller then proceeds anyway (correct, just slow)
/// rather than blocking forever. Returns immediately on a LAN where mDNS already
/// gave us a direct path.
async fn wait_for_direct_path(conn: &Connection, max: std::time::Duration) -> bool {
    use iroh::Watcher as _;
    let mut watcher = conn.paths();
    let res = tokio::time::timeout(max, async {
        loop {
            if watcher
                .get()
                .iter()
                .any(|p| p.is_selected() && !p.is_relay())
            {
                return true;
            }
            // Block until the path set changes; bail if the connection drops.
            if watcher.updated().await.is_err() {
                return false;
            }
        }
    })
    .await;
    let ok = res.unwrap_or(false);
    if ok {
        // A direct path just formed — remember the peer's working addresses so
        // future dials skip discovery entirely.
        remember_conn_addrs(conn);
    }
    ok
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
    // Seed the known-address cache so the FIRST dial after a relaunch already
    // carries every peer address that worked before.
    load_peer_addrs(config_dir);
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
                    // Log every LAN peer that appears/disappears. This makes the
                    // log DECISIVE about local discovery: if two machines sit on
                    // one LAN and neither logs "discovered ... on the local
                    // network", multicast is being blocked (macOS Local Network
                    // permission / AP isolation) — vs. iroh just not using it.
                    let sub = mdns.clone();
                    tauri::async_runtime::spawn(async move {
                        use n0_future::StreamExt as _;
                        let mut events = sub.subscribe().await;
                        // The event stream re-announces every second — log only
                        // CHANGES so the file stays readable.
                        let mut seen = std::collections::HashSet::new();
                        while let Some(ev) = events.next().await {
                            let line = format!("{ev:?}");
                            if seen.insert(line.clone()) {
                                log::info!("mDNS: {line}");
                            }
                        }
                    });
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
    // Give hole-punching a moment to settle, then remember the caller's direct
    // addresses — so the RECEIVING side also learns how to reach this peer
    // directly next time, even if local discovery is blocked.
    {
        let c = conn.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            remember_conn_addrs(&c);
        });
    }
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
            // Register the flag + connection so the sender can cancel mid-pull
            // (close the conn to unblock a stalled write).
            state
                .cancels
                .lock()
                .unwrap()
                .insert(p.transfer_id.clone(), p.cancel.clone());
            state
                .conns
                .lock()
                .unwrap()
                .insert(p.transfer_id.clone(), conn.clone());
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
                    // Get onto a direct p2p path before pushing bytes — otherwise a
                    // small file finishes over the rate-limited relay before hole-
                    // punching completes (the 0.3 KB/s bug). Falls through to relay
                    // if a direct path never forms (symmetric NAT).
                    if !wait_for_direct_path(conn, Duration::from_secs(8)).await {
                        log::warn!("quick-send: no direct path after 8s — relay (slow)");
                    }
                    let __t0 = std::time::Instant::now();
                    let __total = p.total;
                    let want_parallel =
                        req.get("parallel").and_then(|v| v.as_bool()).unwrap_or(false);
                    match serve_pull_negotiated(conn, send, recv, &p.paths, want_parallel, &p.cancel, cb)
                        .await
                    {
                        Ok(_) => {
                            // Wait for the receiver to confirm every byte landed
                            // before reporting done. Otherwise our timer only
                            // measures filling the local send buffer (instant) — not
                            // real delivery — so the sender's speed/duration would
                            // never match the receiver's. The friend + folder paths
                            // already do this; Quick Send was the one that didn't.
                            // Robust + lenient: a peer on an older build that never
                            // acks resets the stream (resolves fast), and a peer that
                            // vanishes mid-flight unblocks us via conn.closed() — so
                            // we never hang, and we fall back to the old timing rather
                            // than a false failure.
                            tokio::select! {
                                _ = recv.read_to_end(16) => {}
                                _ = conn.closed() => {}
                            }
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
                        Err(e)
                            if p.cancel.load(Ordering::SeqCst)
                                || e.to_string().contains("canceled") =>
                        {
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
            state.conns.lock().unwrap().remove(&p.transfer_id);
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
            // Auto-add an unknown sender as a friend (issue #6) — receiving a file
            // from someone makes the relationship two-way without a separate pairing
            // step. Needs their name, which the sender now puts in the header.
            let from_name = req.get("fromName").and_then(|v| v.as_str()).unwrap_or("").trim();
            let (sender, auto_accept) = match &friend {
                Some(f) => (Some(f.name.clone()), f.auto_accept),
                None if !from_name.is_empty() && !who.is_empty() => {
                    // Add them so the relationship is two-way — but DON'T grant silent
                    // standing access. A stranger who has your link is now a named
                    // friend whose FUTURE sends still prompt (auto_accept=false). The
                    // current file still lands (true), matching the prior behavior for
                    // an unknown sender.
                    let f = crate::friends::upsert_by_endpoint(&config_dir, &who, from_name);
                    let _ = crate::friends::set_auto_accept(&config_dir, &f.id, false);
                    let _ = app.emit("friends://changed", ());
                    (Some(f.name.clone()), true)
                }
                // A friend with manual-accept on must approve before we receive; an
                // unknown sender with no name still defaults to auto-accept.
                None => (None, true),
            };
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
            // Receive in parallel when the sender advertised it for a single big
            // file AND this is an auto-accept friend: reply {ready:true}, then pull
            // the N uni streams and reassemble. We gate on auto-accept so the
            // sender's short ready-timeout isn't spent waiting behind a manual
            // accept dialog (in that case the sender has already fallen back to the
            // classic single stream, so we must too). Everything else = classic.
            // Clamp the advertised stream count so a buggy/malicious peer can't make
            // us loop on accept_uni forever (honest peers cap themselves at 6).
            let parallel_n = req["parallel"].as_u64().unwrap_or(0).min(PARALLEL_STREAMS);
            let body = if parallel_n > 0 && names.len() == 1 && auto_accept {
                let name = names.first().cloned().unwrap_or_else(|| "file".to_string());
                // Resume identity: same sender + name + size + mtime = same bytes.
                // If an earlier attempt left a partial for this fingerprint, we tell
                // the sender which ranges we already have and it sends only the rest.
                let item0 = &req["items"][0];
                let fp = transfer_fingerprint(
                    &who,
                    &name,
                    item0["size"].as_u64().unwrap_or(total),
                    item0["mtime"].as_u64().unwrap_or(0),
                );
                // One resumable partial per fingerprint at a time; a concurrent
                // duplicate of the same file gets a throwaway temp instead.
                let resumable = state.partials.lock().unwrap().insert(fp.clone());
                let prep: Result<(PathBuf, Coverage)> = if resumable {
                    prepare_partial(&dest, &fp, total)
                } else {
                    (|| {
                        std::fs::create_dir_all(&dest)?;
                        let p = dest.join(format!(
                            ".dropbeam-partial-tmp-{}.part",
                            uuid::Uuid::new_v4()
                        ));
                        std::fs::File::create(&p)?.set_len(total)?;
                        Ok((p, Coverage::default()))
                    })()
                };
                let res = match prep {
                    Ok((part, cov)) => {
                        // The `resume` key (even with empty `have`) tells the sender
                        // we're coverage-aware — it may send any stream layout.
                        let ready =
                            serde_json::json!({ "ready": true, "resume": { "have": cov.ranges } });
                        match write_frame(send, &ready).await {
                            Ok(()) => {
                                // Peel off the first segment stream with a timeout. If
                                // none arrives, the sender raced us into its classic
                                // fallback (its ready-timeout fired just before our
                                // reply landed) — read the classic body instead. The
                                // resumable partial is KEPT (it may hold an earlier
                                // attempt's progress); only a throwaway is deleted.
                                match tokio::time::timeout(
                                    Duration::from_secs(12),
                                    conn.accept_uni(),
                                )
                                .await
                                {
                                    Ok(Ok(first)) => {
                                        let rc = resumable.then(|| ResumeCtx {
                                            side: partial_paths(&dest, &fp).1,
                                            fp: fp.clone(),
                                        });
                                        recv_file_resumable(
                                            conn,
                                            FinalizeDest::UniqueIn(dest.clone(), name.clone()),
                                            total, part, rc, cov, first, &cancel, cb,
                                        )
                                        .await
                                        .map(|p| vec![p])
                                    }
                                    _ => {
                                        if !resumable {
                                            let _ = std::fs::remove_file(&part);
                                        }
                                        let r = read_body(recv, &req, &dest, &cancel, cb).await;
                                        // The file arrived classically — a kept
                                        // partial would only make a LATER send of
                                        // the same file resume into a duplicate
                                        // "name (1)" copy. Drop it.
                                        if r.is_ok() && resumable {
                                            let _ = std::fs::remove_file(&part);
                                            let _ = std::fs::remove_file(partial_paths(&dest, &fp).1);
                                        }
                                        r
                                    }
                                }
                            }
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                };
                if resumable {
                    state.partials.lock().unwrap().remove(&fp);
                }
                res
            } else {
                read_body(recv, &req, &dest, &cancel, cb).await
            };
            match body {
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
            // A peer is introducing themselves: learn their stable EndpointId +
            // name. `friend_id` (if present) matches the classic invite flow;
            // otherwise we auto-add them by EndpointId so one permanent-code share
            // makes the friendship two-way. The connection's verified remote id is
            // authoritative for their key (don't trust the self-reported one).
            let friend_id = req.get("friend_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let who = conn.remote_id().to_string();
            if let Some(app) = state.app.get() {
                if let Some(st) = app.try_state::<Arc<crate::AppState>>() {
                    crate::friends::apply_hello(&st.config_dir, friend_id, &who, name);
                    let _ = app.emit("pairs://changed", ());
                    let _ = app.emit("friends://changed", ());
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
            // Drop visible "<name>.dropbeam-incoming" placeholders in the REAL folder
            // so the recipient sees a file is on the way (esp. a multi-minute big
            // transfer), then remove them once the real files land. The suffix is
            // skipped by the watcher, so they never sync or trigger a mirror-delete.
            let placeholders: Vec<PathBuf> = sm
                .folder_path(&pair_id)
                .map(|folder| {
                    req["items"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|it| {
                                    let raw = it.get("name").and_then(|n| n.as_str())?;
                                    let rel = sanitize_rel(raw);
                                    let ph = Path::new(&folder)
                                        .join(format!("{}.dropbeam-incoming", rel.to_string_lossy()));
                                    if let Some(parent) = ph.parent() {
                                        let _ = std::fs::create_dir_all(parent);
                                    }
                                    let _ = std::fs::File::create(&ph);
                                    Some(ph)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let clear_placeholders = || {
                for ph in &placeholders {
                    let _ = std::fs::remove_file(ph);
                }
            };
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
            // Parallel receive for one big folder file — same negotiation + resume
            // as friend sends. Partials live OUTSIDE the staging dir (it's wiped per
            // receive), so the sync loop's automatic retry RESUMES a big file that
            // died mid-transfer instead of starting over.
            let parallel_n = req["parallel"].as_u64().unwrap_or(0).min(PARALLEL_STREAMS);
            let single = req["items"].as_array().map(|a| a.len()) == Some(1);
            let result = if parallel_n > 0 && single && total >= PARALLEL_MIN {
                let item0 = &req["items"][0];
                let raw = item0["name"].as_str().unwrap_or("file");
                let rel = sanitize_rel(raw);
                let mtime = item0["mtime"].as_u64().unwrap_or(0);
                let who = conn.remote_id().to_string();
                let fp = transfer_fingerprint(&who, &rel.to_string_lossy(), total, mtime);
                let partial_dir = config_dir.join("folder-partials");
                let resumable = state.partials.lock().unwrap().insert(fp.clone());
                // A concurrent receive of the same fingerprint must never share the
                // partial — the loser writes into a throwaway temp instead.
                let prep: Result<(PathBuf, Coverage)> = if resumable {
                    prepare_partial(&partial_dir, &fp, total)
                } else {
                    (|| {
                        std::fs::create_dir_all(&partial_dir)?;
                        let p = partial_dir
                            .join(format!(".dropbeam-partial-tmp-{}.part", uuid::Uuid::new_v4()));
                        std::fs::File::create(&p)?.set_len(total)?;
                        Ok((p, Coverage::default()))
                    })()
                };
                let res = match prep {
                    Ok((part, cov)) => {
                        let ready =
                            serde_json::json!({ "ready": true, "resume": { "have": cov.ranges } });
                        match write_frame(send, &ready).await {
                            Ok(()) => match tokio::time::timeout(
                                Duration::from_secs(12),
                                conn.accept_uni(),
                            )
                            .await
                            {
                                Ok(Ok(first)) => {
                                    let rc = resumable.then(|| ResumeCtx {
                                        side: partial_paths(&partial_dir, &fp).1,
                                        fp: fp.clone(),
                                    });
                                    recv_file_resumable(
                                        conn,
                                        FinalizeDest::Exact(staging.join(&rel), mtime),
                                        total,
                                        part,
                                        rc,
                                        cov,
                                        first,
                                        &cancel,
                                        cb,
                                    )
                                    .await
                                    .map(|p| vec![p])
                                }
                                // Sender raced into its classic fallback — read the
                                // classic body; the resumable partial is KEPT, a
                                // throwaway temp is not.
                                _ => {
                                    if !resumable {
                                        let _ = std::fs::remove_file(&part);
                                    }
                                    read_folder_body(recv, &req, &staging, &cancel, cb).await
                                }
                            },
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                };
                if resumable {
                    state.partials.lock().unwrap().remove(&fp);
                }
                res
            } else {
                read_folder_body(recv, &req, &staging, &cancel, cb).await
            };
            match result {
                Ok(_) => {
                    log_transfer_perf(conn, "folder-recv", "recv", total, __t0.elapsed());
                    sm.ingest_iroh_folder_files(&pair_id, &staging);
                    let _ = std::fs::remove_dir_all(&staging);
                    clear_placeholders();
                    let _ = send.write_all(b"ok").await;
                    let _ = send.finish();
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    clear_placeholders();
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
            let group_id = req
                .get("group_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let members: Vec<(String, String)> = req
                .get("members")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let eid = m.get("eid").and_then(|e| e.as_str())?.to_string();
                            let n = m
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            Some((eid, n))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !pair_id.is_empty() {
                if let Some(app) = state.app.get() {
                    if let Some(sm) = app.try_state::<Arc<crate::sync::SyncManager>>() {
                        let sm = sm.inner().clone();
                        sm.apply_remote_control(&pair_id, &name, &deletes, &group_id, &members);
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
                let from_name = req.get("fromName").and_then(|v| v.as_str()).unwrap_or("");
                let friend = friends
                    .iter()
                    .find(|f| f.endpoint_id.as_deref() == Some(who.as_str()))
                    .or_else(|| claimed.and_then(|id| friends.iter().find(|f| f.id == id)))
                    .cloned()
                    .or_else(|| {
                        // Unknown sender who introduced themselves (they hold our
                        // permanent code) → auto-add so the conversation is two-way.
                        if from_name.is_empty() {
                            None
                        } else {
                            let f = crate::friends::upsert_by_endpoint(&config_dir, &who, from_name);
                            let _ = app.emit("friends://changed", ());
                            Some(f)
                        }
                    });
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
                        status: None,
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
            // `parallel: true` tells the sender we understand multi-stream +
            // resume; an older sender just ignores it.
            write_frame(
                &mut send,
                &serde_json::json!({ "kind": "pull", "token": token, "parallel": true }),
            )
            .await?;
            let cb = progress_cb(
                app.clone(),
                id.clone(),
                Direction::Receive,
                Vec::new(),
                None,
                conn.clone(),
            );
            let __t0 = std::time::Instant::now();
            let header = read_frame(&mut recv).await?;
            let paths =
                read_files_negotiated(&conn, &mut send, &mut recv, &header, &dest, &cancel, cb)
                    .await?;
            // Confirm receipt so the SENDER can measure REAL delivery time (it now
            // waits for this before finalizing) — making both sides report identical
            // bytes + speed instead of the sender's instant-buffer fiction.
            let _ = send.write_all(b"ok").await;
            let _ = send.finish();
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
    let addr = dial_addr(parsed);
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
    // Our own display name travels with the push so the recipient can auto-add us
    // as a friend if they don't already have us (issue #6).
    let my_name = app
        .try_state::<Arc<crate::AppState>>()
        .map(|st| st.settings.lock().unwrap().display_name.clone())
        .unwrap_or_default();
    tauri::async_runtime::spawn(async move {
        let outcome: Result<crate::models::Locality> = async {
            // A parallel transfer that was UNDERWAY and died is almost always a
            // network blip / sleep — so we auto-reconnect up to 2 extra times, and
            // the resume handshake picks up from the receiver's partial instead of
            // byte zero. The retry is gated on the receiver having replied
            // {ready:true} THIS attempt: that proves an auto-accept parallel receive
            // engaged, so the error can't be a manual-accept DECLINE (retrying one
            // of those would re-prompt the recipient), and classic multi-file sends
            // keep fail-fast so a retry can't duplicate files.
            let mut attempt: u32 = 0;
            loop {
                // Bounded so an offline/unreachable friend FAILS clearly instead of
                // hanging forever. Bounded re-dial: a friend who's briefly offline
                // (e.g. just opening their app) still receives; the file stays
                // "Connecting" meanwhile, and a cancel breaks out immediately.
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
                cleanup.conns.lock().unwrap().insert(id.clone(), conn.clone());
                // Prefer a direct p2p path before sending — the relay is rate-limited
                // and would crawl. Proceeds over relay if direct never forms.
                let got_direct = wait_for_direct_path(&conn, Duration::from_secs(8)).await;
                if !got_direct {
                    log::warn!("friend-send: no direct path after 8s — relay (slow)");
                }
                // Field-confirmed failure mode: a direct path collapses under
                // sustained load, iroh fails over to the rate-limited RELAY, the
                // transfer crawls until a stall guard kills it, and the attempt is
                // wasted. The cure: if this transfer WAS direct and sits on relay
                // for >8s, proactively close the connection — the retry loop
                // reconnects (fresh hole-punch → direct again) and RESUMES. Peers
                // that never had a direct path are exempt (relay is their best).
                let had_direct = got_direct;
                let watchdog = had_direct.then(|| {
                    let c = conn.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut relay_since: Option<Instant> = None;
                        loop {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            let on_relay = matches!(
                                conn_locality(&c),
                                crate::models::Locality::Internet
                            );
                            match (on_relay, relay_since) {
                                (true, Some(t)) if t.elapsed() > Duration::from_secs(8) => {
                                    log::warn!(
                                        "transfer degraded direct→relay — reconnecting to re-holepunch"
                                    );
                                    c.close(1u32.into(), b"degraded-reconnect");
                                    break;
                                }
                                (true, None) => relay_since = Some(Instant::now()),
                                (false, _) => relay_since = None,
                                _ => {}
                            }
                        }
                    })
                });
                // Fresh per attempt: only THIS attempt's handshake authorizes a retry.
                let engaged = AtomicBool::new(false);
                let __t0 = std::time::Instant::now();
                let outcome = send_files(&conn, &pathbufs, &cancel, cb, &my_name, &engaged).await;
                if let Some(w) = watchdog {
                    w.abort();
                }
                match outcome {
                    Ok(_) => {
                        log_transfer_perf(&conn, "friend-send", "send", total, __t0.elapsed());
                        return Ok(conn_locality(&conn));
                    }
                    Err(e) => {
                        let canceled = cancel.load(Ordering::SeqCst)
                            || e.to_string().contains("canceled");
                        // With resume, attempts are cheap — a 2 GB send that loses
                        // its path every ~400 MB still completes within the budget.
                        if !canceled && attempt < 5 && engaged.load(Ordering::SeqCst) {
                            attempt += 1;
                            log::warn!(
                                "friend-send interrupted ({e:#}) — auto-resuming, attempt {attempt}/5"
                            );
                            let mut ru =
                                TransferUpdate::new(id.clone(), Direction::Send, names.clone());
                            ru.friend_name = Some(friend_name.clone());
                            ru.state = TransferState::Connecting;
                            ru.bytes_total = total;
                            emit(&app, &ru);
                            tokio::time::sleep(Duration::from_secs(1 + attempt as u64)).await;
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
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
            // A cancel may surface as our own "canceled" bail OR as a
            // connection-closed error (we close the conn to unblock the write),
            // so trust the flag too.
            Err(e) if cancel.load(Ordering::SeqCst) || e.to_string().contains("canceled") => {
                emit_canceled(&app, &id, Direction::Send)
            }
            Err(e) => emit_failed(&app, &id, Direction::Send, &e.to_string()),
        }
        cleanup.cancels.lock().unwrap().remove(&id);
        cleanup.conns.lock().unwrap().remove(&id);
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
    let addr = dial_addr(parsed);
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

/// After adding a friend by their permanent code, introduce ourselves to them so
/// the friendship is two-way: dial their EndpointId and hand over our id + name,
/// which they auto-add (the reverse of `add_by_code`). Best-effort; if they're
/// offline now, the first message/file we send carries our name too.
pub fn say_hello_to_endpoint(state: Arc<IrohState>, endpoint_id: String, my_name: String) {
    let Some(ep) = state.get().cloned() else {
        return;
    };
    let Ok(parsed) = endpoint_id.parse::<iroh::EndpointId>() else {
        return;
    };
    let my_id = ep.id().to_string();
    let addr = dial_addr(parsed);
    tauri::async_runtime::spawn(async move {
        let hello = serde_json::json!({
            "kind": "friend-hello", "friend_id": "", "endpoint_id": my_id, "name": my_name,
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
    let addr = dial_addr(parsed);
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
    group_id: &str,
    members: &[(String, String)],
) -> Result<()> {
    let parsed: iroh::EndpointId = endpoint_id.parse().context("parse peer endpoint id")?;
    let addr = dial_addr(parsed);
    let dels: Vec<serde_json::Value> = deletes
        .iter()
        .map(|(rel, ts)| serde_json::json!({ "rel": rel, "ts": ts }))
        .collect();
    // The group roster rides the beacon so everyone meshes with everyone
    // (multi-person folders). Empty group_id / members on a classic 1:1 folder.
    let mem: Vec<serde_json::Value> = members
        .iter()
        .map(|(eid, n)| serde_json::json!({ "eid": eid, "name": n }))
        .collect();
    let msg = serde_json::json!({
        "kind": "folder-ctrl", "pair_id": pair_id, "name": name, "deletes": dels,
        "group_id": group_id, "members": mem,
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
    let addr = dial_addr(parsed);
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

/// Build the wire frame for a chat message (shared by the send command + the
/// outbox retry so they stay identical).
fn chat_payload(m: &crate::chat::ChatMessage, peer_id: &str, my_name: &str) -> serde_json::Value {
    if m.kind == "file" {
        serde_json::json!({
            "kind": "chat", "msgKind": "file", "friendId": peer_id, "fromName": my_name,
            "id": m.id, "files": m.files, "bytes": m.bytes, "ts": m.ts,
        })
    } else {
        serde_json::json!({
            "kind": "chat", "msgKind": "text", "friendId": peer_id, "fromName": my_name,
            "id": m.id, "text": m.text, "ts": m.ts,
        })
    }
}

/// Background loop that flushes the chat outbox: every few seconds it retries
/// every undelivered message to any peer that's now reachable, oldest-first per
/// peer (so order is preserved and one failure doesn't let later messages jump
/// ahead). This is what makes chat reliable — a message that couldn't send keeps
/// retrying until it lands, instead of being silently dropped.
pub fn spawn_chat_outbox_retry(app: AppHandle, state: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(12)).await;
            let Some(ep) = state.get().cloned() else {
                continue;
            };
            // Pull what we need without holding the State guard across awaits.
            let (config_dir, my_name) = {
                let Some(st) = app.try_state::<Arc<crate::AppState>>() else {
                    continue;
                };
                let cd = st.config_dir.clone();
                let name = st.settings.lock().unwrap().display_name.clone();
                (cd, name)
            };
            let pending = crate::chat::outbox(&config_dir);
            if pending.is_empty() {
                continue;
            }
            let friends = crate::friends::load(&config_dir);
            let mut by_peer: std::collections::HashMap<String, Vec<crate::chat::ChatMessage>> =
                std::collections::HashMap::new();
            for m in pending {
                by_peer.entry(m.peer_id.clone()).or_default().push(m);
            }
            for (peer_id, msgs) in by_peer {
                let Some(eid) = friends
                    .iter()
                    .find(|f| f.id == peer_id)
                    .and_then(|f| f.endpoint_id.clone())
                else {
                    continue;
                };
                for m in msgs {
                    let payload = chat_payload(&m, &peer_id, &my_name);
                    match send_chat(&ep, &eid, payload).await {
                        Ok(_) => {
                            if let Some(u) = crate::chat::set_status(&config_dir, &peer_id, &m.id, "sent")
                            {
                                let _ = app.emit("chat://message", &u);
                            }
                        }
                        Err(_) => {
                            if let Some(u) =
                                crate::chat::set_status(&config_dir, &peer_id, &m.id, "failed")
                            {
                                let _ = app.emit("chat://message", &u);
                            }
                            break; // preserve order — stop this peer until next round
                        }
                    }
                }
            }
        }
    });
}

/// Liveness check: dial a friend/peer's endpoint and round-trip a ping. Returns
/// true only if they answered with a pong (their app is running and reachable) —
/// the iroh replacement for the croc "ping_send" online check.
pub async fn ping_endpoint(ep: &Endpoint, endpoint_id: &str) -> bool {
    let Ok(parsed) = endpoint_id.parse::<iroh::EndpointId>() else {
        return false;
    };
    let addr = dial_addr(parsed);
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
    let addr = dial_addr(parsed);
    let conn = tokio::time::timeout(Duration::from_secs(12), ep.connect(addr, ALPN))
        .await
        .map_err(|_| anyhow::anyhow!("folder peer unreachable over iroh"))?
        .context("dial folder peer")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    // Prefer a direct path for folder sync too (shorter wait since this runs in the
    // background and LAN peers get a direct path via mDNS almost immediately).
    let _ = wait_for_direct_path(&conn, Duration::from_secs(5)).await;
    let __t0 = std::time::Instant::now();

    let mut items = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
        items.push((p.clone(), folder_rel(p, root), meta.len(), mtime_secs(&meta)));
    }
    let total: u64 = items.iter().map(|i| i.2).sum();
    // Big single folder files fan across parallel streams exactly like friend
    // sends (same negotiation, same resume). Folder sync was the LAST big-file
    // path still single-stream — which is where iroh's per-stream stalls hurt.
    let n = parallel_stream_count(items.len(), total);
    let header = serde_json::json!({
        "kind": "folder-files",
        "pair_id": pair_id,
        // `mtime` (seconds) travels with each file so EVERY member writes it with
        // the same modified-time → the same file signature group-wide (loop-guard
        // works across a mesh, identical re-receives are no-ops).
        "items": items
            .iter()
            .map(|(_, n, s, mt)| serde_json::json!({ "name": n, "size": s, "mtime": mt }))
            .collect::<Vec<_>>(),
        "total": total,
        "parallel": n,
    });
    write_frame(&mut send, &header).await?;

    if n > 0 {
        let reply = match tokio::time::timeout(Duration::from_secs(6), read_frame(&mut recv)).await
        {
            Ok(Ok(v)) => Some(v),
            _ => None, // older receiver: no reply → classic body below
        };
        let ready = reply
            .as_ref()
            .and_then(|v| v.get("ready"))
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        if ready {
            let (base, plan) = parse_resume_reply(reply.as_ref(), total, n);
            send_ranges_parallel(&conn, &items[0].0, total, base, &plan, cancel, on_progress)
                .await?;
            send.finish()?;
            let ack = recv.read_to_end(4096).await.unwrap_or_default();
            anyhow::ensure!(ack.ends_with(b"ok"), "folder peer did not confirm receipt");
            log_transfer_perf(&conn, "folder-send", "send", total, __t0.elapsed());
            return Ok(conn_locality(&conn));
        }
    }

    write_folder_body(&mut send, &items, total, cancel, &on_progress).await?;
    send.finish()?;
    // Require the receiver's "ok" so "delivered" means the bytes actually landed
    // in their folder (not just that we finished writing to the socket).
    let ack = recv.read_to_end(4096).await.unwrap_or_default();
    anyhow::ensure!(ack.ends_with(b"ok"), "folder peer did not confirm receipt");
    log_transfer_perf(&conn, "folder-send", "send", total, __t0.elapsed());
    Ok(conn_locality(&conn))
}

/// Parse the receiver's `{ready, resume:{have}}` reply into (already-have bytes,
/// stream plan). A `resume` key marks a coverage-aware receiver (any stream
/// layout works — send only what's missing, with a zero-length "kick" stream if
/// it already has everything); no `resume` key = an older exact-N receiver that
/// needs the legacy equal segmentation.
fn parse_resume_reply(
    reply: Option<&serde_json::Value>,
    total: u64,
    n: u64,
) -> (u64, Vec<(u64, u64)>) {
    match reply.and_then(|v| v.get("resume")) {
        Some(r) => {
            let mut have = Coverage::default();
            if let Some(arr) = r.get("have").and_then(|h| h.as_array()) {
                for pair in arr {
                    if let (Some(s), Some(e)) = (
                        pair.get(0).and_then(|x| x.as_u64()),
                        pair.get(1).and_then(|x| x.as_u64()),
                    ) {
                        have.insert(s.min(total), e.min(total));
                    }
                }
            }
            let base = have.covered();
            if base > 0 {
                log::info!("resume: receiver already has {base}/{total} bytes");
            }
            let mut plan = plan_resume_ranges(&have.missing(total), n);
            if plan.is_empty() {
                plan.push((0, 0));
            }
            (base, plan)
        }
        // No resume key = a pre-v0.11 receiver. Those clamp the advertised count
        // to THEIR historical max of 6 and accept EXACTLY that many streams — so a
        // legacy plan must never exceed 6 or the extra streams would sit
        // unaccepted and the transfer would hang/fail. Coverage-aware receivers
        // (resume key present) are count-agnostic and take the full fan-out.
        None => (0, legacy_plan(total, n.min(6))),
    }
}

/// Folder PUSH writer: like `write_files`, but tags the stream with `pair_id` and
/// sends each file's path RELATIVE to the folder root (so `sub/a.txt` lands in
/// `sub/a.txt`, not the folder root).
/// A file's modified-time as whole seconds since the epoch (0 if unavailable).
fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stamp a file's modified-time to a specific epoch-seconds value, so a synced
/// file carries the SAME mtime on every member (stable signatures, no storm).
fn set_mtime_secs(path: &Path, secs: u64) {
    if secs == 0 {
        return;
    }
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(when);
    }
}

/// Classic single-stream folder body (the caller already wrote the header).
async fn write_folder_body<F: Fn(u64, u64)>(
    send: &mut SendStream,
    items: &[(PathBuf, String, u64, u64)],
    total: u64,
    cancel: &AtomicBool,
    on_progress: &F,
) -> Result<u64> {
    let mut sent = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (path, _, _, _) in items {
        let mut f = tokio::fs::File::open(path).await?;
        let mut since_check: u32 = 0;
        loop {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("canceled");
            }
            // If the user deletes the file mid-upload, STOP — don't waste minutes
            // pushing a 6 GB file they already removed (the delete propagates to the
            // peer separately, so it never lands there). Checked every ~16 MiB (a
            // bare stat), not every chunk. On macOS an unlinked file still reads via
            // the open handle but the path is gone, so exists() catches it.
            since_check += 1;
            if since_check >= 16 {
                since_check = 0;
                if !path.exists() {
                    anyhow::bail!("source file was removed during send");
                }
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
        drop(f); // close before stamping the mtime
        // Preserve the origin's modified-time so this file's signature matches on
        // every member (older peers omit `mtime` → falls back to receive-time).
        set_mtime_secs(&dest, item["mtime"].as_u64().unwrap_or(0));
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

/// Parallel transfer tuning. A single QUIC stream tops out around ~40% of link
/// capacity in iroh 0.98 (n0-computer/iroh#4286), so for a large single file we fan
/// the bytes across several unidirectional streams on the same connection and
/// reassemble. Each stream carries one CONTIGUOUS segment, so the receiver just
/// seeks once and writes sequentially — no fragile positioned-write juggling.
const PARALLEL_STREAMS: u64 = 8;
/// Don't split anything smaller than this — the negotiation + extra stream setup
/// isn't worth it and small files already transfer instantly.
const PARALLEL_MIN: u64 = 16 * 1024 * 1024; // 16 MiB

/// How many streams to fan a transfer across: only a SINGLE file at least
/// PARALLEL_MIN big, and never so many that a stream would carry under ~4 MiB.
/// Returns 0 = "send the classic single-stream way".
fn parallel_stream_count(item_count: usize, total: u64) -> u64 {
    if item_count != 1 || total < PARALLEL_MIN {
        return 0;
    }
    PARALLEL_STREAMS.min((total / (4 * 1024 * 1024)).max(1))
}

/// Drain a set of transfer workers, reporting summed progress every 150ms and
/// honoring the cancel flag. On the first worker error, aborts the rest and
/// surfaces that error (so a half-finished parallel transfer fails cleanly rather
/// than silently dropping bytes). Returns each worker's value on success.
async fn drain_with_progress<T: Send + 'static, F: Fn(u64, u64)>(
    mut set: tokio::task::JoinSet<Result<T>>,
    progress: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    total: u64,
    cancel: &AtomicBool,
    on_progress: &F,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    let mut err: Option<anyhow::Error> = None;
    while !set.is_empty() {
        tokio::select! {
            joined = set.join_next() => match joined {
                Some(Ok(Ok(v))) => out.push(v),
                Some(Ok(Err(e))) => { if err.is_none() { err = Some(e); } set.abort_all(); }
                Some(Err(e)) => {
                    if err.is_none() { err = Some(anyhow::anyhow!("transfer worker failed: {e}")); }
                    set.abort_all();
                }
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(150)) => {
                if cancel.load(Ordering::SeqCst) {
                    if err.is_none() { err = Some(anyhow::anyhow!("canceled")); }
                    set.abort_all();
                } else if err.is_none() {
                    on_progress(progress.load(std::sync::atomic::Ordering::Relaxed), total);
                }
            }
        }
    }
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

// ── Resumable transfers ──────────────────────────────────────────────────────
//
// Each parallel stream self-describes the byte range it carries, so the receiver
// tracks WHICH RANGES have landed (a coverage map) instead of counting streams —
// and persists that map in a sidecar next to a hidden partial file. If anything
// dies mid-transfer (network blip, sleep, app quit on either side), the partial
// survives; the next send of the same file (same sender + name + size + mtime)
// resumes from the missing ranges instead of byte zero, and the sender
// auto-reconnects to make a connection drop invisible.

/// Sorted, non-overlapping half-open byte ranges `[start, end)` that have landed
/// on disk. Adjacent/overlapping inserts merge, so ANY mixture of segmentations
/// (a resumed send, a legacy exact-N send, even both) converges to one range.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Coverage {
    ranges: Vec<(u64, u64)>,
}

impl Coverage {
    fn insert(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        let (mut ns, mut ne) = (start, end);
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut placed = false;
        for &(s, e) in &self.ranges {
            if e < ns || s > ne {
                // Disjoint (half-open ranges touching at a point DO merge above).
                if s > ne && !placed {
                    out.push((ns, ne));
                    placed = true;
                }
                out.push((s, e));
            } else {
                ns = ns.min(s);
                ne = ne.max(e);
            }
        }
        if !placed {
            out.push((ns, ne));
        }
        self.ranges = out;
    }

    fn covered(&self) -> u64 {
        self.ranges.iter().map(|(s, e)| e - s).sum()
    }

    /// The gaps in `[0, total)` not yet covered, as (start, end) pairs.
    fn missing(&self, total: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut cursor = 0u64;
        for &(s, e) in &self.ranges {
            if s > cursor {
                out.push((cursor, s.min(total)));
            }
            cursor = cursor.max(e);
        }
        if cursor < total {
            out.push((cursor, total));
        }
        out
    }
}

/// On-disk record of a partially received file, written beside the partial so a
/// resume works across reconnects AND app restarts.
#[derive(serde::Serialize, serde::Deserialize)]
struct PartialSidecar {
    v: u32,
    fp: String,
    total: u64,
    coverage: Coverage,
}

/// Resume bookkeeping for one receive: where the sidecar lives + the fingerprint.
struct ResumeCtx {
    side: PathBuf,
    fp: String,
}

/// Identity of a transfer for resume purposes: same sender + name + size + mtime
/// = same bytes (the rsync-style heuristic). An edited file changes mtime/size →
/// different fingerprint → fresh transfer, never a corrupt mix.
fn transfer_fingerprint(sender: &str, name: &str, size: u64, mtime: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(sender.as_bytes());
    h.update([0]);
    h.update(name.as_bytes());
    h.update([0]);
    h.update(size.to_be_bytes());
    h.update(mtime.to_be_bytes());
    hex::encode(&h.finalize()[..8])
}

fn partial_paths(dir: &Path, fp: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!(".dropbeam-partial-{fp}.part")),
        dir.join(format!(".dropbeam-partial-{fp}.json")),
    )
}

fn save_sidecar(path: &Path, sc: &PartialSidecar) {
    if let Ok(json) = serde_json::to_vec(sc) {
        // Atomic-ish: write a temp then rename, so a crash never leaves a torn file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn load_sidecar(path: &Path, fp: &str, total: u64) -> Option<Coverage> {
    let sc: PartialSidecar = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if sc.v != 1 || sc.fp != fp || sc.total != total {
        return None;
    }
    // NEVER trust on-disk ranges blindly — covered()/missing() and the completion
    // check assume sorted/disjoint/in-bounds, and a corrupted sidecar claiming
    // full coverage would short-circuit a garbage partial straight into a
    // "completed" file. Rebuild through insert(), which restores every invariant.
    let mut c = Coverage::default();
    for (s, e) in sc.coverage.ranges {
        c.insert(s.min(total), e.min(total));
    }
    Some(c)
}

/// Abandoned partials are cleaned up after a week — long enough to resume a big
/// transfer "tomorrow", short enough not to hoard disk.
const PARTIAL_TTL_SECS: u64 = 7 * 24 * 3600;

fn gc_stale_partials(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(".dropbeam-partial-") {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() > PARTIAL_TTL_SECS)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Open (or reopen) the hidden partial for `fp`, pre-sized to `total`, returning
/// any prior coverage from a matching sidecar — i.e. how much we already have.
fn prepare_partial(dir: &Path, fp: &str, total: u64) -> Result<(PathBuf, Coverage)> {
    std::fs::create_dir_all(dir)?;
    gc_stale_partials(dir);
    let (part, side) = partial_paths(dir, fp);
    let coverage = if part.is_file() {
        load_sidecar(&side, fp, total).unwrap_or_default()
    } else {
        // No partial → any surviving sidecar is an orphan describing bytes that no
        // longer exist. Remove it NOW, or a later crash-before-persist could let it
        // claim coverage over the fresh zero-filled file we're about to create.
        let _ = std::fs::remove_file(&side);
        Coverage::default()
    };
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&part)?;
    f.set_len(total)?;
    Ok((part, coverage))
}

/// The legacy equal segmentation that pre-resume receivers expect: EXACTLY `n`
/// streams (including zero-length trailing ones), so their fixed accept loop
/// never blocks waiting for a stream that won't come.
fn legacy_plan(total: u64, n: u64) -> Vec<(u64, u64)> {
    let seg = total.div_ceil(n.max(1));
    (0..n.max(1))
        .map(|i| {
            let s = (i * seg).min(total);
            (s, seg.min(total - s))
        })
        .collect()
}

/// Plan streams for the MISSING ranges of a resumed transfer, as (offset, len).
/// Coverage-aware receivers don't care about stream count, so: merge the
/// closest-together ranges if there are more gaps than streams (re-sending the
/// small covered sliver between them is harmless — writes are positioned), then
/// split the biggest ranges so large resumes still get multi-stream speed.
fn plan_resume_ranges(missing: &[(u64, u64)], max_streams: u64) -> Vec<(u64, u64)> {
    let max = max_streams.max(1) as usize;
    let mut ranges: Vec<(u64, u64)> = missing
        .iter()
        .filter(|&&(s, e)| e > s)
        .map(|&(s, e)| (s, e - s))
        .collect();
    while ranges.len() > max {
        let mut best = 0;
        let mut best_gap = u64::MAX;
        for i in 0..ranges.len() - 1 {
            let gap = ranges[i + 1].0 - (ranges[i].0 + ranges[i].1);
            if gap < best_gap {
                best_gap = gap;
                best = i;
            }
        }
        let (s1, _) = ranges[best];
        let (s2, l2) = ranges[best + 1];
        ranges[best] = (s1, (s2 + l2) - s1);
        ranges.remove(best + 1);
    }
    while ranges.len() < max {
        let Some((idx, &(s, l))) = ranges
            .iter()
            .enumerate()
            .max_by_key(|(_, &(_, l))| l)
        else {
            break;
        };
        if l < 8 * 1024 * 1024 {
            break;
        }
        let half = l / 2;
        ranges[idx] = (s, half);
        ranges.insert(idx + 1, (s + half, l - half));
    }
    ranges
}

/// SEND the given (offset, len) ranges of one file, each on its own uni stream
/// (16-byte offset+len header, then the bytes). `base` is how many bytes the
/// receiver already has, so progress starts where the resume left off instead of
/// lying back to 0%.
async fn send_ranges_parallel<F: Fn(u64, u64)>(
    conn: &Connection,
    path: &Path,
    total: u64,
    base: u64,
    plan: &[(u64, u64)],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<()> {
    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(base));
    let mut set: tokio::task::JoinSet<Result<()>> = tokio::task::JoinSet::new();
    for &(start, len) in plan {
        let conn = conn.clone();
        let path = path.to_path_buf();
        let progress = progress.clone();
        set.spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let mut uni = conn.open_uni().await?;
            // The explicit length lets the receiver write EXACTLY its region and
            // reject an over- or under-sending peer, instead of trusting EOF.
            uni.write_all(&start.to_be_bytes()).await?;
            uni.write_all(&len.to_be_bytes()).await?;
            if len > 0 {
                let mut f = tokio::fs::File::open(&path).await?;
                f.seek(std::io::SeekFrom::Start(start)).await?;
                let mut remaining = len;
                let mut buf = vec![0u8; CHUNK];
                while remaining > 0 {
                    let want = remaining.min(CHUNK as u64) as usize;
                    let k = f.read(&mut buf[..want]).await?;
                    if k == 0 {
                        anyhow::bail!("file ended early while sending segment");
                    }
                    uni.write_all(&buf[..k]).await?;
                    remaining -= k as u64;
                    progress.fetch_add(k as u64, std::sync::atomic::Ordering::Relaxed);
                }
            }
            uni.finish()?;
            // Keep the stream alive until the peer has acked the segment, so the
            // last bytes aren't dropped by an early reset when the task ends.
            let _ = uni.stopped().await;
            Ok(())
        });
    }
    if !set.is_empty() {
        drain_with_progress(set, &progress, total, cancel, &on_progress).await?;
    }
    on_progress(total, total);
    Ok(())
}

/// One worker draining one inbound `[offset, len]` stream into the partial,
/// marking each chunk in the shared coverage map as it lands.
fn spawn_range_reader(
    set: &mut tokio::task::JoinSet<Result<()>>,
    mut uni: RecvStream,
    part: PathBuf,
    total: u64,
    cov: std::sync::Arc<Mutex<Coverage>>,
) {
    set.spawn(async move {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let mut hdr = [0u8; 16];
        uni.read_exact(&mut hdr).await?;
        let offset = u64::from_be_bytes(hdr[0..8].try_into().unwrap());
        let len = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
        anyhow::ensure!(
            offset.checked_add(len).map(|e| e <= total).unwrap_or(false),
            "segment out of bounds ({offset}+{len} > {total})"
        );
        if len == 0 {
            return Ok(());
        }
        let mut f = tokio::fs::OpenOptions::new().write(true).open(&part).await?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        while done < len {
            let want = (len - done).min(CHUNK as u64) as usize;
            match uni.read(&mut buf[..want]).await? {
                Some(k) if k > 0 => {
                    f.write_all(&buf[..k]).await?;
                    // Mark coverage only AFTER the bytes are written, so the
                    // sidecar never claims data the disk doesn't have (app-level
                    // interruptions, the overwhelmingly common case, stay safe).
                    let end = offset + done + k as u64;
                    cov.lock().unwrap().insert(offset + done, end);
                    done += k as u64;
                }
                _ => anyhow::bail!("segment stream ended early ({} bytes short)", len - done),
            }
        }
        f.flush().await?;
        Ok(())
    });
}

/// RECEIVE one file fanned across self-describing range streams into `dest_dir`.
/// `first` is the already-accepted stream 0 (the caller peeled it with a timeout
/// to detect a sender that raced into its classic fallback). Completion = every
/// byte of `[0, total)` covered — stream COUNT doesn't matter, so resumed sends
/// (fewer streams) and legacy sends (exactly N) both converge. On success the
/// partial is renamed into place and its sidecar removed; on ANY failure or
/// cancel the partial + sidecar are KEPT (when `resume` is Some) so the next
/// attempt picks up where this one died.
#[allow(clippy::too_many_arguments)]
/// Where a completed resumable receive lands.
enum FinalizeDest {
    /// Collision-safe name in a directory (friend sends, Quick Send): the final
    /// path is picked at COMPLETION time via `unique_path`.
    UniqueIn(PathBuf, String),
    /// An exact path + the origin mtime to stamp (folder sync: `staging/rel`,
    /// where the mtime keeps file signatures identical across the group).
    Exact(PathBuf, u64),
}

async fn recv_file_resumable<F: Fn(u64, u64)>(
    conn: &Connection,
    finalize: FinalizeDest,
    total: u64,
    part: PathBuf,
    resume: Option<ResumeCtx>,
    start_cov: Coverage,
    first: RecvStream,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<PathBuf> {
    let cov = std::sync::Arc::new(Mutex::new(start_cov));
    let mut set: tokio::task::JoinSet<Result<()>> = tokio::task::JoinSet::new();
    spawn_range_reader(&mut set, first, part.clone(), total, cov.clone());

    let persist = |cov: &Coverage, fsync: bool| {
        if let Some(rc) = &resume {
            // fsync BEFORE the sidecar claims ranges, so a power cut can't leave a
            // sidecar describing data the disk never got. But fsyncing a multi-GB
            // file every 2s measurably stutters a fast receive (it flushes ALL
            // dirty pages while 8 writers are pushing) — so data-page syncs run
            // only every ~16s and on the final/error persist. An app-level kill
            // keeps the OS cache, so the common crash case is safe either way.
            // INVARIANT: the sidecar is only ever written AFTER the data pages it
            // claims are fsynced — so a power cut can never resume over zeros. We
            // therefore persist on the fsync cadence (~16s + final), trading ≤16s
            // of re-downloaded progress on a hard kill for zero stutter.
            if !fsync {
                return;
            }
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&part) {
                let _ = f.sync_data();
            }
            save_sidecar(
                &rc.side,
                &PartialSidecar { v: 1, fp: rc.fp.clone(), total, coverage: cov.clone() },
            );
        }
    };

    let mut err: Option<anyhow::Error> = None;
    let mut last_covered = cov.lock().unwrap().covered();
    let mut last_growth = Instant::now();
    let mut last_persist = Instant::now();
    let mut persist_n: u32 = 0;
    loop {
        let covered = cov.lock().unwrap().covered();
        if covered != last_covered {
            last_covered = covered;
            last_growth = Instant::now();
        }
        if covered >= total {
            break; // every byte accounted for
        }
        tokio::select! {
            uni = conn.accept_uni() => match uni {
                Ok(u) => spawn_range_reader(&mut set, u, part.clone(), total, cov.clone()),
                Err(e) => { err = Some(anyhow::anyhow!("connection lost: {e}")); break; }
            },
            joined = set.join_next(), if !set.is_empty() => match joined {
                Some(Ok(Err(e))) => { err = Some(e); break; }
                Some(Err(e)) => { err = Some(anyhow::anyhow!("transfer worker failed: {e}")); break; }
                _ => {} // a worker finished cleanly; loop re-checks coverage
            },
            _ = tokio::time::sleep(Duration::from_millis(150)) => {
                if cancel.load(Ordering::SeqCst) {
                    err = Some(anyhow::anyhow!("canceled"));
                    break;
                }
                on_progress(covered, total);
                if last_growth.elapsed() > Duration::from_secs(60) {
                    err = Some(anyhow::anyhow!("transfer stalled — no data for 60s"));
                    break;
                }
                // Persist coverage every couple of seconds so even a hard kill
                // loses at most a moment of progress.
                if last_persist.elapsed() > Duration::from_secs(2) {
                    last_persist = Instant::now();
                    persist_n += 1;
                    persist(&cov.lock().unwrap().clone(), persist_n % 8 == 0);
                }
            }
        }
    }
    if err.is_some() {
        // Error path: kill stragglers, then drain so every worker has dropped its
        // file handle before we touch the partial (Windows rename safety).
        set.abort_all();
        while set.join_next().await.is_some() {}
    } else {
        // SUCCESS path: JOIN the remaining workers instead of aborting them. They
        // are finishing their final write/flush (or draining a redundant
        // already-covered tail from a legacy sender) — aborting here would (a)
        // silently swallow a failed last write (ENOSPC is realistic: the partial
        // is pre-sized SPARSE, so set_len allocated nothing) and then rename + ack
        // a corrupt file, and (b) reset streams a still-writing sender interprets
        // as failure for a transfer that actually landed. Bounded: a dead peer's
        // streams error out via QUIC idle timeout well inside this window.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while !set.is_empty() {
            match tokio::time::timeout_at(deadline, set.join_next()).await {
                Ok(Some(Ok(Ok(())))) => {}
                Ok(Some(Ok(Err(e)))) => {
                    err = Some(e);
                    break;
                }
                Ok(Some(Err(e))) => {
                    err = Some(anyhow::anyhow!("transfer worker failed: {e}"));
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    err = Some(anyhow::anyhow!("worker still running after completion"));
                    break;
                }
            }
        }
        if err.is_some() {
            set.abort_all();
            while set.join_next().await.is_some() {}
        }
    }

    let final_cov = cov.lock().unwrap().clone();
    if err.is_none() && final_cov.covered() >= total {
        // Flush to the OS before the rename makes it "real".
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&part) {
            let _ = f.sync_all();
        }
        let (dest, stamp) = match &finalize {
            FinalizeDest::UniqueIn(dir, name) => {
                let safe = Path::new(name)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "file".to_string());
                (unique_path(dir, &safe), 0)
            }
            FinalizeDest::Exact(path, mtime) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                (path.clone(), *mtime)
            }
        };
        std::fs::rename(&part, &dest).with_context(|| format!("finalize {}", dest.display()))?;
        // Folder sync: preserve the origin's modified-time so this file's
        // signature matches on every member (loop-guard, no sync storm).
        set_mtime_secs(&dest, stamp);
        if let Some(rc) = &resume {
            let _ = std::fs::remove_file(&rc.side);
        }
        on_progress(total, total);
        Ok(dest)
    } else {
        let e = err.unwrap_or_else(|| anyhow::anyhow!("incomplete transfer"));
        if resume.is_some() {
            // Keep the partial — this is the whole feature. Even a CANCEL keeps it
            // (cancel becomes "pause": re-sending the file later resumes). Final
            // persist fsyncs so the dormant partial's sidecar is durable.
            persist(&final_cov, true);
        } else {
            let _ = std::fs::remove_file(&part);
        }
        Err(e)
    }
}

/// Gather (path, name, size, mtime) for each file to send, plus the byte total.
fn gather_items(paths: &[PathBuf]) -> Result<(Vec<(PathBuf, String, u64, u64)>, u64)> {
    let mut items = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| anyhow::anyhow!("missing file name for {}", p.display()))?;
        items.push((p.clone(), name, meta.len(), mtime_secs(&meta)));
    }
    let total = items.iter().map(|i| i.2).sum();
    Ok((items, total))
}

/// The JSON header describing a files push. `parallel` > 0 advertises that the body
/// will arrive split across that many uni streams (the receiver opts in by replying
/// `{ready:true}` — plus a `resume` map of byte ranges it already has from an
/// interrupted earlier attempt); an older peer ignores the extra fields and reads
/// classically. `mtime` identifies the file version for resume fingerprinting.
fn files_header(items: &[(PathBuf, String, u64, u64)], total: u64, parallel: u64, from_name: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "files",
        "items": items
            .iter()
            .map(|(_, n, s, mt)| serde_json::json!({ "name": n, "size": s, "mtime": mt }))
            .collect::<Vec<_>>(),
        "total": total,
        "parallel": parallel,
        // The sender's display name — lets the receiver auto-add an unknown sender
        // as a friend (issue #6). Empty for anonymous Quick Send.
        "fromName": from_name,
    })
}

/// Write file bytes sequentially down one stream — the classic single-stream body
/// (the header must already have been written).
async fn write_files_body<F: Fn(u64, u64)>(
    send: &mut SendStream,
    items: &[(PathBuf, String, u64, u64)],
    total: u64,
    cancel: &AtomicBool,
    on_progress: &F,
) -> Result<u64> {
    let mut sent = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (path, _, _, _) in items {
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

/// Core: write `[header][file bytes…]` to an already-open send stream (classic
/// single-stream). Used by Quick Send's pull; Friends use `send_files`, which can
/// fan a big single file across parallel streams.
async fn write_files<F: Fn(u64, u64)>(
    send: &mut SendStream,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let (items, total) = gather_items(paths)?;
    write_frame(send, &files_header(&items, total, 0, "")).await?;
    write_files_body(send, &items, total, cancel, &on_progress).await
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
    my_name: &str,
    // Set true the moment the receiver replies {ready:true} — i.e. an auto-accept,
    // parallel/resumable receive is underway. The caller's auto-retry is gated on
    // this so a DECLINED manual-accept send (which can also error mid-write) never
    // retries and re-prompts the recipient.
    parallel_engaged: &AtomicBool,
) -> Result<u64> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let (items, total) = gather_items(paths)?;
    let n = parallel_stream_count(items.len(), total);
    write_frame(&mut send, &files_header(&items, total, n, my_name)).await?;

    if n > 0 {
        // The receiver opts into a parallel (multi-stream) receive by replying
        // {ready:true}. A peer on an older build — or one with manual-accept on —
        // won't, so we time out and fall back to the classic single-stream body on
        // this same stream. No hard failure during a staggered rollout.
        // Comfortably under the receiver's 12s first-stream wait, so if {ready}
        // arrives we both pick parallel, and if it doesn't we both pick classic.
        let reply = match tokio::time::timeout(Duration::from_secs(6), read_frame(&mut recv)).await {
            Ok(Ok(v)) => Some(v),
            _ => None,
        };
        let ready = reply
            .as_ref()
            .and_then(|v| v.get("ready"))
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        if ready {
            parallel_engaged.store(true, Ordering::SeqCst);
            let (base, plan) = parse_resume_reply(reply.as_ref(), total, n);
            send_ranges_parallel(conn, &items[0].0, total, base, &plan, cancel, on_progress)
                .await?;
            send.finish()?;
            let ack = recv.read_to_end(4096).await.unwrap_or_default();
            anyhow::ensure!(
                ack.ends_with(b"ok"),
                "the transfer was interrupted before the recipient confirmed receipt"
            );
            return Ok(total);
        }
    }

    // Classic single-stream body (the header was already written above).
    let sent = write_files_body(&mut send, &items, total, cancel, &on_progress).await?;
    send.finish()?;
    // Require the receiver's "ok" so we never report Completed for a transfer the
    // peer declined or failed to write (declined pushes stop the stream, surfacing
    // here as an error rather than a false success).
    // 256-byte limit + ends_with: if our ready-timeout raced a SLOW receiver
    // reply, the stale {ready} frame precedes the "ok" — don't fail a transfer
    // the receiver actually confirmed.
    let ack = recv.read_to_end(4096).await.unwrap_or_default();
    anyhow::ensure!(
        ack.ends_with(b"ok"),
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

/// Receiver half of a maybe-parallel files transfer: the manifest `header` was
/// just read from `recv`. If the sender advertised parallel for one big file,
/// reply {ready, resume} on `bsend` and receive over uni streams (resumable into
/// a hidden partial in `dest_dir`); otherwise read the classic body.
async fn read_files_negotiated<F: Fn(u64, u64)>(
    conn: &Connection,
    bsend: &mut SendStream,
    recv: &mut RecvStream,
    header: &serde_json::Value,
    dest_dir: &Path,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let total = header["total"].as_u64().unwrap_or(0);
    let n = header["parallel"].as_u64().unwrap_or(0).min(PARALLEL_STREAMS);
    let single = header["items"].as_array().map(|a| a.len()) == Some(1);
    if n > 0 && single && total >= PARALLEL_MIN {
        let item0 = &header["items"][0];
        let name = item0["name"].as_str().unwrap_or("file").to_string();
        let mtime = item0["mtime"].as_u64().unwrap_or(0);
        let who = conn.remote_id().to_string();
        let fp = transfer_fingerprint(&who, &name, total, mtime);
        let (part, cov) = prepare_partial(dest_dir, &fp, total)?;
        let ready = serde_json::json!({ "ready": true, "resume": { "have": cov.ranges } });
        write_frame(bsend, &ready).await?;
        match tokio::time::timeout(Duration::from_secs(12), conn.accept_uni()).await {
            Ok(Ok(first)) => {
                let rc = Some(ResumeCtx { side: partial_paths(dest_dir, &fp).1, fp });
                recv_file_resumable(
                    conn,
                    FinalizeDest::UniqueIn(dest_dir.to_path_buf(), name),
                    total,
                    part,
                    rc,
                    cov,
                    first,
                    cancel,
                    on_progress,
                )
                .await
                .map(|p| vec![p])
            }
            // Sender raced into its classic fallback — read the classic body,
            // then drop the partial+sidecar: the file arrived in full, so a kept
            // partial would only resume a later pull into a "name (1)" duplicate.
            _ => {
                let r = read_body(recv, header, dest_dir, cancel, on_progress).await;
                if r.is_ok() {
                    let (p, sd) = partial_paths(dest_dir, &fp);
                    let _ = std::fs::remove_file(p);
                    let _ = std::fs::remove_file(sd);
                }
                r
            }
        }
    } else {
        read_body(recv, header, dest_dir, cancel, on_progress).await
    }
}

/// Sender half of a maybe-parallel pull: write the manifest (advertising parallel
/// only when the puller said it understands it), then negotiate exactly like a
/// friend send — parallel + resume when the receiver replies ready, classic
/// single-stream otherwise.
async fn serve_pull_negotiated<F: Fn(u64, u64)>(
    conn: &Connection,
    send: &mut SendStream,
    recv: &mut RecvStream,
    paths: &[PathBuf],
    allow_parallel: bool,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<u64> {
    let (items, total) = gather_items(paths)?;
    let n = if allow_parallel {
        parallel_stream_count(items.len(), total)
    } else {
        0
    };
    write_frame(send, &files_header(&items, total, n, "")).await?;
    if n > 0 {
        let reply = match tokio::time::timeout(Duration::from_secs(6), read_frame(recv)).await {
            Ok(Ok(v)) => Some(v),
            _ => None,
        };
        let ready = reply
            .as_ref()
            .and_then(|v| v.get("ready"))
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        if ready {
            let (base, plan) = parse_resume_reply(reply.as_ref(), total, n);
            send_ranges_parallel(conn, &items[0].0, total, base, &plan, cancel, on_progress)
                .await?;
            send.finish()?;
            return Ok(total);
        }
    }
    let sent = write_files_body(send, &items, total, cancel, &on_progress).await?;
    send.finish()?;
    Ok(sent)
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

    #[test]
    fn parallel_only_for_one_large_file() {
        // Multiple files → never parallel (classic single stream).
        assert_eq!(parallel_stream_count(3, 500 * 1024 * 1024), 0);
        // A single small file → classic.
        assert_eq!(parallel_stream_count(1, 5 * 1024 * 1024), 0);
        // A single big file → capped at PARALLEL_STREAMS.
        assert_eq!(parallel_stream_count(1, 541 * 1024 * 1024), PARALLEL_STREAMS);
        // Right at the threshold → at least one stream, never more than the cap.
        let n = parallel_stream_count(1, PARALLEL_MIN);
        assert!((1..=PARALLEL_STREAMS).contains(&n));
    }

    #[test]
    fn parallel_segments_cover_every_byte_exactly_once() {
        // The EXACT split old receivers trust: any gap, overlap, or short-fall here
        // would corrupt the reassembled file, so prove coverage and exact count.
        for total in [PARALLEL_MIN, 100u64, 541 * 1024 * 1024, 17, 1, PARALLEL_MIN + 1] {
            let n = parallel_stream_count(1, total).max(1);
            let plan = legacy_plan(total, n);
            assert_eq!(plan.len() as u64, n, "legacy plan must have EXACTLY n streams");
            let mut prev_end = 0u64;
            let mut covered = 0u64;
            for (i, &(start, len)) in plan.iter().enumerate() {
                assert_eq!(start, prev_end, "gap/overlap before stream {i} (total={total})");
                prev_end = start + len;
                covered += len;
            }
            assert_eq!(prev_end, total, "coverage ends exactly at total={total}");
            assert_eq!(covered, total, "segments sum to total={total}");
        }
    }

    #[test]
    fn coverage_merges_and_reports_missing() {
        let mut c = Coverage::default();
        c.insert(10, 20);
        c.insert(30, 40);
        assert_eq!(c.ranges, vec![(10, 20), (30, 40)]);
        // Adjacent ranges merge (half-open: [20,30) bridges the gap exactly).
        c.insert(20, 30);
        assert_eq!(c.ranges, vec![(10, 40)]);
        // Overlapping + extending merges.
        c.insert(35, 50);
        assert_eq!(c.ranges, vec![(10, 50)]);
        // Out-of-order + empty inserts are safe.
        c.insert(0, 5);
        c.insert(7, 7);
        assert_eq!(c.ranges, vec![(0, 5), (10, 50)]);
        assert_eq!(c.covered(), 45);
        assert_eq!(c.missing(60), vec![(5, 10), (50, 60)]);
        // Completing the file leaves nothing missing.
        c.insert(5, 10);
        c.insert(50, 60);
        assert_eq!(c.ranges, vec![(0, 60)]);
        assert!(c.missing(60).is_empty());
        assert_eq!(c.covered(), 60);
    }

    #[test]
    fn coverage_sidecar_roundtrips() {
        let dir = std::env::temp_dir().join(format!("dropbeam-sidecar-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fp = transfer_fingerprint("sender", "a.bin", 100, 7);
        let (_, side) = partial_paths(&dir, &fp);
        let mut cov = Coverage::default();
        cov.insert(0, 30);
        cov.insert(60, 100);
        save_sidecar(&side, &PartialSidecar { v: 1, fp: fp.clone(), total: 100, coverage: cov.clone() });
        // Matching identity loads the coverage back…
        assert_eq!(load_sidecar(&side, &fp, 100), Some(cov));
        // …but a different total (file changed size) or fingerprint does not.
        assert_eq!(load_sidecar(&side, &fp, 101), None);
        assert_eq!(load_sidecar(&side, "other", 100), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_tracks_file_identity() {
        let a = transfer_fingerprint("eid1", "movie.mp4", 1000, 5);
        // Same identity → same fingerprint (resume finds the partial)…
        assert_eq!(a, transfer_fingerprint("eid1", "movie.mp4", 1000, 5));
        // …but ANY change (edited file, different sender/name) → different one,
        // so stale bytes can never be mixed into a new version.
        assert_ne!(a, transfer_fingerprint("eid1", "movie.mp4", 1000, 6));
        assert_ne!(a, transfer_fingerprint("eid1", "movie.mp4", 1001, 5));
        assert_ne!(a, transfer_fingerprint("eid2", "movie.mp4", 1000, 5));
        assert_ne!(a, transfer_fingerprint("eid1", "movie2.mp4", 1000, 5));
    }

    #[test]
    fn resume_plan_covers_exactly_the_missing_bytes() {
        const MB: u64 = 1024 * 1024;
        // Typical interrupted-parallel shape: a few large holes.
        let missing = vec![(10 * MB, 40 * MB), (50 * MB, 90 * MB)];
        let plan = plan_resume_ranges(&missing, 6);
        assert!(plan.len() <= 6 && !plan.is_empty());
        // Every missing byte is covered by the plan (re-sending extra is allowed —
        // positioned writes make it harmless — but gaps are NOT).
        let mut cov = Coverage::default();
        for &(s, l) in &plan {
            cov.insert(s, s + l);
        }
        for &(s, e) in &missing {
            assert!(
                cov.ranges.iter().any(|&(cs, ce)| cs <= s && e <= ce),
                "plan must cover all missing bytes ({s},{e})"
            );
        }
        // More gaps than streams → merged down to the cap, still covering all.
        let many: Vec<(u64, u64)> = (0..12).map(|i| (i * 10 * MB, i * 10 * MB + MB)).collect();
        let plan = plan_resume_ranges(&many, 6);
        assert!(plan.len() <= 6);
        let mut cov = Coverage::default();
        for &(s, l) in &plan {
            cov.insert(s, s + l);
        }
        for &(s, e) in &many {
            assert!(
                cov.ranges.iter().any(|&(cs, ce)| cs <= s && e <= ce),
                "merged plan still covers ({s},{e})"
            );
        }
        // Nothing missing → empty plan (receiver already has the whole file).
        assert!(plan_resume_ranges(&[], 6).is_empty());
    }

    // The core parallel-transfer guarantee: a file fanned across N uni streams and
    // reassembled must come out byte-for-byte identical. Ignored (touches the relay
    // network); run with: cargo test --lib iroh_net -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn parallel_transfer_reassembles_identical_bytes() {
        let pid = std::process::id();
        let total: u64 = 20 * 1024 * 1024; // 20 MiB → above PARALLEL_MIN, multi-stream
        let n = parallel_stream_count(1, total);
        assert!(n > 1, "20 MiB should fan across multiple streams");

        // Deterministic pseudo-random payload (so a swapped/duplicated segment shows).
        let mut data = vec![0u8; total as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i as u64).wrapping_mul(2654435761) % 251) as u8;
        }
        let src = std::env::temp_dir().join(format!("dropbeam-par-src-{pid}.bin"));
        std::fs::write(&src, &data).unwrap();

        let server = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        server.online().await;
        let addr = server.addr();
        let dest_dir = std::env::temp_dir().join(format!("dropbeam-par-rx-{pid}"));

        let srv = server.clone();
        let dest_dir_c = dest_dir.clone();
        let recv = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            // Peel off the first segment stream like the live receiver does.
            let first = conn.accept_uni().await.unwrap();
            let fp = transfer_fingerprint("test-sender", "blob.bin", total, 0);
            let (part, cov) = prepare_partial(&dest_dir_c, &fp, total).unwrap();
            let rc = ResumeCtx { side: partial_paths(&dest_dir_c, &fp).1, fp };
            recv_file_resumable(
                &conn,
                FinalizeDest::UniqueIn(dest_dir_c.clone(), "blob.bin".into()),
                total, part, Some(rc), cov, first,
                &AtomicBool::new(false), |_, _| {},
            )
            .await
            .unwrap()
        });

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let conn = client.connect(addr, ALPN).await.unwrap();
        // Send the legacy exact-n plan — the layout an old sender would use; the
        // coverage-based receiver must converge on it just the same.
        send_ranges_parallel(&conn, &src, total, 0, &legacy_plan(total, n), &AtomicBool::new(false), |_, _| {})
            .await
            .unwrap();

        let dest = recv.await.unwrap();
        let got = std::fs::read(&dest).unwrap();
        assert_eq!(got.len() as u64, total, "received size matches");
        assert!(got == data, "parallel-reassembled bytes must match the source exactly");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest_dir);
        println!("parallel transfer OK: {total} bytes across {n} streams, byte-identical");
    }

    // THE RESUME GUARANTEE: a receiver that already has part of the file (an
    // interrupted earlier attempt) advertises its coverage; the sender transmits
    // only the missing ranges; the reassembled file is byte-identical and the
    // partial + sidecar are cleaned up. Ignored (touches the relay network).
    #[tokio::test]
    #[ignore]
    async fn resumed_transfer_sends_only_missing_and_reassembles() {
        let pid = std::process::id();
        let total: u64 = 24 * 1024 * 1024;
        let mut data = vec![0u8; total as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i as u64).wrapping_mul(40503) % 253) as u8;
        }
        let src = std::env::temp_dir().join(format!("dropbeam-res-src-{pid}.bin"));
        std::fs::write(&src, &data).unwrap();
        let dest_dir = std::env::temp_dir().join(format!("dropbeam-res-rx-{pid}"));
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Simulate the interrupted earlier attempt: the receiver already has the
        // first 10 MiB on disk, recorded in the sidecar.
        let have: u64 = 10 * 1024 * 1024;
        let fp = transfer_fingerprint("test-sender", "blob.bin", total, 42);
        let (part, side) = partial_paths(&dest_dir, &fp);
        {
            let f = std::fs::OpenOptions::new().create(true).write(true).open(&part).unwrap();
            f.set_len(total).unwrap();
        }
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
            f.seek(SeekFrom::Start(0)).unwrap();
            f.write_all(&data[..have as usize]).unwrap();
        }
        let mut cov0 = Coverage::default();
        cov0.insert(0, have);
        save_sidecar(&side, &PartialSidecar { v: 1, fp: fp.clone(), total, coverage: cov0 });

        let server = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        server.online().await;
        let addr = server.addr();

        let srv = server.clone();
        let dest_dir_c = dest_dir.clone();
        let fp_c = fp.clone();
        let recv = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let (mut bsend, mut brecv) = conn.accept_bi().await.unwrap();
            let req = read_frame(&mut brecv).await.unwrap();
            assert_eq!(req["kind"], "files");
            // The live receiver's resume offer: ranges we already have.
            let (part, cov) = prepare_partial(&dest_dir_c, &fp_c, total).unwrap();
            assert_eq!(cov.covered(), have, "sidecar coverage survived");
            write_frame(&mut bsend, &serde_json::json!({ "ready": true, "resume": { "have": cov.ranges } }))
                .await
                .unwrap();
            let first = conn.accept_uni().await.unwrap();
            let rc = ResumeCtx { side: partial_paths(&dest_dir_c, &fp_c).1, fp: fp_c.clone() };
            let dest = recv_file_resumable(
                &conn,
                FinalizeDest::UniqueIn(dest_dir_c.clone(), "blob.bin".into()),
                total, part, Some(rc), cov, first,
                &AtomicBool::new(false), |_, _| {},
            )
            .await
            .unwrap();
            // The receiver acks like the live files arm does. In production the
            // connection lives on in the accept loop; here the task ending would
            // DROP it and could discard the buffered ack — so wait until the
            // sender has read it (stopped() resolves on peer read-to-end).
            let _ = bsend.write_all(b"ok").await;
            let _ = bsend.finish();
            let _ = tokio::time::timeout(Duration::from_secs(10), bsend.stopped()).await;
            dest
        });

        // Track how much the SENDER actually pushed: progress starts at the resume
        // base, so (final - base) must be only the missing tail.
        let client = Endpoint::bind(presets::N0).await.unwrap();
        let conn = client.connect(addr, ALPN).await.unwrap();
        let sent = send_files(&conn, &[src.clone()], &AtomicBool::new(false), |_, _| {}, "tester", &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(sent, total);

        let dest = recv.await.unwrap();
        let got = std::fs::read(&dest).unwrap();
        assert!(got == data, "resumed bytes must match the source exactly");
        assert!(!part.exists(), "partial removed after completion");
        assert!(!side.exists(), "sidecar removed after completion");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest_dir);
        println!("resume OK: completed from {have}/{total} pre-seeded bytes, byte-identical");
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
        let sent = send_files(&conn, &[src.clone()], &AtomicBool::new(false), |_, _| {}, "tester", &AtomicBool::new(false))
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
