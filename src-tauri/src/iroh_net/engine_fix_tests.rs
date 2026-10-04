//! Regression tests for the 2026-10-04 transfer-engine audit fixes (T1–T16,
//! S7, S10). Each test names the audit id it pins.
use super::*;

/// T4: a headless `--server` host refuses plain pushes but must still accept a
/// Location upload (the refusal arm used to shadow the location arm).
#[test]
fn t4_headless_still_hosts_location_uploads() {
    let plain = serde_json::json!({"kind": "files", "items": []});
    let location = serde_json::json!({"kind": "files", "location": {"location_id": "x"}, "items": []});
    let stat = serde_json::json!({"kind": "files.stat", "items": []});
    assert!(headless_refuses_in(true, &plain));
    assert!(headless_refuses_in(true, &stat));
    assert!(!headless_refuses_in(true, &location), "a Location upload must reach receive_location_headless");
    assert!(!headless_refuses_in(false, &plain));
}

fn scratch(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("dropbeam-efix-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// S7 + T5: files.stat / files.verify answer only for paths THIS sender
/// delivered (wherever they landed — Save-to folder, iOS Documents, a
/// collision sibling); other senders and never-sent files read as absent.
#[test]
fn s7_stat_and_verify_only_see_the_senders_own_deliveries() {
    let config = scratch("ledger-cfg");
    let downloads = scratch("ledger-dl");
    // A private file the user never received from anyone.
    std::fs::write(downloads.join("secret.pdf"), b"private").unwrap();
    set_mtime_secs(&downloads.join("secret.pdf"), 1_700_000_000);
    // Alice delivered "photo.jpg"; it landed as a collision sibling elsewhere.
    let landed = downloads.join("photo (1).jpg");
    std::fs::write(&landed, b"alice-bytes").unwrap();
    set_mtime_secs(&landed, 1_700_000_123);
    delivered::record(&config, "alice", "photo.jpg", &landed);

    let stat = |who: &'static str, name: &str, size: u64, mtime: u64| {
        let req = serde_json::json!({"files_v": 1, "items": [{"name": name, "size": size, "mtime": mtime}]});
        friend_stat_reply_with(&req, |n| delivered::lookup(&config, who, n)).unwrap()["landed"].as_array().unwrap().len() == 1
    };
    assert!(stat("alice", "photo.jpg", 11, 1_700_000_123));
    assert!(!stat("bob", "photo.jpg", 11, 1_700_000_123), "another friend can't see Alice's delivery");
    assert!(!stat("alice", "secret.pdf", 7, 1_700_000_000), "never-delivered files are invisible");
    assert!(!stat("alice", "photo.jpg", 11, 0), "mtime 0 is not a wildcard");

    let verify = |who: &'static str, names: &[&str]| {
        let items: Vec<_> = names.iter().map(|n| serde_json::json!({"name": n, "size": 11})).collect();
        let req = serde_json::json!({"files_v": 1, "items": items});
        friend_verify_reply_with(&req, &AtomicBool::new(false), &AtomicU64::new(0), |n| delivered::lookup(&config, who, n)).unwrap()
    };
    let got = verify("alice", &["photo.jpg", "secret.pdf"]);
    assert!(got[0].is_some());
    assert_eq!(got[1], None);
    assert_eq!(verify("bob", &["photo.jpg"]), vec![None]);
    // The same name repeated hashes once and answers identically.
    let repeated = verify("alice", &["photo.jpg"; 50]);
    assert!(repeated.iter().all(|d| d == &got[0]));
    let _ = std::fs::remove_dir_all(config);
    let _ = std::fs::remove_dir_all(downloads);
}

use super::xfer_matrix::{dial, endpoint, friend_send, scratch as mscratch, PACE_GATE};

/// 20,000 files with long paths: a manifest (~2 MB) far over the 1 MiB header
/// cap that used to fail every such send.
fn many_files(root: &Path, n: usize) -> PathBuf {
    let top = root.join("Big folder");
    for i in 0..n {
        let dir = top.join(format!("a fairly long subfolder name number {:03}", i % 200));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("photo with a long descriptive name {i:05}.jpg")), format!("{i}")).unwrap();
    }
    top
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let t = e.file_type().unwrap();
        if t.is_dir() { n += count_files(&e.path()); } else if t.is_file() { n += 1; }
    }
    n
}

