//! Real WebRTC through the bridge: headless Chrome with fake camera/mic, two
//! peer connections, each limited to its own side's TURN bridge, the bridges
//! joined by a real iroh connection over loopback. `#[ignore]`: needs Chrome.
//! Run: CHROME=/path/to/chrome cargo test --lib calls::lab_tests -- --ignored --nocapture

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::media::Bridge;

const ALPN: &[u8] = b"dropbeam-call-lab/1";

pub(crate) fn chrome() -> String {
    std::env::var("CHROME").unwrap_or_else(|_| if cfg!(target_os = "macos") {
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into()
    } else { "google-chrome".into() })
}

/// Serve `page` at `/`; every other request's (path, body) goes to the channel.
pub(crate) async fn harness(page: String) -> (u16, tokio::sync::mpsc::Receiver<(String, String)>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let page = Arc::new(page);
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { break };
            let (tx, page) = (tx.clone(), page.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let (head_end, len) = loop {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        break (p + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let line = String::from_utf8_lossy(&buf[..head_end]).lines().next().unwrap_or("").to_string();
                let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let resp = if path == "/" { page.as_str().to_string() } else { let _ = tx.send((path, body)).await; String::new() };
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp.len(), resp).as_bytes()).await;
            });
        }
    });
    (port, rx)
}

pub(crate) fn spawn_chrome(url: &str, profile: &std::path::Path) -> tokio::process::Child {
    spawn_chrome_with(url, profile, &[])
}

pub(crate) fn spawn_chrome_with(url: &str, profile: &std::path::Path, extra: &[String]) -> tokio::process::Child {
    tokio::process::Command::new(chrome())
        .args(extra)
        .args(["--headless=new", "--no-first-run", "--password-store=basic", "--use-mock-keychain", "--no-default-browser-check", "--use-fake-device-for-media-stream",
            "--use-fake-ui-for-media-stream", "--autoplay-policy=no-user-gesture-required",
            &format!("--user-data-dir={}", profile.display()), url])
        .kill_on_drop(true).stdout(std::process::Stdio::null()).stderr(if std::env::var("LAB_CHROME_LOG").is_ok() { std::process::Stdio::inherit() } else { std::process::Stdio::null() }).spawn().expect("start chrome")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn webrtc_call_over_iroh_bridge_in_chrome() {
    let a = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate()).alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled).bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
    let b = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate()).alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled).bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
    let b_addr = b.addr();
    let b2 = b.clone(); // the endpoint must outlive the test, not just the accept
    let accept = tokio::spawn(async move { b2.accept().await.unwrap().await.unwrap() });
    let conn_a = a.connect(b_addr, ALPN).await.unwrap();
    let conn_b = accept.await.unwrap();
    let tag = rand::random::<u64>();
    let (user, pass) = ("lab-user", "lab-pass");
    let bridge_a = Bridge::start(user, pass, true, tag).await.unwrap();
    let bridge_b = Bridge::start(user, pass, false, tag).await.unwrap();
    bridge_a.attach(conn_a.clone());
    bridge_b.attach(conn_b.clone());
    let seconds = std::env::var("LAB_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8u32);
    let cfg = serde_json::json!({"portA": bridge_a.port, "portB": bridge_b.port, "user": user, "pass": pass, "seconds": seconds});
    let page = include_str!("lab_page.html").replace("__CFG__", &cfg.to_string());
    let (port, mut rx) = harness(page).await;
    let profile = std::env::temp_dir().join(format!("dropbeam-call-lab-{}", rand::random::<u32>()));
    let mut child = spawn_chrome(&format!("http://127.0.0.1:{port}/"), &profile);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60 + seconds as u64);
    let result = loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some((path, body))) if path == "/result" => break body,
            Ok(Some((_, body))) => println!("page: {body}"),
            _ => panic!("no result from Chrome"),
        }
    };
    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&profile);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    println!("bridge A: {:?}", bridge_a.snapshot());
    println!("bridge B: {:?}", bridge_b.snapshot());
    let mut short = v.clone();
    if let Some(o) = short.as_object_mut() { o.remove("samples"); }
    println!("RESULT {}", serde_json::to_string_pretty(&short).unwrap());
    assert_eq!(v["ok"], true, "{result}");
    assert!(bridge_a.snapshot().packets_out > 100 && bridge_b.snapshot().packets_in > 100);
}

/// Speech mouth-to-ear: Chrome's real capture path fed a speech WAV
/// (LAB_WAV=/path/48k.wav), A→B through the bridges over loopback iroh
/// (LAB_BRIDGE=0: Chrome's own direct path, the baseline). Prints the
/// cross-correlated capture→receiver-output delay and the jitter buffer.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn webrtc_speech_latency_in_chrome() {
    let wav = std::env::var("LAB_WAV").expect("LAB_WAV=/path/to/speech.wav (48 kHz mono)");
    let bridge = std::env::var("LAB_BRIDGE").map(|v| v != "0").unwrap_or(true);
    let a = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate()).alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled).bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
    let b = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate()).alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled).bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
    let b_addr = b.addr();
    let b2 = b.clone();
    let accept = tokio::spawn(async move { b2.accept().await.unwrap().await.unwrap() });
    let conn_a = a.connect(b_addr, ALPN).await.unwrap();
    let conn_b = accept.await.unwrap();
    let tag = rand::random::<u64>();
    let (user, pass) = ("lab-user", "lab-pass");
    let bridge_a = Bridge::start(user, pass, true, tag).await.unwrap();
    let bridge_b = Bridge::start(user, pass, false, tag).await.unwrap();
    bridge_a.attach(conn_a.clone());
    bridge_b.attach(conn_b.clone());
    let seconds = std::env::var("LAB_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(25u32);
    let cfg = serde_json::json!({"portA": bridge_a.port, "portB": bridge_b.port, "user": user, "pass": pass, "seconds": seconds, "bridge": bridge});
    let page = include_str!("lab_speech.html").replace("__CFG__", &cfg.to_string());
    let (port, mut rx) = harness(page).await;
    let profile = std::env::temp_dir().join(format!("dropbeam-call-lab-{}", rand::random::<u32>()));
    let mut child = spawn_chrome_with(&format!("http://127.0.0.1:{port}/"), &profile, &[format!("--use-file-for-fake-audio-capture={wav}")]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60 + seconds as u64);
    let result = loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some((path, body))) if path == "/result" => break body,
            Ok(Some((_, body))) => println!("page: {body}"),
            _ => panic!("no result from Chrome"),
        }
    };
    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&profile);
    println!("RESULT {result}");
    let sb = bridge_b.snapshot();
    println!("bridge B gaps from peer: {:?} max {} ms; packets in {}", sb.gaps_from_peer, sb.max_gap_from_peer_ms, sb.packets_in);
    println!("bridge A: {:?}", bridge_a.snapshot());
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["ok"], true, "{result}");
}
