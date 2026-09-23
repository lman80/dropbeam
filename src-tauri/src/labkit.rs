//! Two-machine test lab support — the surface the `dropbeam-lab` binary drives.
//!
//! The lab runs REAL end-to-end transfers between two machines (or two processes)
//! through the exact engine code the shipping app uses (`send_files`/`recv_files`,
//! same ALPN, same production endpoint preset), so anything it proves or measures
//! is true of the app itself. It is a dev-only binary: never bundled, never shipped,
//! zero effect on the app.
//!
//! Design notes:
//!  - The endpoint uses `presets::N0` — identical relays + discovery to production —
//!    so a "relay" run rides the same public relay real internet transfers do.
//!  - Dial modes work by FILTERING the peer's advertised addresses before
//!    connecting: `direct` keeps only IP addrs (LAN/WAN hole-punch path), `relay`
//!    keeps only the relay URL (forces every byte through the relay even on the
//!    same LAN), `auto` leaves the full set (production behavior).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iroh::endpoint::presets;
use iroh::TransportAddr;
pub use iroh::EndpointAddr;
use sha2::{Digest, Sha256};

pub use crate::iroh_net::{
    conn_detail, lab_landed_rel, lab_manifest, recv_files, recv_files_negotiated, send_files,
    send_like_friend, serve_friend_conn, set_parallel_streams, ALPN,
};
pub use iroh::endpoint::Connection;
pub use iroh::Endpoint;


/// Side-channel ALPN the lab receiver answers on: the runner dials it to pull
/// the receiver's accumulated JSON results over iroh itself — no SSH, no file
/// copying from the second machine.
pub const LAB_RESULTS_ALPN: &[u8] = b"dropbeam-lab/results";

/// The runner streams a freshly-built receiver binary here; the receiver stages
/// it and re-execs. This is what makes the test→fix loop autonomous — I never
/// have to ask the user to re-copy the tester.
pub const LAB_UPDATE_ALPN: &[u8] = b"dropbeam-lab/update";

/// Small ALPN returning the running receiver's build stamp, so the runner can
/// confirm a pushed update actually took effect before re-testing.
pub const LAB_INFO_ALPN: &[u8] = b"dropbeam-lab/info";

/// Build stamp compiled into this binary. Set `LAB_BUILD` in the environment at
/// build time (the loop does); falls back to "dev" for a plain `cargo build`.
pub const LAB_BUILD: &str = match option_env!("LAB_BUILD") {
    Some(v) => v,
    None => "dev",
};

/// Load or create the receiver's persistent identity so its node id — and thus
/// the lab code the user pasted once — stays STABLE across self-update restarts.
/// Ports change on restart; the stable node id + relay in the encoded addr let
/// the runner reconnect anyway (relay rendezvous + hole-punch, same as the app).
pub fn lab_secret(state_dir: &Path) -> iroh::SecretKey {
    let path = state_dir.join("lab-identity.key");
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return iroh::SecretKey::from_bytes(&seed);
        }
    }
    let seed: [u8; 32] = rand::random();
    let _ = std::fs::create_dir_all(state_dir);
    if std::fs::write(&path, seed).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    iroh::SecretKey::from_bytes(&seed)
}

/// Bind a lab endpoint with the PRODUCTION preset (default relays + discovery)
/// AND the production transport tuning — BBR congestion control + 8 MB windows
/// (see `iroh_net::start`). Without this the lab measures quinn's CUBIC
/// defaults, which crawl on lossy Wi-Fi, not what the app actually does.
/// `accept` registers the app ALPN (plus the lab results channel) so peers can
/// dial us.
pub async fn lab_endpoint(accept: bool) -> Result<Endpoint> {
    lab_endpoint_inner(accept, None).await
}

/// Like `lab_endpoint`, but persists identity under `state_dir` so the node id
/// survives self-update restarts. Used by `serve`.
pub async fn lab_endpoint_persistent(accept: bool, state_dir: &Path) -> Result<Endpoint> {
    lab_endpoint_inner(accept, Some(state_dir)).await
}