/// T1: a 20k-file friend send to a CURRENT receiver: the manifest goes once
/// over `chat-manifest`, every push header stays small, all files land.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_twenty_thousand_file_friend_send_lands() {
    let _gate = PACE_GATE.read().await;
    let src = mscratch("t1-src");
    let rx = mscratch("t1-rx");
    let top = many_files(&src.0, 20_000);
    let (items, dirs, total) = gather_items(std::slice::from_ref(&top)).unwrap();
    let link = super::xfer_matrix::chat_link(&items, &dirs, total);
    assert!(chat_manifest::needs_out_of_band(&link));
    assert!(serde_json::to_vec(&link).unwrap().len() > MAX_HEADER, "the inline manifest alone exceeds the frame cap");

    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let state = Arc::new(IrohState::default());
    let _ = state.test_inbox.set(rx.0.clone());
    let accept = tokio::spawn(accept_loop(server.clone(), state));
    let conn = dial(&client, &server).await;
    tokio::time::timeout(Duration::from_secs(240), friend_send(&conn, &server.id().to_string(), &[top], &AtomicBool::new(false)))
        .await.expect("20k-file send hung").expect("20k-file send failed");
    conn.close(0u32.into(), b"done");
    assert_eq!(count_files(&rx.0), 20_000);
    assert_eq!(chat_manifest::held_for(&client.id().to_string()), 1, "the manifest went over once, out of band");
    accept.abort();
    client.close().await;
    server.close().await;
}

/// T1 compatibility: a receiver that predates `chat-manifest` answers the
/// unknown kind with `{"kind":"ok"}` (every shipped build does). The send must
/// still deliver every file — as plain pushes at the right item offsets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_big_manifest_to_old_receiver_degrades_not_fails() {
    let _gate = PACE_GATE.read().await;
    let src = mscratch("t1old-src");
    let rx = mscratch("t1old-rx");
    let top = many_files(&src.0, 12_000);
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let dest = rx.0.clone();
    let srv = server.clone();
    let old = tokio::spawn(async move {
        let conn = srv.accept().await.unwrap().await.unwrap();
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Ok(header) = read_frame(&mut recv).await else { continue };
            match header["kind"].as_str() {
                Some("files.stat") => {
                    let _ = write_frame(&mut send, &friend_stat_reply(&dest, &header).unwrap()).await;
                }
                Some("files") => {
                    assert!(header.get("chatTransfer").is_none(), "an old receiver never gets a compact link");
                    integrity::scope(read_files_negotiated(&conn, &mut send, &mut recv, &header, &dest,
                        &AtomicBool::new(false), &AtomicBool::new(false), |_, _| {})).await.expect("plain push lands");
                }
                _ => { let _ = write_frame(&mut send, &serde_json::json!({"kind": "ok"})).await; }
            }
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
        }
    });
    let conn = dial(&client, &server).await;
    tokio::time::timeout(Duration::from_secs(240), friend_send(&conn, &server.id().to_string(), &[top], &AtomicBool::new(false)))
        .await.expect("send hung").expect("send to an old receiver must still deliver");
    conn.close(0u32.into(), b"done");
    let _ = tokio::time::timeout(Duration::from_secs(10), old).await;
    assert_eq!(count_files(&rx.0), 12_000);
    client.close().await;
    server.close().await;
}

/// Pull `staged` like the app's Quick Send receiver (pages_v + read_header).
async fn quick_pull(staged: Vec<PathBuf>, dest: &Path, pages_ok: bool) -> (Result<Vec<PathBuf>>, Result<u64>) {
    let server = endpoint(true).await;
    let client = endpoint(false).await;
    let ticket = make_ticket(&server, "t1-token").unwrap();
    let (addr, token) = parse_ticket(&ticket).unwrap();
    let srv = server.clone();
    let serve = tokio::spawn(async move {
        let conn = srv.accept().await.unwrap().await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        let req = read_frame(&mut recv).await?;
        let (items, dirs, total) = gather_items(&staged)?;
        let sent = integrity::scope(serve_pull_verified_items(&conn, &mut send, &mut recv, (&items, &dirs, total),
            true, pages_ok && req["pages_v"] == 1, &AtomicBool::new(false), |_, _| {})).await?;
        let _ = recv.read_to_end(256).await;
        Ok(sent)
    });
    let conn = client.connect(addr, ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &serde_json::json!({"kind": "pull", "token": token, "parallel": true, "integrity_v": 1, "pages_v": 1})).await.unwrap();
    let got = async {
        let first = read_frame(&mut recv).await?;
        let header = quick::read_header(&mut recv, first).await?;
        read_pull_files_negotiated(&conn, &mut send, &mut recv, &header, dest,
            &AtomicBool::new(false), &AtomicBool::new(false), |_, _| {}).await
    }.await;
    drop(conn);
    let sent = serve.await.unwrap();
    client.close().await;
    server.close().await;
    (got, sent)
}

