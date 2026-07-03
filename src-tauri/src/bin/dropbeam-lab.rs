//! dropbeam-lab — two-machine transfer test driver (dev tool, never shipped).
//!
//! Runs REAL DropBeam engine transfers (`send_files`/`recv_files`, production
//! endpoint preset, same ALPN) between two processes or two machines, and prints
//! machine-readable JSON lines so an automated runner can verify byte-identity
//! and measure speed on the direct path, the relay path, or auto.
//!
//! Receiver:  dropbeam-lab serve [--dest <dir>]
//!     prints `{"event":"ready","addr":"lab..."}` then one JSON line per
//!     completed inbound transfer (files, rel paths, sha256s, bytes, ms).
//!
//! Sender:    dropbeam-lab send --to <labADDR> [--mode auto|direct|relay]
//!                [--suite quick|full|big] [--parallel on|off] [--dir <corpus>]
//!     builds the corpus, runs each case over a fresh connection, prints one
//!     JSON line per case with the local (expected) sha256s + throughput.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use app_lib::labkit;
use serde_json::json;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn emit(v: serde_json::Value) {
    // One JSON object per line; the runner parses stdout line-by-line.
    println!("{v}");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve(&args).await,
        Some("send") => send(&args).await,
        Some("results") => results(&args).await,
        _ => {
            eprintln!(
                "usage:\n  dropbeam-lab serve [--dest <dir>]\n  dropbeam-lab send --to <labADDR> [--mode auto|direct|relay] [--suite quick|full|big] [--parallel on|off] [--dir <corpus>]\n  dropbeam-lab results --to <labADDR>"
            );
            std::process::exit(2);
        }
    }
}

async fn serve(args: &[String]) -> Result<()> {
    let dest_root = flag(args, "--dest")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-recv"));
    std::fs::create_dir_all(&dest_root)?;

    let ep = labkit::lab_endpoint(true).await?;
    let addr = labkit::lab_addr_ready(&ep).await;
    emit(json!({
        "event": "ready",
        "addr": labkit::encode_addr(&addr)?,
        "id": addr.id.to_string(),
        "dest": dest_root.display().to_string(),
    }));

    // Every completed receive is BOTH printed (local runs) and kept in memory so
    // the runner can pull it over the results ALPN (cross-machine runs, no SSH).
    let results: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));

    let mut n: u64 = 0;
    // Accept forever; each inbound connection is one lab case. Ctrl-C to stop.
    while let Some(incoming) = ep.accept().await {
        n += 1;
        let idx = n;
        let dest = dest_root.join(format!("conn-{idx:03}"));
        let results = results.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let result: Result<serde_json::Value> = async {
                let conn = incoming.await.context("accept connection")?;
                if conn.alpn() == labkit::LAB_RESULTS_ALPN {
                    // Runner pulling results: reply with everything so far.
                    let (mut s, mut r) = conn.accept_bi().await?;
                    let _ = r.read_to_end(64).await; // drain the "get" request
                    let body = serde_json::to_vec(&*results.lock().unwrap())?;
                    s.write_all(&body).await?;
                    s.finish()?;
                    let _ = s.stopped().await;
                    return Ok(json!({"event": "results-served", "conn": idx}));
                }
                std::fs::create_dir_all(&dest)?;
                // Negotiated receive = the same path the app's handlers run, so a
                // parallel-advertised big file takes the real resumable route.
                let engaged = AtomicBool::new(false);
                let got = labkit::recv_files_negotiated(
                    &conn,
                    &dest,
                    &AtomicBool::new(false),
                    &engaged,
                    |_, _| {},
                )
                .await?;
                let ms = started.elapsed().as_millis() as u64;
                let hashes = labkit::sha256_tree(&dest)?;
                let bytes: u64 = hashes
                    .iter()
                    .filter_map(|(rel, _)| std::fs::metadata(dest.join(rel)).ok())
                    .map(|m| m.len())
                    .sum();
                Ok(json!({
                    "event": "received",
                    "conn": idx,
                    "files": got.len(),
                    "bytes": bytes,
                    "ms": ms,
                    "parallelEngaged": engaged.load(std::sync::atomic::Ordering::Relaxed),
                    "hashes": hashes.iter().map(|(rel, h)| json!({"rel": rel, "sha256": h})).collect::<Vec<_>>(),
                }))
            }
            .await;
            let line = match result {
                Ok(v) => v,
                Err(e) => json!({"event": "recv-error", "conn": idx, "error": e.to_string()}),
            };
            emit(line.clone());
            if line["event"] != "results-served" {
                results.lock().unwrap().push(line);
            }
        });
    }
    Ok(())
}

