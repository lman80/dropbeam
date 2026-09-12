//! Cross-machine path test for the iroh 1.2 upgrade (not shipped).
//! serve:  pathtest serve            → prints a lab address, accepts pushes, reports bytes.
//! send:   pathtest send <addr> [mb] → dials, prints the paths it got, pushes N MiB.
use anyhow::{Context, Result};
use app_lib::labkit::{decode_addr, encode_addr, filter_addr, lab_addr_ready, lab_endpoint, payload, LAB_RESULTS_ALPN};
use std::time::{Duration, Instant};

fn paths(conn: &iroh::endpoint::Connection) -> String {
    conn.paths().iter().map(|p| format!("{}{} {:?} rtt={}ms",
        if p.is_selected() { "*" } else { "" },
        if p.is_relay() { "relay" } else { "direct" },
        p.remote_addr(), p.rtt().as_millis())).collect::<Vec<_>>().join(" | ")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("serve") => {
            let ep = lab_endpoint(true).await?;
            let addr = lab_addr_ready(&ep).await;
            println!("ADDR {}", encode_addr(&addr)?);
            println!("ID {}", ep.id());
            loop {
                let Some(incoming) = ep.accept().await else { break };
                let conn = match incoming.await { Ok(c) => c, Err(e) => { println!("accept error: {e}"); continue; } };
                println!("conn from {} paths: {}", conn.remote_id().fmt_short(), paths(&conn));
                tokio::spawn(async move {
                    let t0 = Instant::now();
                    let (mut send, mut recv) = match conn.accept_bi().await { Ok(x) => x, Err(e) => { println!("accept_bi: {e}"); return; } };
                    let mut total: u64 = 0; let mut buf = vec![0u8; 1 << 16]; let mut last = Instant::now();
                    loop {
                        match recv.read(&mut buf).await {
                            Ok(Some(n)) => { total += n as u64; if last.elapsed() > Duration::from_secs(2) { last = Instant::now(); println!("  rx {:.1} MiB  paths: {}", total as f64 / 1048576.0, paths(&conn)); } }
                            Ok(None) => break,
                            Err(e) => { println!("read error after {total} bytes: {e}"); break; }
                        }
                    }
                    let secs = t0.elapsed().as_secs_f64();
                    println!("DONE {:.1} MiB in {:.1}s = {:.1} MiB/s  final paths: {}", total as f64 / 1048576.0, secs, total as f64 / 1048576.0 / secs, paths(&conn));
                    let _ = send.write_all(format!("{total}").as_bytes()).await; let _ = send.finish();
                    tokio::time::sleep(Duration::from_secs(2)).await;
                });
            }
            Ok(())
        }
        Some("send") => {
            let addr = filter_addr(decode_addr(args.get(2).context("addr")?)?, args.get(4).map(|s| s.as_str()).unwrap_or("auto"));
            let mb: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(100);
            let ep = lab_endpoint(false).await?;
            let t0 = Instant::now();
            let conn = tokio::time::timeout(Duration::from_secs(30), ep.connect(addr, LAB_RESULTS_ALPN)).await.context("dial timeout")??;
            println!("connected in {}ms paths: {}", t0.elapsed().as_millis(), paths(&conn));
            for _ in 0..6 { tokio::time::sleep(Duration::from_millis(500)).await; println!("  t+{}ms paths: {}", t0.elapsed().as_millis(), paths(&conn)); }
            let (mut send, mut recv) = conn.open_bi().await?;
            let chunk = payload(1 << 16, 7);
            let t1 = Instant::now(); let mut sent: u64 = 0; let mut last = Instant::now();
            for _ in 0..(mb * 16) {
                send.write_all(&chunk).await?; sent += chunk.len() as u64;
                if last.elapsed() > Duration::from_secs(2) { last = Instant::now(); println!("  tx {:.1} MiB {:.1} MiB/s  paths: {}", sent as f64 / 1048576.0, sent as f64 / 1048576.0 / t1.elapsed().as_secs_f64(), paths(&conn)); }
            }
            send.finish()?;
            let ack = tokio::time::timeout(Duration::from_secs(60), recv.read_to_end(64)).await.context("ack timeout")??;
            let secs = t1.elapsed().as_secs_f64();
            println!("SENT {:.1} MiB in {:.1}s = {:.1} MiB/s; receiver got {} bytes; final paths: {}", sent as f64 / 1048576.0, secs, sent as f64 / 1048576.0 / secs, String::from_utf8_lossy(&ack), paths(&conn));
            conn.close(0u32.into(), b"done");
            ep.close().await;
            Ok(())
        }
        _ => { eprintln!("usage: pathtest serve | pathtest send <addr> [mb] [auto|direct|relay]"); Ok(()) }
    }
}