/// T1: a Quick Send of 20,000 files: the header (~3 MB) is paged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_quick_send_twenty_thousand_files_paged() {
    let _gate = PACE_GATE.read().await;
    let src = mscratch("t1q-src");
    let rx = mscratch("t1q-rx");
    let top = many_files(&src.0, 20_000);
    let (got, sent) = tokio::time::timeout(Duration::from_secs(240), quick_pull(vec![top], &rx.0, true)).await.expect("hung");
    sent.expect("sender");
    assert_eq!(got.expect("receiver").len(), 20_000);
    assert_eq!(count_files(&rx.0), 20_000);
}

/// T1 compat: a puller that can't take pages gets a plain refusal, not a hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_quick_send_huge_header_refused_without_pages() {
    let _gate = PACE_GATE.read().await;
    let src = mscratch("t1qr-src");
    let rx = mscratch("t1qr-rx");
    let top = many_files(&src.0, 12_000);
    let (got, sent) = tokio::time::timeout(Duration::from_secs(120), quick_pull(vec![top], &rx.0, false)).await.expect("hung");
    assert!(sent.is_err());
    let e = format!("{:#}", got.unwrap_err());
    assert!(e.contains(quick::REFUSED) && e.contains("update DropBeam"), "{e}");
}

/// T13/T6: through the production pull arm — a canceled link reads as
/// "canceled" (never retried), an unknown/expired one as a final refusal, and a
/// second device can't take over a link another device is receiving.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_pull_arm_refuses_dead_and_foreign_pulls() {
    let server = endpoint(true).await;
    let state = Arc::new(IrohState::default());
    let src = mscratch("t13-src");
    let file = src.0.join("a.txt");
    std::fs::write(&file, b"hello").unwrap();
    let mk = |expires_at: Instant| PendingSend { transfer_id: uuid::Uuid::new_v4().to_string(), paths: vec![file.clone()],
        names: vec!["a.txt".into()], total: 5, cancel: Arc::default(), gen: Arc::default(),
        items: Arc::new(gather_items(std::slice::from_ref(&file)).unwrap().0), dirs: Arc::default(), expires_at, puller: Arc::default() };
    state.pending.lock().unwrap().insert("old".into(), mk(Instant::now() - Duration::from_secs(1)));
    let busy = mk(Instant::now() + Duration::from_secs(600));
    *busy.puller.lock().unwrap() = Some("someone-else".into());
    state.pending.lock().unwrap().insert("busy".into(), busy);
    quick::note_canceled("gone");
    let accept = tokio::spawn(accept_loop(server.clone(), state.clone()));
    let client = endpoint(false).await;
    let rx = mscratch("t13-rx");
    let pull = |tok: &str| {
        let ticket = make_ticket(&server, tok).unwrap();
        let (client, dest) = (client.clone(), rx.0.clone());
        async move { format!("{:#}", pull_files(&client, &ticket, &dest, &AtomicBool::new(false), |_, _| {}).await.unwrap_err()) }
    };
    let canceled = pull("gone").await;
    assert!(canceled.contains(errors::PEER_CANCELED), "{canceled}");
    let unknown = pull("never-made").await;
    assert!(unknown.contains(quick::REFUSED) && unknown.contains("expired or was already used"), "{unknown}");
    let expired = pull("old").await;
    assert!(expired.contains("has expired"), "{expired}");
    assert!(!state.pending.lock().unwrap().contains_key("old"), "an expired link is retired");
    let foreign = pull("busy").await;
    assert!(foreign.contains("another device"), "{foreign}");
    assert!(errors::friendly(Direction::Receive, &foreign).starts_with("This link is already being received"));
    accept.abort();
    client.close().await;
    server.close().await;
}