/// A SENDER endpoint for one dial mode. `relay` removes every IP transport, so
/// the connection can't hole-punch its way off the relay mid-run (filtering the
/// peer's addrs alone still lets iroh upgrade to direct) — every byte provably
/// rides the public relay, the path a user behind a hostile NAT gets.
pub async fn lab_endpoint_for(mode: &str) -> Result<Endpoint> {
    if mode != "relay" { return lab_endpoint(false).await; }
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.congestion_controller_factory(std::sync::Arc::new(noq_proto::congestion::Bbr3Config::default()));
    tcfg = tcfg.stream_receive_window((8u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(8 * 1024 * 1024);
    Endpoint::builder(presets::N0)
        .clear_ip_transports()
        .path_selector(std::sync::Arc::new(crate::iroh_net::DirectPathSelector))
        .transport_config(tcfg.build())
        .bind().await.context("bind relay-only lab endpoint")
}

async fn lab_endpoint_inner(accept: bool, state_dir: Option<&Path>) -> Result<Endpoint> {
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.congestion_controller_factory(std::sync::Arc::new(
        noq_proto::congestion::Bbr3Config::default(),
    ));
    tcfg = tcfg.stream_receive_window((8u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(8 * 1024 * 1024);
    let mut b = Endpoint::builder(presets::N0)
        .path_selector(std::sync::Arc::new(crate::iroh_net::DirectPathSelector))
        .transport_config(tcfg.build());
    if let Some(dir) = state_dir {
        b = b.secret_key(lab_secret(dir));
    }
    if accept {
        b = b.alpns(vec![
            ALPN.to_vec(),
            LAB_RESULTS_ALPN.to_vec(),
            LAB_UPDATE_ALPN.to_vec(),
            LAB_INFO_ALPN.to_vec(),
        ]);
    }
    b.bind().await.context("bind lab iroh endpoint")
}

/// Wait (bounded) until the endpoint has learned enough of its own addresses to be
/// dialable cross-machine: at least one IP addr, and ideally a relay. Returns the
/// best addr we managed to learn — the caller prints/encodes it for the peer.
pub async fn lab_addr_ready(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..40 {
        let addr = ep.addr();
        let has_ip = addr.addrs.iter().any(|a| matches!(a, TransportAddr::Ip(_)));
        let has_relay = addr.addrs.iter().any(|a| matches!(a, TransportAddr::Relay(_)));
        if has_ip && has_relay {
            return addr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    ep.addr() // best effort — direct-only still works on a LAN
}

/// Encode an EndpointAddr as a single copy-paste token (same base64-JSON scheme as
/// the app's Quick Send ticket, different prefix so the two can't be confused).
pub fn encode_addr(addr: &EndpointAddr) -> Result<String> {
    use base64::Engine as _;
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(addr)?);
    Ok(format!("lab{body}"))
}

pub fn decode_addr(s: &str) -> Result<EndpointAddr> {
    use base64::Engine as _;
    let body = s.trim().strip_prefix("lab").unwrap_or(s.trim());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .context("lab addr is not valid base64")?;
    serde_json::from_slice(&bytes).context("lab addr is not a valid EndpointAddr")
}

/// Restrict a peer addr to one path family so a run PROVABLY exercises that path.
/// `mode`: "direct" (IP only), "relay" (relay only), anything else = auto (full set).
pub fn filter_addr(addr: EndpointAddr, mode: &str) -> EndpointAddr {
    let keep = |a: &TransportAddr| match mode {
        "direct" => matches!(a, TransportAddr::Ip(_)),
        "relay" => matches!(a, TransportAddr::Relay(_)),
        _ => true,
    };
    EndpointAddr {
        id: addr.id,
        addrs: addr.addrs.into_iter().filter(keep).collect(),
    }
}

/// Operator endpoint for driving Lab Mode: a PERSISTENT identity (so the node id
/// the user pastes into each device's "Lab operator" field stays stable) with the
/// app ALPN as a CLIENT (we dial devices, they don't dial us). Reuses the same
/// production transport as the app so path behavior matches.
pub async fn operator_endpoint(state_dir: &Path) -> Result<Endpoint> {
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.congestion_controller_factory(std::sync::Arc::new(
        noq_proto::congestion::Bbr3Config::default(),
    ));
    tcfg = tcfg.stream_receive_window((8u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(8 * 1024 * 1024);
    Endpoint::builder(presets::N0)
        .path_selector(std::sync::Arc::new(crate::iroh_net::DirectPathSelector))
        .secret_key(lab_secret(state_dir))
        .transport_config(tcfg.build())
        .bind()
        .await
        .context("bind operator endpoint")
}

/// Dial a device's REAL app endpoint by node id and run one Lab Mode command.
/// `cmd` + `extra` fields form the request; returns the device's JSON reply.
/// The device only answers if its Lab Mode is on and this operator's node id is
/// the one it trusts — otherwise the reply is `{ok:false,error:"unauthorized"}`.
pub async fn lab_call(
    ep: &Endpoint,
    node_id: &str,
    cmd: &str,
    extra: serde_json::Value,
) -> Result<serde_json::Value> {
    let id: iroh::EndpointId = node_id.trim().parse().context("parse device node id")?;
    let conn = ep
        .connect(iroh::EndpointAddr::from(id), ALPN)
        .await
        .context("dial device app endpoint")?;
    let (mut s, mut r) = conn.open_bi().await?;
    let mut req = serde_json::json!({ "kind": "lab", "cmd": cmd });
    if let serde_json::Value::Object(map) = extra {
        for (k, v) in map {
            req[k] = v;
        }
    }
    // Frame: [u32 BE len][json], same as the app's control protocol.
    let body = serde_json::to_vec(&req)?;
    use tokio::io::AsyncWriteExt;
    s.write_all(&(body.len() as u32).to_be_bytes()).await?;
    s.write_all(&body).await?;
    s.finish()?;
    let bytes = r.read_to_end(128 * 1024 * 1024).await?;
    // Reply is also a length-prefixed frame.
    anyhow::ensure!(bytes.len() >= 4, "short lab reply");
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let json = &bytes[4..4 + len.min(bytes.len() - 4)];
    let reply: serde_json::Value = serde_json::from_slice(json).context("parse lab reply")?;
    let _ = conn;
    Ok(reply)
}

/// sha256 of a file, hex — the byte-identity check both sides report.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// One landed (or expected) file: `/`-separated rel, sha256, size, mtime secs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FileReport { pub rel: String, pub sha256: String, pub size: u64, pub mtime: u64 }

/// Every regular file under `root` — hidden ones too (a stray stage/partial is
/// a finding), symlinks NOT followed — with its hash, size and mtime.
pub fn tree_report(root: &Path) -> Result<Vec<FileReport>> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<FileReport>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let p = entry.path();
            if ft.is_dir() { walk(&p, root, out)?; continue; }
            if !ft.is_file() { continue; }
            let meta = entry.metadata()?;
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string();
            // A '\\' is a legal name character on macOS/Linux, a separator only on Windows.
            #[cfg(windows)]
            let rel = rel.replace('\\', "/");
            use unicode_normalization::UnicodeNormalization;
            out.push(FileReport { rel: rel.nfc().collect(), sha256: sha256_file(&p)?, size: meta.len(), mtime: lab_mtime(&meta) });
        }
        Ok(())
    }
    let mut out = Vec::new();
    if root.exists() { walk(root, root, &mut out)?; }
    out.sort();
    Ok(out)
}
use crate::iroh_net::lab_mtime;

/// Every directory under `root` (NFC, `/`-separated) — lets the runner check
/// that advertised empty folders were recreated.
pub fn dir_report(root: &Path) -> Vec<String> {
    use unicode_normalization::UnicodeNormalization;
    fn walk(d: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().to_string();
                #[cfg(windows)]
                let rel = rel.replace('\\', "/");
                out.push(rel.nfc().collect());
                walk(&e.path(), root, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// What a macOS/Linux receiver must end up with for `paths`: the engine's own
/// manifest, each rel mapped through the receiver's landing rule, hashed from
/// the source. Symlinks, dotfiles and OS junk the engine skips are absent here
/// too — so a sender↔receiver difference is a real bug, not corpus noise.
pub fn expected_report(paths: &[PathBuf]) -> Result<(Vec<FileReport>, Vec<String>)> {
    use unicode_normalization::UnicodeNormalization;
    let (items, dirs) = lab_manifest(paths)?;
    let mut out = Vec::new();
    for (src, rel, size, mtime) in items {
        out.push(FileReport { rel: lab_landed_rel(&rel).nfc().collect(), sha256: sha256_file(&src)?, size, mtime });
    }
    out.sort();
    Ok((out, dirs.iter().map(|d| lab_landed_rel(d)).collect()))
}

/// Write `len` bytes of the deterministic `payload` pattern WITHOUT holding it
/// in memory (multi-GB fixtures), then stamp a fixed past mtime.
pub fn write_payload_file(path: &Path, len: u64, seed: u64) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut f = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(path)?);
    let mut buf = vec![0u8; 8 << 20];
    let mut i: u64 = 0;
    while i < len {
        let n = (len - i).min(buf.len() as u64) as usize;
        for (k, b) in buf[..n].iter_mut().enumerate() {
            *b = ((i + k as u64).wrapping_mul(2654435761).wrapping_add(seed) % 251) as u8;
        }
        f.write_all(&buf[..n])?;
        i += n as u64;
    }
    f.flush()?;
    drop(f);
    crate::iroh_net::set_mtime_secs(path, 1_600_000_000 + seed * 7919);
    Ok(())
}

/// sha256 of every FILE under `root` (recursive), keyed by rel path with `/`
/// separators — so sender corpus and receiver output compare across machines.
pub fn sha256_tree(root: &Path) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                walk(&p, root, out)?;
            } else if p.is_file() {
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, sha256_file(&p)?));
            }
        }
        Ok(())
    }
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Deterministic pseudo-random payload (same scheme as the loopback tests): any
/// swapped/duplicated/dropped segment changes the bytes, so byte-equality is a real
/// reassembly check, not just a length check.
pub fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut v = vec![0u8; len];
    for (i, b) in v.iter_mut().enumerate() {
        *b = ((i as u64).wrapping_mul(2654435761).wrapping_add(seed) % 251) as u8;
    }
    v
}

