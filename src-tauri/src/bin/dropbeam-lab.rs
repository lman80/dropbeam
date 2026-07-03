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

/// Close the endpoint gracefully before the process exits. Dropping it abruptly
/// cancels iroh's background actor tasks, which panic-print "task N was
/// cancelled" onto stdout — corrupting the JSON the runner parses. `close()`
/// shuts those down cleanly first.
async fn shutdown(ep: &app_lib::labkit::Endpoint) {
    ep.close().await;
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve(&args).await,
        Some("send") => send(&args).await,
        Some("results") => results(&args).await,
        Some("info") => info(&args).await,
        Some("push-update") => push_update(&args).await,
        Some("version") => {
            println!("{}", labkit::LAB_BUILD);
            Ok(())
        }
        _ => {
            eprintln!(
                "usage:\n  dropbeam-lab serve [--dest <dir>] [--state <dir>]\n  dropbeam-lab send --to <labADDR> [--mode auto|direct|relay] [--suite quick|full|big|edge|many|mixed] [--only <case>] [--parallel on|off] [--profile] [--dir <corpus>]\n  dropbeam-lab results --to <labADDR>\n  dropbeam-lab info --to <labADDR>\n  dropbeam-lab push-update --to <labADDR> --bin <path>"
            );
            std::process::exit(2);
        }
    }
}

/// Exit code the supervisor script watches for: "I staged an update, swap the
/// binary and relaunch me." Any other exit = real stop (Ctrl-C, crash).
const EXIT_UPDATE: i32 = 42;

