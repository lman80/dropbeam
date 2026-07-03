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
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use sha2::{Digest, Sha256};

pub use crate::iroh_net::{
    conn_detail, recv_files, recv_files_negotiated, send_files, set_parallel_streams, ALPN,
};
pub use iroh::endpoint::Connection;

/// Side-channel ALPN the lab receiver answers on: the runner dials it to pull
/// the receiver's accumulated JSON results over iroh itself — no SSH, no file
/// copying from the second machine.
pub const LAB_RESULTS_ALPN: &[u8] = b"dropbeam-lab/results";

/// Bind a lab endpoint with the PRODUCTION preset (default relays + discovery)
/// AND the production transport tuning — BBR congestion control + 8 MB windows
/// (see `iroh_net::start`). Without this the lab measures quinn's CUBIC
/// defaults, which crawl on lossy Wi-Fi, not what the app actually does.
/// `accept` registers the app ALPN (plus the lab results channel) so peers can
/// dial us.
pub async fn lab_endpoint(accept: bool) -> Result<Endpoint> {
    let mut tcfg = iroh::endpoint::QuicTransportConfig::builder();
    tcfg = tcfg.congestion_controller_factory(std::sync::Arc::new(
        noq_proto::congestion::BbrConfig::default(),
    ));
    tcfg = tcfg.stream_receive_window((8u32 * 1024 * 1024).into());
    tcfg = tcfg.send_window(8 * 1024 * 1024);
    let mut b = Endpoint::builder(presets::N0).transport_config(tcfg.build());
    if accept {
        b = b.alpns(vec![ALPN.to_vec(), LAB_RESULTS_ALPN.to_vec()]);
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

/// sha256 of a file, hex — the byte-identity check both sides report.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
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
pub struct LabCase {
    pub name: &'static str,
    pub paths: Vec<PathBuf>,
}

/// Build the corpus for a suite under `dir`. Cases cover the shapes that have
/// historically broken: single files, many-small batches, a big parallel-streams
/// file, unicode/odd names, empty files, and a nested folder tree.
pub fn build_corpus(dir: &Path, suite: &str) -> Result<Vec<LabCase>> {
    std::fs::create_dir_all(dir)?;
    let mut cases: Vec<LabCase> = Vec::new();
    let file = |rel: &str, len: usize, seed: u64| -> Result<PathBuf> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, payload(len, seed))?;
        Ok(p)
    };

    // quick + full
    cases.push(LabCase { name: "single-1mib", paths: vec![file("single.bin", 1 << 20, 1)?] });
    cases.push(LabCase {
        name: "batch-60-small",
        paths: (0..60)
            .map(|i| file(&format!("small/f{i:03}.bin"), 4096 + i * 13, 100 + i as u64))
            .collect::<Result<Vec<_>>>()?,
    });
    cases.push(LabCase {
        name: "odd-names",
        paths: vec![
            file("héllo wörld 🚀.bin", 8192, 7)?,
            file("name with  spaces.txt", 5000, 8)?,
            file("empty.bin", 0, 9)?,
        ],
    });
    // A folder WITH nested structure, sent as one folder path (exercises
    // gather_items recursion + empty-dir recreation).
    let tree = dir.join("Tree");
    std::fs::create_dir_all(tree.join("sub/deep"))?;
    std::fs::create_dir_all(tree.join("empty-dir"))?;
    file("Tree/root.bin", 65536, 20)?;
    file("Tree/sub/mid.bin", 131072, 21)?;
    file("Tree/sub/deep/leaf.bin", 32768, 22)?;
    cases.push(LabCase { name: "folder-tree", paths: vec![tree] });

    if suite == "full" || suite == "big" {
        // Big single file → the parallel-streams path (threshold-dependent).
        cases.push(LabCase { name: "big-256mib", paths: vec![file("big.bin", 256 << 20, 42)?] });
    }
    if suite == "big" {
        cases.push(LabCase { name: "huge-1gib", paths: vec![file("huge.bin", 1 << 30, 43)?] });
    }

    // "edge" = hostile-input discovery suite. Sender hashes describe what's on
    // the sender's disk; receiver hashes show what actually landed — DIFFERENCES
    // ARE THE FINDINGS (renames, drops, collisions), not automatic failures.
    if suite == "edge" {
        cases.clear();
        // Explicitly-sent dotfile: receiver sanitize_rel strips the leading dot.
        cases.push(LabCase { name: "dotfile-direct", paths: vec![file(".secrets.bin", 4096, 50)?] });
        // Dot-named FOLDER sent explicitly (children are normal names).
        let dotdir = dir.join(".configdir");
        std::fs::create_dir_all(&dotdir)?;
        file(".configdir/inner.bin", 4096, 51)?;
        cases.push(LabCase { name: "dotfolder", paths: vec![dotdir] });
        // Colon names — what a Finder name like "Report 7/3" becomes on disk.
        // TWO of them so a sanitize-to-same-name collision shows up too.
        cases.push(LabCase {
            name: "colon-names",
            paths: vec![file("Report 7:3.bin", 5000, 52)?, file("Data 1:2.bin", 6000, 53)?],
        });
        // NFD-decomposed Korean filename (what macOS's file system reports).
        {
            use unicode_normalization::UnicodeNormalization;
            let nfd: String = "한국어파일.bin".nfd().collect();
            cases.push(LabCase { name: "korean-nfd", paths: vec![file(&nfd, 4096, 54)?] });
        }
        // 254-byte filename (APFS limit is 255).
        let long = format!("{}.bin", "L".repeat(250));
        cases.push(LabCase { name: "long-name", paths: vec![file(&long, 4096, 55)?] });
        // Newline inside a filename (legal on macOS).
        cases.push(LabCase { name: "newline-name", paths: vec![file("two\nlines.bin", 4096, 56)?] });
        // 20-deep nesting with an empty dir mid-tree.
        let mut deep = String::from("Deep");
        for i in 0..20 {
            deep.push_str(&format!("/level{i:02}"));
        }
        file(&format!("{deep}/bottom.bin"), 8192, 57)?;
        std::fs::create_dir_all(dir.join("Deep/level00/empty-here"))?;
        cases.push(LabCase { name: "deep-nest", paths: vec![dir.join("Deep")] });
        // Symlinks INSIDE a sent folder: one live (to a sibling), one dangling.
        #[cfg(unix)]
        {
            let sl = dir.join("Symlinks");
            std::fs::create_dir_all(&sl)?;
            file("Symlinks/real.bin", 4096, 58)?;
            // Absolute targets: a relative target would resolve against the LINK's
            // dir and dangle, testing our corpus instead of the engine.
            let abs = std::fs::canonicalize(dir)?;
            let _ = std::os::unix::fs::symlink(
                abs.join("Symlinks/real.bin"),
                sl.join("alias-to-real"),
            );
            let _ =
                std::os::unix::fs::symlink(abs.join("Symlinks/gone.bin"), sl.join("dangling"));
            cases.push(LabCase { name: "symlink-folder", paths: vec![sl] });
            // A symlink passed DIRECTLY as the dropped path.
            file("linktarget.bin", 4096, 59)?;
            let link = dir.join("direct-link.bin");
            let _ = std::os::unix::fs::symlink(abs.join("linktarget.bin"), &link);
            cases.push(LabCase { name: "symlink-direct", paths: vec![link] });
        }
        // 8 MiB of zeros — degenerate content.
        let z = dir.join("zeros.bin");
        std::fs::write(&z, vec![0u8; 8 << 20])?;
        cases.push(LabCase { name: "zeros-8mib", paths: vec![z] });
    }

    // "many" = per-file overhead: 400 small files in one folder.
    if suite == "many" {
        cases.clear();
        let many = dir.join("Many");
        std::fs::create_dir_all(&many)?;
        for i in 0..400 {
            file(&format!("Many/doc{i:04}.bin"), 1024 + (i % 16) * 1024, 200 + i as u64)?;
        }
        cases.push(LabCase { name: "many-400", paths: vec![many] });
    }

    // "mixed" = one big file + many smalls in a single batch: multi-item batches
    // never go parallel, so this measures the classic path carrying bulk.
    if suite == "mixed" {
        cases.clear();
        let mut paths = vec![file("mixed-big.bin", 300 << 20, 60)?];
        for i in 0..50 {
            paths.push(file(&format!("mixed-small-{i:02}.bin"), 4096 + i * 7, 300 + i as u64)?);
        }
        cases.push(LabCase { name: "mixed-batch", paths });
    }
    Ok(cases)
}