/// One named test case: the paths to send (files and/or folders) rooted in `dir`.
/// `loose`: names may legitimately land differently on a case-insensitive or
/// name-colliding receiver ("README (1).md"), so the verdict compares the
/// multiset of (sha256, size, mtime) instead of exact names.
pub struct LabCase {
    pub name: &'static str,
    pub paths: Vec<PathBuf>,
    pub loose: bool,
}

fn case(name: &'static str, paths: Vec<PathBuf>) -> LabCase { LabCase { name, paths, loose: false } }

/// Build the corpus for a suite under `dir`. Suites:
///  quick — the everyday shapes (single, 60 small, odd names, nested tree)
///  full  — quick + a 256 MiB parallel file
///  big   — full + 1 GiB;  huge — one streamed multi-GB file (LAB_HUGE_GIB, default 3)
///  edge  — every size boundary + hostile-name case the engine must survive,
///          each VERIFIED against the receiver (names, bytes, mtimes, no strays)
///  many / mixed / torture2 — scale + collision discovery
pub fn build_corpus(dir: &Path, suite: &str) -> Result<Vec<LabCase>> {
    // Fresh every run: a leftover file from an older corpus would be sent too.
    let _ = make_writable(dir);
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let mut cases: Vec<LabCase> = Vec::new();
    let file = |rel: &str, len: usize, seed: u64| -> Result<PathBuf> {
        let p = dir.join(rel);
        write_payload_file(&p, len as u64, seed)?;
        Ok(p)
    };

    if matches!(suite, "quick" | "full" | "big") {
        cases.push(case("single-1mib", vec![file("single.bin", 1 << 20, 1)?]));
        cases.push(case("batch-60-small",
            (0..60).map(|i| file(&format!("small/f{i:03}.bin"), 4096 + i * 13, 100 + i as u64)).collect::<Result<Vec<_>>>()?));
        cases.push(case("odd-names", vec![
            file("héllo wörld 🚀.bin", 8192, 7)?,
            file("name with  spaces.txt", 5000, 8)?,
            file("empty.bin", 0, 9)?,
        ]));
        let tree = dir.join("Tree");
        std::fs::create_dir_all(tree.join("sub/deep"))?;
        std::fs::create_dir_all(tree.join("empty-dir"))?;
        file("Tree/root.bin", 65536, 20)?;
        file("Tree/sub/mid.bin", 131072, 21)?;
        file("Tree/sub/deep/leaf.bin", 32768, 22)?;
        cases.push(case("folder-tree", vec![tree]));
    }
    if suite == "full" || suite == "big" {
        cases.push(case("big-256mib", vec![file("big.bin", 256 << 20, 42)?]));
    }
    if suite == "big" {
        cases.push(case("huge-1gib", vec![file("huge.bin", 1 << 30, 43)?]));
    }
    if suite == "huge" {
        let gib: u64 = std::env::var("LAB_HUGE_GIB").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
        let p = dir.join(format!("huge-{gib}gib.bin"));
        write_payload_file(&p, gib << 30, 44)?;
        cases.push(case("huge-streamed", vec![p]));
    }

    if suite == "edge" {
        use unicode_normalization::UnicodeNormalization;
        const MIB: usize = 1 << 20;
        // Sizes: empty, 1 byte, the 1 MiB I/O chunk ±1, the 4 MiB integrity
        // block / small-file limit ±1 — one batch (friend packing path).
        cases.push(case("sizes-boundaries", vec![
            file("sizes/empty.bin", 0, 60)?, file("sizes/one.bin", 1, 61)?,
            file("sizes/chunk-1.bin", MIB - 1, 62)?, file("sizes/chunk.bin", MIB, 63)?, file("sizes/chunk+1.bin", MIB + 1, 64)?,
            file("sizes/block-1.bin", 4 * MIB - 1, 65)?, file("sizes/block.bin", 4 * MIB, 66)?, file("sizes/block+1.bin", 4 * MIB + 1, 67)?,
        ]));
        cases.push(case("empty-alone", vec![file("alone/empty-alone.bin", 0, 68)?]));
        // The 16 MiB parallel threshold, each file alone (classic vs parallel).
        cases.push(case("parallel-under", vec![file("par/under.bin", 16 * MIB - 1, 69)?]));
        cases.push(case("parallel-exact", vec![file("par/exact.bin", 16 * MIB, 70)?]));
        cases.push(case("parallel-over", vec![file("par/over.bin", 16 * MIB + 1, 71)?]));
        // Unicode: NFD (what macOS hands a sender), emoji, RTL, whitespace.
        let nfd: String = "Café 한국어 ñ.txt".nfd().collect();
        cases.push(case("names-unicode", vec![
            file(&format!("uni/{nfd}"), 4096, 72)?, file("uni/🚀📦 emoji.bin", 4097, 73)?,
            file("uni/مرحبا بالعالم.txt", 4098, 74)?, file("uni/שלום.txt", 4099, 75)?,
            file("uni/  leading spaces.txt", 100, 76)?, file("uni/trailing space .txt", 101, 77)?,
            file("uni/tab\there.txt", 102, 78)?, file("uni/new\nline.txt", 103, 79)?,
        ]));
        // Names Windows can't hold (a Mac/Linux receiver keeps them verbatim).
        cases.push(case("names-windows-illegal", vec![
            file("win/Report 7:3.pdf", 200, 80)?, file("win/star*.txt", 201, 81)?, file("win/q?.txt", 202, 82)?,
            file("win/quote\".txt", 203, 83)?, file("win/lt<gt>.txt", 204, 84)?, file("win/pipe|.txt", 205, 85)?,
            file("win/back\\slash.txt", 206, 86)?, file("win/CON", 207, 87)?, file("win/NUL.txt", 208, 88)?,
            file("win/COM1.tar.gz", 209, 89)?, file("win/trailing dot.", 210, 90)?,
        ]));
        // Max-length names (255 bytes ASCII, 254 bytes multibyte).
        cases.push(case("long-names", vec![
            file(&format!("long/{}.bin", "L".repeat(251)), 4096, 91)?,
            file(&format!("long/{}.bin", "é".repeat(125)), 4096, 92)?,
        ]));
        // 50 levels deep with empty dirs; a long (but locally legal) path.
        let mut deep = String::from("Nest");
        for i in 0..50 { deep.push_str(&format!("/lvl{i:02}")); }
        file(&format!("{deep}/bottom.bin"), 8192, 93)?;
        std::fs::create_dir_all(dir.join(format!("{deep}/empty-bottom")))?;
        std::fs::create_dir_all(dir.join("Nest/lvl00/empty-mid"))?;
        cases.push(case("deep-nest-50", vec![dir.join("Nest")]));
        let room = if cfg!(target_os = "macos") { 1000usize.saturating_sub(2 * dir.as_os_str().len()) } else { 1400 };
        let mut lp = String::from("LongPath");
        while lp.len() + 210 < room { lp.push('/'); lp.push_str(&"P".repeat(200)); }
        file(&format!("{lp}/leaf.bin"), 5000, 94)?;
        cases.push(case("long-path", vec![dir.join("LongPath")]));
        // Hidden + OS junk inside a folder: only the visible file travels.
        for (rel, n) in [("Dots/visible.txt", 95u64), ("Dots/.env", 96), ("Dots/.git/config", 97), ("Dots/.DS_Store", 98),
                         ("Dots/._visible.txt", 99), ("Dots/Thumbs.db", 100), ("Dots/desktop.ini", 101)] {
            file(rel, 300 + n as usize, n)?;
        }
        cases.push(case("dotfiles-folder", vec![dir.join("Dots")]));
        cases.push(case("dotfile-direct", vec![file(".secrets.bin", 4096, 102)?]));
        let dotdir = dir.join(".configdir");
        file(".configdir/inner.bin", 4096, 103)?;
        cases.push(case("dotfolder", vec![dotdir]));
        // Packages are directories (exec bit must survive).
        let tool = file("Pkg/Tool.app/Contents/MacOS/Tool", 5000, 104)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))?; }
        file("Pkg/Tool.app/Contents/Info.plist", 300, 105)?;
        file("Pkg/Lib.photoslibrary/database/Photos.sqlite", 7000, 106)?;
        std::fs::create_dir_all(dir.join("Pkg/Lib.photoslibrary/resources/empty"))?;
        cases.push(case("packages", vec![dir.join("Pkg/Tool.app"), dir.join("Pkg/Lib.photoslibrary")]));
        // Read-only source.
        let ro = file("ro/locked.txt", 4000, 107)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444))?; }
        cases.push(case("read-only", vec![ro]));
        // Symlinks inside a folder are never followed; a directly-chosen link
        // sends what it points at.
        #[cfg(unix)]
        {
            let abs = std::fs::canonicalize(dir)?;
            file("outside/secret.txt", 99, 108)?;
            file("Links/real.txt", 50, 109)?;
            let _ = std::os::unix::fs::symlink(abs.join("outside/secret.txt"), dir.join("Links/file-link"));
            let _ = std::os::unix::fs::symlink(abs.join("outside"), dir.join("Links/dir-link"));
            let _ = std::os::unix::fs::symlink(abs.join("gone"), dir.join("Links/dangling"));
            let _ = std::os::unix::fs::symlink("..", dir.join("Links/loop"));
            cases.push(case("symlinks-in-folder", vec![dir.join("Links")]));
            file("linktarget.bin", 4096, 110)?;
            let link = dir.join("direct-link.bin");
            let _ = std::os::unix::fs::symlink(abs.join("linktarget.bin"), &link);
            cases.push(case("symlink-direct", vec![link]));
        }
        // A file and a folder with the same stem side by side.
        file("TypeClash/thing/inside.bin", 4096, 111)?;
        file("TypeClash/thing.bin", 4096, 112)?;
        cases.push(case("type-clash", vec![dir.join("TypeClash")]));
        // Collisions: the SAME landed name from two sources. Names can differ on
        // a case-insensitive receiver ("README (1).md"); bytes may not.
        cases.push(LabCase { name: "collide-same-leaf", loose: true, paths: vec![
            file("c1/report.pdf", 1000, 113)?, file("c2/report.pdf", 2000, 114)?] });
        cases.push(LabCase { name: "collide-case-only", loose: true, paths: vec![
            file("c3/README.md", 1100, 115)?, file("c4/readme.md", 1200, 116)?] });
        cases.push(LabCase { name: "collide-dotstrip", loose: true, paths: vec![
            file(".config.bin", 4096, 117)?, file("config.bin", 5000, 118)?] });
        // Scale: 1500 tiny files over 37 folders + an empty one.
        for i in 0..1500u64 { file(&format!("Tiny/d{:02}/f{i:04}.txt", i % 37), (i % 97) as usize, 1000 + i)?; }
        std::fs::create_dir_all(dir.join("Tiny/zz-empty"))?;
        cases.push(case("tiny-1500", vec![dir.join("Tiny")]));
        cases.push(case("zeros-8mib", vec![{ let z = dir.join("zeros.bin"); std::fs::write(&z, vec![0u8; 8 << 20])?; z }]));
    }

    if suite == "many" {
        let many = dir.join("Many");
        for i in 0..400 { file(&format!("Many/doc{i:04}.bin"), 1024 + (i % 16) * 1024, 200 + i as u64)?; }
        cases.push(case("many-400", vec![many]));
    }
    if suite == "torture2" {
        cases.push(LabCase { name: "dotstrip-collision", loose: true, paths: vec![file(".config.bin", 4096, 70)?, file("config.bin", 5000, 71)?] });
        cases.push(case("boundary-16mib", vec![file("edge16.bin", 16 * 1024 * 1024, 72)?]));
        cases.push(case("boundary-under", vec![file("under16.bin", 16 * 1024 * 1024 - 1, 73)?]));
        file("TypeClash/thing/inside.bin", 4096, 74)?;
        file("TypeClash/thing.bin", 4096, 75)?;
        cases.push(case("type-clash", vec![dir.join("TypeClash")]));
        for i in 0..2000 { file(&format!("Scale2000/s{i:04}.bin"), 512 + (i % 8) * 256, 400 + i as u64)?; }
        cases.push(case("scale-2000", vec![dir.join("Scale2000")]));
    }
    if suite == "mixed" {
        let mut paths = vec![file("mixed-big.bin", 300 << 20, 60)?];
        for i in 0..50 { paths.push(file(&format!("mixed-small-{i:02}.bin"), 4096 + i * 7, 300 + i as u64)?); }
        cases.push(case("mixed-batch", paths));
    }
    anyhow::ensure!(!cases.is_empty(), "unknown suite {suite:?} (quick|full|big|huge|edge|many|mixed|torture2)");
    Ok(cases)
}