async fn serve(args: &[String]) -> Result<()> {
    let dest_root = flag(args, "--dest")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-recv"));
    std::fs::create_dir_all(&dest_root)?;
    // Persistent identity dir → stable lab code across self-update restarts.
    let state_dir = flag(args, "--state")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".dropbeam-lab"))
                .unwrap_or_else(|_| std::env::temp_dir().join("dropbeam-lab-state"))
        });
    std::fs::create_dir_all(&state_dir)?;
    // Where a pushed update is staged for the supervisor to swap in.
    let staged_update = state_dir.join("dropbeam-lab.new");
    let _ = std::fs::remove_file(&staged_update); // clear any stale staging

    let ep = labkit::lab_endpoint_persistent(true, &state_dir).await?;
    let addr = labkit::lab_addr_ready(&ep).await;
    emit(json!({
        "event": "ready",
        "addr": labkit::encode_addr(&addr)?,
        "id": addr.id.to_string(),
        "dest": dest_root.display().to_string(),
        "build": labkit::LAB_BUILD,
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
        let staged_update = staged_update.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let result: Result<serde_json::Value> = async {
                let conn = incoming.await.context("accept connection")?;
                if conn.alpn() == labkit::LAB_RESULTS_ALPN {
                    // Runner pulling results: reply with everything so far. A "reset"
                    // request additionally CLEARS the accumulator so the next test
                    // round starts clean (the runner sends it before each round).
                    let (mut s, mut r) = conn.accept_bi().await?;
                    let req = r.read_to_end(64).await.unwrap_or_default();
                    let body = serde_json::to_vec(&*results.lock().unwrap())?;
                    s.write_all(&body).await?;
                    s.finish()?;
                    let _ = s.stopped().await;
                    if req == b"reset" {
                        results.lock().unwrap().clear();
                        return Ok(json!({"event": "results-reset", "conn": idx}));
                    }
                    return Ok(json!({"event": "results-served", "conn": idx}));
                }
                if conn.alpn() == labkit::LAB_INFO_ALPN {
                    // Build-stamp probe — the runner confirms an update took.
                    let (mut s, mut r) = conn.accept_bi().await?;
                    let _ = r.read_to_end(64).await;
                    s.write_all(labkit::LAB_BUILD.as_bytes()).await?;
                    s.finish()?;
                    let _ = s.stopped().await;
                    return Ok(json!({"event": "info-served", "conn": idx}));
                }
                if conn.alpn() == labkit::LAB_UPDATE_ALPN {
                    // Runner streamed a fresh binary. Stage it atomically, ack,
                    // then exit(42) so the supervisor swaps it in and relaunches.
                    let (mut s, mut r) = conn.accept_bi().await?;
                    let bytes = r.read_to_end(256 * 1024 * 1024).await?;
                    anyhow::ensure!(bytes.len() > 1_000_000, "update binary implausibly small");
                    let tmp = staged_update.with_extension("part");
                    std::fs::write(&tmp, &bytes)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
                    }
                    // Rename is atomic — the supervisor only ever sees a complete file.
                    std::fs::rename(&tmp, &staged_update)?;
                    s.write_all(b"ok").await?;
                    s.finish()?;
                    let _ = s.stopped().await;
                    emit(json!({"event": "update-staged", "bytes": bytes.len(), "conn": idx}));
                    // Give the ack a beat to flush, then hand off to the supervisor.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    std::process::exit(EXIT_UPDATE);
                }
                // Clean any stale files first: conn numbering resets each launch,
                // so without this a prior session's conn-001 contents leak into a
                // new receive and the runner's hash tree compares garbage.
                let _ = std::fs::remove_dir_all(&dest);
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
    // --reset clears the receiver's accumulator after this pull (round boundary).
    let req: &[u8] = if args.iter().any(|a| a == "--reset") { b"reset" } else { b"get" };
    let peer = labkit::decode_addr(&to)?;
    let ep = labkit::lab_endpoint(false).await?;
    let conn = ep
        .connect(peer, labkit::LAB_RESULTS_ALPN)
        .await
        .context("dial peer results channel")?;
    let (mut s, mut r) = conn.open_bi().await?;
    s.write_all(req).await?;
    s.finish()?;
    let body = r.read_to_end(64 * 1024 * 1024).await?;
    println!("{}", String::from_utf8_lossy(&body));
    shutdown(&ep).await;
    Ok(())
}

/// Print the running receiver's build stamp (blank line if unreachable). Used by
/// the loop to confirm a pushed update took effect before re-testing.
async fn info(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let peer = labkit::decode_addr(&to)?;
    let ep = labkit::lab_endpoint(false).await?;
    let conn = ep
        .connect(peer, labkit::LAB_INFO_ALPN)
        .await
        .context("dial peer info channel")?;
    let (mut s, mut r) = conn.open_bi().await?;
    s.write_all(b"?").await?;
    s.finish()?;
    let body = r.read_to_end(4096).await?;
    println!("{}", String::from_utf8_lossy(&body));
    shutdown(&ep).await;
    Ok(())
}

/// Stream a freshly-built receiver binary to the running receiver. It stages the
/// bytes and re-execs; the caller then polls `info` until the new build stamp
/// appears. This is the wire that makes test→fix→test fully autonomous.
async fn push_update(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let bin = flag(args, "--bin").context("--bin <path> is required")?;
    let bytes = std::fs::read(&bin).with_context(|| format!("read {bin}"))?;
    anyhow::ensure!(bytes.len() > 1_000_000, "binary at {bin} looks too small");
    let peer = labkit::decode_addr(&to)?;
    let ep = labkit::lab_endpoint(false).await?;
    let conn = ep
        .connect(peer, labkit::LAB_UPDATE_ALPN)
        .await
        .context("dial peer update channel")?;
    let (mut s, mut r) = conn.open_bi().await?;
    s.write_all(&bytes).await?;
    s.finish()?;
    let ack = r.read_to_end(64).await.unwrap_or_default();
    anyhow::ensure!(ack == b"ok", "receiver did not confirm the update");
    emit(json!({"event": "update-sent", "bytes": bytes.len()}));
    shutdown(&ep).await;
    Ok(())
}

async fn send(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let mode = flag(args, "--mode").unwrap_or_else(|| "auto".into());
    let suite = flag(args, "--suite").unwrap_or_else(|| "quick".into());
    let parallel = flag(args, "--parallel").unwrap_or_else(|| "on".into()) != "off";
    let profile = args.iter().any(|a| a == "--profile");
    let corpus_dir = flag(args, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-corpus"));

    labkit::set_parallel_streams(parallel);
    let peer = labkit::filter_addr(labkit::decode_addr(&to)?, &mode);
    if peer.addrs.is_empty() {
        bail!("peer addr has no {mode} transport addresses — can't force that path");
    }

    let ep = labkit::lab_endpoint(false).await?;
    let mut cases = labkit::build_corpus(&corpus_dir, &suite)?;
    if let Some(only) = flag(args, "--only") {
        cases.retain(|c| c.name == only);
    }
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
            // --profile: sample (elapsed_ms, bytes_confirmed) roughly every 2s so
            // a long transfer's rate-over-time shape is visible (decay vs sawtooth).
            let samples = Mutex::new(Vec::<(u64, u64)>::new());
            let sent = labkit::send_files(
                &conn,
                &case.paths,
                &AtomicBool::new(false),
                |done, _| {
                    if profile {
                        let t = started.elapsed().as_millis() as u64;
                        let mut s = samples.lock().unwrap();
                        if s.last().map(|(lt, _)| t - lt >= 2000).unwrap_or(true) {
                            s.push((t, done));
                        }
                    }
                },
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
                "profile": *samples.lock().unwrap(),
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
    shutdown(&ep).await;
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