/// T6: the far side of a cancel/pause reads it from the connection (and from
/// the read error's chain), not as an anonymous "connection lost".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t6_peer_cancel_is_classified_on_the_other_side() {
    for (reason, expect) in [(errors::CLOSE_CANCELED, CancelReason::Cancel), (errors::CLOSE_PAUSED, CancelReason::Pause), (b"done".as_slice(), CancelReason::Cancel)] {
        let server = endpoint(true).await;
        let client = endpoint(false).await;
        let srv = server.clone();
        let served = tokio::spawn(async move {
            let conn = srv.accept().await.unwrap().await.unwrap();
            let (_send, mut recv) = conn.accept_bi().await.unwrap();
            let mut buf = vec![0u8; 64];
            let err = loop {
                match recv.read(&mut buf).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("stream finished instead of a close"),
                    Err(e) => break anyhow::Error::from(e),
                }
            };
            (errors::peer_stopped(&conn), errors::error_peer_stopped(&err), format!("{err}"))
        });
        let conn = dial(&client, &server).await;
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        send.write_all(b"some bytes").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        conn.close(0u32.into(), reason);
        let (by_conn, by_err, display) = served.await.unwrap();
        if reason == b"done" {
            assert_eq!((by_conn, by_err), (None, None), "an ordinary close is not a cancel");
        } else {
            assert_eq!(by_conn, Some(expect));
            assert_eq!(by_err, Some(expect), "error chain: {display}");
        }
        client.close().await;
        server.close().await;
    }
}

/// T10: extra relays are parsed, deduplicated and bad entries reported.
#[test]
fn t10_relay_list_parsing() {
    let (ok, bad) = relay_urls("https://relay.example.com, relay2.example.net:443  https://relay.example.com ftp://nope");
    let ok: Vec<String> = ok.iter().map(|u| u.to_string()).collect();
    assert_eq!(ok.len(), 2, "{ok:?}");
    assert!(ok[0].starts_with("https://relay.example.com"));
    assert!(ok[1].starts_with("https://relay2.example.net"));
    assert_eq!(bad, vec!["ftp://nope".to_string()]);
    assert!(relay_urls("").0.len() == BAKED_RELAYS.len());
}

/// T14: a receive stage lists its directory only while it is live, and a
/// stage left on disk keeps it listed for the startup sweep.
#[test]
fn t14_stage_dirs_listed_only_while_live() {
    use std::io::Write;
    let dir = std::fs::canonicalize(scratch("t14")).unwrap();
    let path = dir.join(format!(".dropbeam-recv-{}.part", uuid::Uuid::new_v4()));
    let (mut stage, mut file) = receive_stage::ReceiveStage::create(path.clone(), 3, "t14").unwrap();
    assert!(load_partial_dirs().contains(&dir), "listed while live");
    file.write_all(b"abc").unwrap();
    drop(file);
    stage.publish(&dir.join("landed.txt")).unwrap();
    drop(stage);
    assert!(!load_partial_dirs().contains(&dir), "a published stage leaves no registration");
    let _ = std::fs::remove_dir_all(dir);
}

/// Smaller: every Windows device name is mangled, including COM¹–³, LPT¹–³,
/// CONIN$/CONOUT$ and a stem with trailing spaces.
#[test]
fn windows_reserved_names_are_complete() {
    for name in ["COM\u{b9}", "lpt\u{b3}.txt", "CONIN$", "conout$.log", "COM0", "LPT0.bin", "CON .txt", "nul"] {
        assert!(windows_safe_component(name).starts_with('_'), "{name}");
    }
    for name in ["COM10", "console.txt", "CONINX", "Company.pdf"] {
        assert_eq!(windows_safe_component(name), name);
    }
}

/// Review fix: a dead progress back-channel or a reconnecting sender is not a
/// user cancel — only `cancel_with(Cancel)` marks one (and pause never does).
#[test]
fn user_cancel_is_explicit() {
    let state = IrohState::default();
    let _ = state.cancel_with("x", CancelReason::Pause);
    assert!(!state.user_canceled.lock().unwrap().contains("x"));
    let _ = state.cancel_with("x", CancelReason::Cancel);
    assert!(state.user_canceled.lock().unwrap().contains("x"));
}
