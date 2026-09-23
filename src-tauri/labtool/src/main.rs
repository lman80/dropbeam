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
        Some("host-location") => host_location(&args).await,
        Some("send") => send(&args).await,
        Some("results") => results(&args).await,
        Some("info") => info(&args).await,
        Some("push-update") => push_update(&args).await,
        Some("lab-id") => lab_id(&args).await,
        Some("lab") => lab_cmd(&args).await,
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

/// Host a scratch folder as a Locations share for ONE client endpoint (e.g. the
/// iOS simulator) through the real app protocol; prints the friend code to add.
///   dropbeam-lab host-location --dir <folder> --friend <client eid> [--state <dir>]
async fn host_location(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(flag(args, "--dir").context("--dir <folder> required")?);
    let friend = flag(args, "--friend").context("--friend <client endpoint id> required")?;
    let state_dir = PathBuf::from(flag(args, "--state").unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-host").display().to_string()));
    std::fs::create_dir_all(&dir)?;
    let ep = labkit::lab_endpoint_persistent(true, &state_dir).await?;
    let _ = labkit::lab_addr_ready(&ep).await;
    let code = labkit::host_location(&ep, &state_dir.join("config"), &dir, &friend, "Lab Host")?;
    emit(json!({"event": "hosting", "id": ep.id().to_string(), "code": code, "dir": dir.display().to_string()}));
    let _ep = ep; // serve until killed
    loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
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
                let _ = labkit::make_writable(&dest);
                let _ = std::fs::remove_dir_all(&dest);
                std::fs::create_dir_all(&dest)?;
                // Serve the connection exactly like the app serves a friend:
                // files.stat + every (negotiated, maybe parallel) push until the
                // sender closes. A one-shot push is just a single stream.
                let engaged = AtomicBool::new(false);
                let got = labkit::serve_friend_conn(&conn, &dest, &engaged, |_, _| {}).await?;
                let ms = started.elapsed().as_millis() as u64;
                let files = labkit::tree_report(&dest)?;
                let bytes: u64 = files.iter().map(|f| f.size).sum();
                let dirs = labkit::dir_report(&dest);
                Ok(json!({
                    "event": "received",
                    "conn": idx,
                    "files": got.len(),
                    "bytes": bytes,
                    "ms": ms,
                    "parallelEngaged": engaged.load(std::sync::atomic::Ordering::Relaxed),
                    "report": files,
                    "dirs": dirs,
                }))
            }
            .await;
            let line = match result {
                Ok(v) => v,
                Err(e) => json!({"event": "recv-error", "conn": idx, "error": e.to_string()}),
            };
            emit(line.clone());
            // A dial that lost a happy-eyeballs race (several direct addrs) or
            // was abandoned never carried a case — keep it out of the results.
            let abandoned = line["event"] == "recv-error" && line["error"] == "accept connection";
            if line["event"] != "results-served" && !abandoned {
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

/// The operator's persistent state dir — its identity lives here so the node id
/// stays stable across runs (the user pastes it into each device's Lab operator
/// field once).
fn operator_state_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".dropbeam-lab-operator"))
        .unwrap_or_else(|_| std::env::temp_dir().join("dropbeam-lab-operator"))
}

/// Print this operator's node id — the value the user sets as "Lab operator" on
/// each device so it will accept our commands.
async fn lab_id(_args: &[String]) -> Result<()> {
    let ep = labkit::operator_endpoint(&operator_state_dir()).await?;
    println!("{}", ep.id());
    shutdown(&ep).await;
    Ok(())
}

/// Run one Lab Mode command against a device's real app, by its node id.
///   dropbeam-lab lab --to <device-node-id> --cmd ping [--json '{"k":"v"}']
async fn lab_cmd(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <device-node-id> is required")?;
    let cmd = flag(args, "--cmd").unwrap_or_else(|| "ping".into());
    let extra: serde_json::Value = match flag(args, "--json") {
        Some(j) => serde_json::from_str(&j).context("--json must be a JSON object")?,
        None => serde_json::json!({}),
    };
    let ep = labkit::operator_endpoint(&operator_state_dir()).await?;
    let reply = labkit::lab_call(&ep, &to, &cmd, extra).await?;
    println!("{reply}");
    shutdown(&ep).await;
    if reply.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        std::process::exit(1);
    }
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

/// Pull the receiver's accumulated results over iroh; `reset` clears them.
async fn pull_results(ep: &labkit::Endpoint, peer: &labkit::EndpointAddr, reset: bool) -> Result<Vec<serde_json::Value>> {
    let conn = ep.connect(peer.clone(), labkit::LAB_RESULTS_ALPN).await.context("dial peer results channel")?;
    let (mut s, mut r) = conn.open_bi().await?;
    s.write_all(if reset { b"reset" } else { b"get" }).await?;
    s.finish()?;
    let body = r.read_to_end(256 * 1024 * 1024).await?;
    conn.close(0u32.into(), b"ok");
    Ok(serde_json::from_slice(&body)?)
}

