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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, SecretKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::OnceCell;

/// Application-layer protocol id. Bumped if the wire format changes.
pub const ALPN: &[u8] = b"dropbeam/1";

/// Shared, lazily-initialised iroh endpoint, managed by Tauri as `Arc<IrohState>`.
/// The boot task fills `endpoint` once the node is up; commands await it.
#[derive(Default)]
pub struct IrohState {
    pub endpoint: OnceCell<Endpoint>,
}

impl IrohState {
    /// The endpoint once it's ready (None during the brief startup window).
    pub fn get(&self) -> Option<&Endpoint> {
        self.endpoint.get()
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
    let ep = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .context("bind iroh endpoint")?;
    Ok(ep)
}

/// Accept incoming connections forever, dispatching each to the protocol handler.
/// Runs for the life of the app. Errors on a single connection are logged, never
/// fatal.
pub async fn accept_loop(ep: Endpoint) {
    while let Some(incoming) = ep.accept().await {
        tauri::async_runtime::spawn(async move {
            match incoming.await {
                Ok(conn) => handle_conn(conn).await,
                Err(e) => log::debug!("iroh: incoming connection failed: {e}"),
            }
        });
    }
}

/// Per-connection handler. Phase 1 speaks one trivial protocol — echo — so the
/// self-test can prove a round trip. Later phases branch on a stream header.
async fn handle_conn(conn: Connection) {
    let who = conn.remote_id();
    loop {
        match conn.accept_bi().await {
            Ok((mut send, mut recv)) => {
                tauri::async_runtime::spawn(async move {
                    // Echo up to 64 KiB (self-test payloads are tiny).
                    if let Ok(data) = recv.read_to_end(64 * 1024).await {
                        let _ = send.write_all(&data).await;
                        let _ = send.finish();
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
    send.write_all(b"dropbeam-ping").await?;
    send.finish()?;
    let echoed = recv.read_to_end(64).await.context("read echo")?;
    let ok = echoed == b"dropbeam-ping";
    let id = main.id().to_string();
    client.close().await;
    if ok {
        Ok(format!("ok · node {}…{}", &id[..6], &id[id.len() - 4..]))
    } else {
        anyhow::bail!("echo mismatch: got {:?}", String::from_utf8_lossy(&echoed))
    }
}

/// Spawn the endpoint at app startup and keep accepting connections. Fills
/// `state.endpoint` once bound. Safe to fail — croc remains the live transport.
pub fn spawn(config_dir: std::path::PathBuf, state: Arc<IrohState>) {
    tauri::async_runtime::spawn(async move {
        match start(&config_dir).await {
            Ok(ep) => {
                log::info!("iroh endpoint up: {}", ep.id());
                let _ = state.endpoint.set(ep.clone());
                accept_loop(ep).await;
            }
            Err(e) => log::warn!("iroh endpoint failed to start: {e:#}"),
        }
    });
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
    on_progress: F,
) -> Result<u64> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let sent = write_files(&mut send, paths, on_progress).await?;
    send.finish()?;
    let _ = recv.read_to_end(16).await;
    Ok(sent)
}

/// PUSH receive: accept the peer's next stream and write its files to `dest_dir`.
pub async fn recv_files<F: Fn(u64, u64)>(
    conn: &Connection,
    dest_dir: &Path,
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let out = read_files(&mut recv, dest_dir, on_progress).await?;
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
pub fn make_ticket(ep: &Endpoint, token: &str) -> Result<String> {
    use base64::Engine as _;
    let addr = ep.addr();
    let v = serde_json::json!({ "addr": addr, "token": token });
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&v)?))
}

fn parse_ticket(s: &str) -> Result<(iroh::EndpointAddr, String)> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim())
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
    on_progress: F,
) -> Result<Vec<PathBuf>> {
    let (addr, token) = parse_ticket(ticket)?;
    let conn = client.connect(addr, ALPN).await.context("dial ticket")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &serde_json::json!({ "kind": "pull", "token": token })).await?;
    let out = read_files(&mut recv, dest_dir, on_progress).await?;
    Ok(out)
}

/// Sender side: a pull request arrived on `send`/`recv`; push `paths`.
pub async fn serve_pull<F: Fn(u64, u64)>(
    send: &mut SendStream,
    paths: &[PathBuf],
    on_progress: F,
) -> Result<u64> {
    let sent = write_files(send, paths, on_progress).await?;
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
            recv_files(&conn, &dest_c, |_, _| {}).await.unwrap()
        });

        let src = std::env::temp_dir().join(format!("dropbeam-iroh-src-{pid}.bin"));
        let data = vec![0xABu8; 5 * 1024 * 1024];
        std::fs::write(&src, &data).unwrap();

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let conn = client.connect(addr, ALPN).await.unwrap();
        let sent = send_files(&conn, &[src.clone()], |_, _| {}).await.unwrap();
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
            serve_pull(&mut send, &staged, |_, _| {}).await.unwrap();
        });

        let client = Endpoint::bind(presets::N0).await.unwrap();
        let dest = std::env::temp_dir().join(format!("dropbeam-pull-rx-{pid}"));
        let got = pull_files(&client, &ticket, &dest, |_, _| {}).await.unwrap();
        serve.await.unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), data);
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir_all(&dest);
        println!("iroh Quick Send (pull) OK: ticket {} chars", ticket.len());
    }
}
