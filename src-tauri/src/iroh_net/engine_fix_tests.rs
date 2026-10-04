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