async fn send(args: &[String]) -> Result<()> {
    let to = flag(args, "--to").context("--to <labADDR> is required")?;
    let mode = flag(args, "--mode").unwrap_or_else(|| "auto".into());
    let suite = flag(args, "--suite").unwrap_or_else(|| "quick".into());
    // friend = the app's friend/chat path (chat-linked, files.stat, packed
    // small-file pushes); push = the one-shot primitive.
    let via = flag(args, "--via").unwrap_or_else(|| "friend".into());
    let parallel = flag(args, "--parallel").unwrap_or_else(|| "on".into()) != "off";
    let profile = args.iter().any(|a| a == "--profile");
    let verify = !args.iter().any(|a| a == "--no-verify");
    let corpus_dir = flag(args, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dropbeam-lab-corpus"));

    labkit::set_parallel_streams(parallel);
    let peer = labkit::filter_addr(labkit::decode_addr(&to)?, &mode);
    if peer.addrs.is_empty() {
        bail!("peer addr has no {mode} transport addresses — can't force that path");
    }

    let ep = labkit::lab_endpoint_for(&mode).await?;
    let mut cases = labkit::build_corpus(&corpus_dir, &suite)?;
    // --only a,b / --except a,b (comma lists) and --max-mib N (skip cases whose
    // payload is bigger — for slow uplinks) narrow a suite.
    if let Some(only) = flag(args, "--only") {
        let keep: Vec<&str> = only.split(',').collect();
        cases.retain(|c| keep.contains(&c.name));
    }
    if let Some(except) = flag(args, "--except") {
        let drop: Vec<&str> = except.split(',').collect();
        cases.retain(|c| !drop.contains(&c.name));
    }
    if let Some(max) = flag(args, "--max-mib").and_then(|m| m.parse::<u64>().ok()) {
        cases.retain(|c| labkit::expected_report(&c.paths).map(|(f, _)| f.iter().map(|x| x.size).sum::<u64>() <= max << 20).unwrap_or(true));
    }
    emit(json!({
        "event": "start",
        "mode": mode, "suite": suite, "via": via, "parallel": parallel, "verify": verify,
        "cases": cases.len(),
        "corpus": corpus_dir.display().to_string(),
    }));

    let mut failed = 0u32;
    for case in &cases {
        if verify {
            // Round boundary: the next result on the receiver is this case's.
            pull_results(&ep, &peer, true).await.context("reset receiver results")?;
        }
        let started = Instant::now();
        let (expected, exp_dirs) = labkit::expected_report(&case.paths)?;
        let expected_bytes: u64 = expected.iter().map(|f| f.size).sum();
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
            let progress = |done, _| {
                if profile {
                    let t = started.elapsed().as_millis() as u64;
                    let mut s = samples.lock().unwrap();
                    if s.last().map(|(lt, _)| t - lt >= 2000).unwrap_or(true) {
                        s.push((t, done));
                    }
                }
            };
            let sent = if via == "push" {
                labkit::send_files(&conn, &case.paths, &AtomicBool::new(false), progress, "dropbeam-lab", &engaged).await?
            } else {
                labkit::send_like_friend(&conn, &case.paths, &AtomicBool::new(false), progress, "dropbeam-lab", &engaged).await?
            };
            let ms = started.elapsed().as_millis().max(1) as u64;
            let path_end = labkit::conn_detail(&conn);
            conn.close(0u32.into(), b"case done");
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
                "pathEnd": path_end,
                "profile": *samples.lock().unwrap(),
            }))
        }
        .await;
        let mut line = match result {
            Ok(v) => v,
            Err(e) => json!({"event": "send-error", "case": case.name, "error": format!("{e:#}")}),
        };
        let mut pass = line["event"] == "sent";
        if verify && pass {
            // The receiver hashes what landed after acking; give it time.
            let deadline = Instant::now() + std::time::Duration::from_secs(60 + expected_bytes / (20 << 20));
            let got = loop {
                let rs = pull_results(&ep, &peer, false).await.unwrap_or_default();
                if let Some(r) = rs.into_iter().rev().find(|r| r["event"] == "received" || r["event"] == "recv-error") { break Some(r); }
                if Instant::now() > deadline { break None; }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            };
            let problems = match got {
                None => vec!["receiver never reported this case".to_string()],
                Some(r) if r["event"] == "recv-error" => vec![format!("receiver error: {}", r["error"])],
                Some(r) => {
                    let landed: Vec<labkit::FileReport> = serde_json::from_value(r["report"].clone()).unwrap_or_default();
                    let dirs: Vec<String> = serde_json::from_value(r["dirs"].clone()).unwrap_or_default();
                    labkit::verdict(&expected, &exp_dirs, &landed, |d| dirs.iter().any(|x| x == d), case.loose)
                }
            };
            pass = problems.is_empty();
            line["verdict"] = json!(if pass { "PASS" } else { "FAIL" });
            line["problems"] = json!(problems);
        }
        line["files"] = json!(expected.len());
        if !pass { failed += 1; }
        emit(line);
    }
    emit(json!({"event": "done", "failed": failed, "cases": cases.len()}));
    shutdown(&ep).await;
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