/// Make a corpus tree writable again so a re-run can delete read-only fixtures.
pub fn make_writable(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() { return Ok(()); }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(if meta.is_dir() { 0o755 } else { 0o644 }))?;
        if meta.is_dir() { for e in std::fs::read_dir(dir)?.flatten() { let _ = make_writable(&e.path()); } }
    }
    Ok(())
}

/// Compare what landed with what had to land. Returns the problems (empty =
/// PASS). `loose` compares content only (names may be collision-renamed).
pub fn verdict(expected: &[FileReport], dirs: &[String], landed: &[FileReport], dirs_present: impl Fn(&str) -> bool, loose: bool) -> Vec<String> {
    let mut problems = Vec::new();
    if loose {
        let key = |f: &FileReport| (f.sha256.clone(), f.size, f.mtime);
        let mut want: Vec<_> = expected.iter().map(key).collect();
        let mut got: Vec<_> = landed.iter().map(key).collect();
        want.sort(); got.sort();
        if want != got { problems.push(format!("content mismatch: expected {} files, landed {:?}", want.len(), landed.iter().map(|f| &f.rel).collect::<Vec<_>>())); }
    } else {
        for w in expected {
            match landed.iter().find(|l| l.rel == w.rel) {
                None => problems.push(format!("missing: {:?}", w.rel)),
                Some(l) if l.sha256 != w.sha256 || l.size != w.size => problems.push(format!("bytes differ: {:?}", w.rel)),
                Some(l) if l.mtime != w.mtime => problems.push(format!("mtime {} != {}: {:?}", l.mtime, w.mtime, w.rel)),
                _ => {}
            }
        }
        for l in landed {
            if !expected.iter().any(|w| w.rel == l.rel) { problems.push(format!("unexpected: {:?}", l.rel)); }
        }
    }
    for d in dirs { if !dirs_present(d) { problems.push(format!("empty dir missing: {d:?}")); } }
    problems
}

