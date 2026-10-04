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
    let (items, dirs, total) = gather_items(&[top.clone()]).unwrap();
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