/// Pull the receiver's accumulated results over iroh (the no-SSH path for the
/// second machine) and print them as one JSON array.
async fn results(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let peer = labkit::decode_addr(&to)?;
    let ep = labkit::lab_endpoint(false).await?;
    let conn = ep
        .connect(peer, labkit::LAB_RESULTS_ALPN)
        .await
        .context("dial peer results channel")?;
    let (mut s, mut r) = conn.open_bi().await?;
    s.write_all(b"get").await?;
    s.finish()?;
    let body = r.read_to_end(64 * 1024 * 1024).await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

async fn send(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let mode = flag(args, "--mode").unwrap_or_else(|| "auto".into());
    let suite = flag(args, "--suite").unwrap_or_else(|| "quick".into());
    let parallel = flag(args, "--parallel").unwrap_or_else(|| "on".into()) != "off";
    let corpus_dir = flag(args, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-corpus"));

    labkit::set_parallel_streams(parallel);
    let peer = labkit::filter_addr(labkit::decode_addr(&to)?, &mode);
    if peer.addrs.is_empty() {
        bail!("peer addr has no {mode} transport addresses — can't force that path");
    }

    let ep = labkit::lab_endpoint(false).await?;
    let cases = labkit::build_corpus(&corpus_dir, &suite)?;
    emit(json!({
        "event": "start",
        "mode": mode, "suite": suite, "parallel": parallel,
        "cases": cases.len(),
        "corpus": corpus_dir.display().to_string(),
    }));

    let mut failed = 0u32;
    for case in &cases {
        let started = Instant::now();
        let result: Result<serde_json::Value> = async {
            let conn = ep
                .connect(peer.clone(), labkit::ALPN)
                .await
                .context("dial peer")?;
            let path_start = labkit::conn_detail(&conn);
            let engaged = AtomicBool::new(false);
            let sent = labkit::send_files(
                &conn,
                &case.paths,
                &AtomicBool::new(false),
                |_, _| {},
                "dropbeam-lab",
                &engaged,
            )
            .await?;
            let ms = started.elapsed().as_millis().max(1) as u64;
            // Expected hashes: what the receiver's tree must contain for this case.
            let mut hashes = Vec::new();
            for p in &case.paths {
                if p.is_dir() {
                    let base = p.file_name().unwrap_or_default().to_string_lossy();
                    for (rel, h) in labkit::sha256_tree(p)? {
                        hashes.push((format!("{base}/{rel}"), h));
                    }
                } else {
                    let name = p.file_name().unwrap_or_default().to_string_lossy();
                    hashes.push((name.into_owned(), labkit::sha256_file(p)?));
                }
            }
            hashes.sort();
            Ok(json!({
                "event": "sent",
                "case": case.name,
                "bytes": sent,
                "ms": ms,
                "mbps": (sent as f64 / (1024.0 * 1024.0)) / (ms as f64 / 1000.0),
                "parallelEngaged": engaged.load(std::sync::atomic::Ordering::Relaxed),
                // Path the QUIC connection was on at dial time vs after the
                // transfer — shows relay→direct upgrades and hairpin routes.
                "pathStart": path_start,
                "pathEnd": labkit::conn_detail(&conn),
                "hashes": hashes.iter().map(|(rel, h)| json!({"rel": rel, "sha256": h})).collect::<Vec<_>>(),
            }))
        }
        .await;
        match result {
            Ok(v) => emit(v),
            Err(e) => {
                failed += 1;
                emit(json!({"event": "send-error", "case": case.name, "error": e.to_string()}));
            }
        }
    }
    emit(json!({"event": "done", "failed": failed}));
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
