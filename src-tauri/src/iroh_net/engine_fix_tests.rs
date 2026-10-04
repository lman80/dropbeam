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