/// A controllable Locations HOST for testing a client (e.g. the iOS simulator)
/// without touching anybody's real DropBeam config: `config` is a scratch config
/// dir, `folder` the directory to share, `friend_eid` the client's endpoint id.
/// Shares `folder` as "Lab NAS" (browse + upload + manage) with that friend only
/// and serves the REAL app protocol (`accept_loop`) until the process exits.
/// Downloads need the full app (they start a tracked send), so a lab host
/// answers them with an error; list/ls/mkdir/rename/trash/upload are real.
/// Returns this host's friend code for the client to add.
pub fn host_location(ep: &Endpoint, config: &Path, folder: &Path, friend_eid: &str, host_name: &str) -> Result<String> {
    std::fs::create_dir_all(config)?;
    let friend = crate::friends::upsert_by_endpoint(config, friend_eid, "Lab client");
    let path = std::fs::canonicalize(folder).context("shared folder")?;
    let location = crate::locations::Location {
        id: "lab-nas".into(), name: "Lab NAS".into(), path: path.to_string_lossy().into_owned(),
        friend_ids: vec![friend.id], rights: crate::locations::Rights { upload: true, manage: true },
        byte_cap: crate::locations::default_byte_cap(), device: None, marker: None, safe_publish: None,
    };
    crate::locations::save(config, Some(location), None)?;
    // Advertise on the LAN like the app does, so a simulator/phone on this
    // network finds the host without relay/DNS rendezvous.
    if let (Ok(mdns), Ok(al)) = (iroh_mdns_address_lookup::MdnsAddressLookup::builder().build(ep.id()), ep.address_lookup()) { al.add(mdns); }
    let state = std::sync::Arc::new(crate::iroh_net::IrohState::default());
    let _ = state.location_config.set(config.to_path_buf());
    tokio::spawn(crate::iroh_net::accept_loop(ep.clone(), state));
    Ok(crate::friends::my_code(host_name, &ep.id().to_string()))
}
