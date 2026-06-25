//! Background diagnostics telemetry.
//!
//! Once a day (and ~30s after launch, to catch the tail of the previous session),
//! we read DropBeam's own rotating log files, keep only the NOTABLE lines — errors,
//! warnings, transfer stalls, relay/canary fallbacks, "re-queued" loops, Local
//! Network blocks, etc. — REDACT anything personal (file paths/names, endpoint ids,
//! IP addresses), group them into a tiny digest, and POST it to a collector the
//! developer controls. This surfaces the background problems users never see or
//! report. Opt-out via Settings; nothing here ever sends file contents or names.
//!
//! Privacy: only error/perf METADATA leaves the device. See `redact()`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use tauri::{AppHandle, Manager};

// The operator's deployed collector, built in for EVERY install (the user explicitly
// asked for built-in-for-everyone) unless a device overrides it in Settings. It's the
// developer's own Cloudflare Worker (see DIAGNOSTICS-SETUP.md). The `?t=` ingest token
// is intentionally public here — in a public build it can't be kept secret, so ingest
// is effectively open (the Worker still caps body size; the review dashboard stays
// password-gated). Still gated by the `share_diagnostics` opt-out, and only error/perf
// METADATA is ever sent (see `redact()`).
const DEFAULT_DIAG_URL: &str =
    "https://dropbeam-diag.ashton-mcp-worker.workers.dev/ingest?t=b85e1e1c2bb1964fbe44c5cd";

/// The endpoint to use: a device's own override if set, else the built-in default.
fn endpoint_for(configured: &str) -> String {
    let c = configured.trim();
    if c.is_empty() {
        DEFAULT_DIAG_URL.to_string()
    } else {
        c.to_string()
    }
}

const FIRST_DELAY_SECS: u64 = 30;
const UPLOAD_INTERVAL_SECS: u64 = 12 * 3600; // twice a day while running
const MAX_GROUPS: usize = 80; // cap distinct issue-signatures per digest
const MAX_SAMPLE_LEN: usize = 240; // cap each sample line
/// After a transfer fails, wait this long before uploading — so a burst of
/// failures coalesces into ONE digest instead of one upload per failure.
const FAILURE_DEBOUNCE_SECS: u64 = 90;

/// Signalled when a transfer fails, to wake the telemetry loop early (instead of
/// waiting up to 12h) so a real problem reaches the dashboard within a couple of
/// minutes. The opt-in toggle + operator URL are still honored by the loop.
fn failure_nudge() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// Call from a transfer-failure path (emit_failed): ask the telemetry loop to
/// upload the new log lines soon. Cheap, non-blocking, safe from any thread.
pub fn nudge_after_failure() {
    failure_nudge().notify_one();
}

/// A stable, NON-identifying per-install id (random uuid persisted in the config
/// dir). Lets the developer tell the 3 devices apart without any real identity.
pub fn device_id(config_dir: &Path) -> String {
    let p = config_dir.join("diag-id");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::write(&p, &id);
    id
}

fn watermark_path(config_dir: &Path) -> PathBuf {
    config_dir.join("diag-watermark")
}

/// The timestamp string ("YYYY-MM-DD HH:MM:SS") of the newest line we've already
/// uploaded, so each digest only carries what's NEW since last time.
fn read_watermark(config_dir: &Path) -> String {
    std::fs::read_to_string(watermark_path(config_dir))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn write_watermark(config_dir: &Path, ts: &str) {
    let _ = std::fs::write(watermark_path(config_dir), ts);
}

/// Keep only the newest few rotated log files (the active `DropBeam.log` is never
/// touched). `KeepAll` rotation never deletes, and this always-running menu-bar app
/// rarely restarts, so run this BOTH at startup AND on every telemetry cycle to bound
/// disk on a long-lived session (a startup-only sweep let 12+ rotations pile up).
/// Bounded by file COUNT and total BYTES so a transfer-heavy stretch that rotates many
/// files within one cycle still can't blow the budget. Also clears stale `.log.bak`
/// orphans from older builds, which the digest never reads.
pub fn prune_logs(log_dir: &Path) {
    const KEEP_FILES: usize = 5;
    const KEEP_BYTES: u64 = 40 * 1024 * 1024; // ~40 MB ceiling on rotated history
    let Ok(rd) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut rotated: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("DropBeam")
                && n != "DropBeam.log"
                && (n.ends_with(".log") || n.ends_with(".bak"))
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    rotated.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    let (mut kept, mut bytes) = (0usize, 0u64);
    for (_, len, p) in rotated {
        if kept < KEEP_FILES && bytes + len <= KEEP_BYTES {
            kept += 1;
            bytes += len;
        } else {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Compiled redaction patterns (built once).
fn redactors() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| {
        vec![
            // ANY quoted string → "<q>". DropBeam logs put the variable (file/peer/
            // folder name, path) in quotes and the static text outside, so this scrubs
            // names/paths while preserving the message for grouping. Run FIRST so a
            // quoted path/id is caught here before the looser rules below.
            (Regex::new(r#""[^"]{0,200}""#).unwrap(), "\"<q>\""),
            // Some logs quote the variable with single quotes — scrub those too.
            // {1,200} (not 0) so a lone apostrophe in prose isn't a match start.
            (Regex::new(r#"'[^']{1,200}'"#).unwrap(), "'<q>'"),
            // URLs → <url>. iroh logs the home-relay/dial URL (wss://…, https://…) on
            // connect and on relay fallback; the region code in the host is coarse geo.
            (Regex::new(r#"(?:https?|wss?)://[^\s"']+"#).unwrap(), "<url>"),
            // Bare hostnames that identify a device or its region: iroh relay/pkarr
            // hosts (*.iroh.network / *.dns.iroh.link) and .local mDNS names (which
            // routinely embed the user's real name, e.g. Mong-MacBook-Pro.local).
            (Regex::new(r"(?i)\b(?:[\w-]+\.)+(?:iroh\.(?:network|link)|local)\.?").unwrap(), "<host>"),
            // Any absolute filesystem path (prefix-agnostic: /Users, /Volumes,
            // /System/Volumes/Data firmlinks, /home, Windows drives) → <path>.
            (Regex::new(r"(?:/[\w.+\- ]+){2,}/?").unwrap(), "<path>"),
            (Regex::new(r"[A-Za-z]:\\[\w.+\\\- ]*").unwrap(), "<path>"),
            // Windows paths WITHOUT a drive letter: UNC shares (\\srv\share\…) and
            // relative backslash paths (\Users\name\…) — these leak the OS username and
            // the drive rule above misses them.
            (Regex::new(r"(?:\\[\w.$+\- ]+){2,}\\?").unwrap(), "<path>"),
            // iroh's SHORT node id (fmt_short = first 5 bytes as 10 lowercase-hex chars)
            // is logged as a structured field — remote_id=, peer=, me=, dst_endpoint=, …
            // — on nearly every connection-lifecycle line at DEBUG/WARN. The {40,} rule
            // below only catches the FULL 52/64-char id, so anchor on the field keyword
            // here to scrub the short prefix too (a stable device fingerprint).
            (Regex::new(r"(?i)\b(remote_id|node_id|endpoint_id|src_endpoint|dst_endpoint|remote|endpoint|node|peer|conn|me|src|dst|from|to|who|id|key|addr)\b[\s=:]+([0-9a-fA-F]{8,64})\b").unwrap(), "$1 <id>"),
            // iroh node/endpoint ids (z-base-32, 52 chars), sha256 hex (64), and any
            // other long opaque token → <id>. 40+ alnum is an id/hash, not a word.
            (Regex::new(r"\b[A-Za-z0-9_-]{40,}\b").unwrap(), "<id>"),
            // MAC / hardware address (immutable, globally-unique device fingerprint):
            // 6 octets of 2 hex, colon OR hyphen separated. BEFORE the IP rules so the
            // colon form isn't half-eaten by the leading-hex IPv6 rule below.
            (Regex::new(r"(?i)\b(?:[0-9a-f]{2}[:-]){5}[0-9a-f]{2}\b").unwrap(), "<mac>"),
            // IP addresses (a peer's IP is personal) → <ip>. IPv4, then bracketed
            // socket-addr IPv6 ([fe80::1]:port), then a bare IPv6 that REQUIRES a
            // leading hex group so it can't match a module path's "::" or a clock.
            (Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(), "<ip>"),
            (Regex::new(r"\[[0-9a-fA-F:]{2,}\]").unwrap(), "[<ip>]"),
            (Regex::new(r"\b[0-9a-fA-F]{1,4}:(?:[0-9a-fA-F]{0,4}:){1,6}[0-9a-fA-F]{0,4}\b").unwrap(), "<ip>"),
            // Email addresses → <email>.
            (Regex::new(r"[\w.+\-]+@[\w.\-]+\.\w{2,}").unwrap(), "<email>"),
        ]
    })
}

fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for (re, rep) in redactors() {
        out = re.replace_all(&out, *rep).into_owned();
    }
    if out.len() > MAX_SAMPLE_LEN {
        out.truncate(MAX_SAMPLE_LEN);
        out.push('…');
    }
    out
}

/// Classify a raw log line — `None` means "not worth reporting". We deliberately
/// only keep signals, not the routine chatter, so digests stay tiny.
fn notable(line: &str) -> Option<&'static str> {
    // Tauri's asset protocol logs "File does not exist at path: …" at ERROR level
    // whenever a convertFileSrc avatar/thumbnail 404s (a missing/edited image — a
    // benign UI miss, NOT an app fault). It was the single largest [ERROR] bucket on
    // the dashboard (~194×), burying genuine failures. Drop it before the level
    // checks. (It is NOT the folder-send race — that surfaces as "No such file or
    // directory (os error 2)" — see iroh_net.rs.)
    if line.contains("File does not exist at path") {
        return None;
    }
    if line.contains("[ERROR]") || line.contains("panicked") {
        Some("error")
    } else if line.contains("[WARN]") {
        Some("warn")
    } else if line.contains("PERF[") {
        Some("perf")
    } else if line.contains("stalled")
        || line.contains("REFUSED")
        || line.contains("re-queued")
        // "canary" appears both in OUR relay-fallback diagnostics AND in iroh's relay
        // hostnames (*.iroh-canary.iroh.link). Only keep our own — otherwise iroh's
        // routine relay chatter floods the "signal" tier and eats MAX_GROUPS slots.
        || (line.contains("app_lib") && line.contains("canary"))
        || line.contains("Resource busy")
        || line.contains("Local Network")
        || line.contains("unreachable over iroh")
        || line.contains("did not confirm receipt")
    {
        Some("signal")
    } else {
        None
    }
}

/// Classify a PERF line's transport path: Some(true)=direct, Some(false)=relay,
/// None=neither. The PERF line tags it `DIRECT/p2p` or `RELAY/internet`
/// (iroh_net.rs). Compare case-insensitively — the literal token is uppercase
/// `RELAY`, so a naive `contains("relay")` silently never counts relay transfers,
/// zeroing THE signal this whole feature exists to surface (relay-vs-direct slowness).
fn perf_path_kind(line: &str) -> Option<bool> {
    let l = line.to_ascii_lowercase();
    if l.contains("direct") || l.contains("p2p") {
        Some(true)
    } else if l.contains("relay") {
        Some(false)
    } else {
        None
    }
}

/// Parse the `[YYYY-MM-DD][HH:MM:SS]` prefix → "YYYY-MM-DD HH:MM:SS" (sortable as a
/// plain string). Returns None for lines without the stamp (continuation lines).
fn line_ts(line: &str) -> Option<String> {
    let b = line.as_bytes();
    // Expect: [YYYY-MM-DD][HH:MM:SS]…  → indices 1..11 and 13..21
    if b.len() < 22 || b[0] != b'[' || b[11] != b']' || b[12] != b'[' || b[21] != b']' {
        return None;
    }
    let date = &line[1..11];
    let time = &line[13..21];
    if date.as_bytes()[4] == b'-' && time.as_bytes()[2] == b':' {
        Some(format!("{date} {time}"))
    } else {
        None
    }
}

/// The module + message part of a line, dropping the `[date][time][module][LVL]`
/// scaffolding so the signature/sample focus on what actually happened.
fn message_of(line: &str) -> &str {
    // After the 4th `]` (date, time, module, level) comes "] message".
    let mut closes = 0;
    for (i, c) in line.char_indices() {
        if c == ']' {
            closes += 1;
            if closes == 4 {
                return line[(i + 1)..].trim();
            }
        }
    }
    line.trim()
}

/// Collapse a message to a grouping signature: redacted, digits → `#`, whitespace
/// squeezed — so "re-queued 1 file" and "re-queued 12 files" group together.
fn signature(msg_redacted: &str) -> String {
    static DIGITS: OnceLock<Regex> = OnceLock::new();
    static WS: OnceLock<Regex> = OnceLock::new();
    let digits = DIGITS.get_or_init(|| Regex::new(r"\d+").unwrap());
    let ws = WS.get_or_init(|| Regex::new(r"\s+").unwrap());
    let a = digits.replace_all(msg_redacted, "#");
    let b = ws.replace_all(&a, " ");
    let mut s = b.trim().to_string();
    if s.len() > 120 {
        s.truncate(120);
    }
    s
}

struct Group {
    level: &'static str,
    sample: String,
    count: u32,
    last: String,
}

/// Read all DropBeam*.log files, filter to notable lines newer than `since`, and
/// build a grouped digest. Returns (digest_json_value, newest_ts_seen).
fn build_digest(
    log_dir: &Path,
    since: &str,
    header: &serde_json::Value,
) -> Option<(serde_json::Value, String)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(log_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("DropBeam") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());

    use std::collections::HashMap;
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut newest = since.to_string();
    let mut perf_send: Vec<f64> = Vec::new();
    let mut perf_recv: Vec<f64> = Vec::new();
    let mut relay_uses = 0u32;
    let mut direct_uses = 0u32;
    let mut total_notable = 0u32;

    static MBPS: OnceLock<Regex> = OnceLock::new();
    let mbps = MBPS.get_or_init(|| Regex::new(r"([\d.]+)\s*MB/s").unwrap());

    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let mut cur_ts = String::new();
        for line in text.lines() {
            if let Some(ts) = line_ts(line) {
                cur_ts = ts;
            }
            // Only lines strictly newer than the watermark.
            if cur_ts.is_empty() || cur_ts.as_str() <= since {
                continue;
            }
            let Some(kind) = notable(line) else { continue };
            if cur_ts > newest {
                newest = cur_ts.clone();
            }
            total_notable += 1;

            if kind == "perf" {
                if let Some(c) = mbps.captures(line) {
                    if let Ok(v) = c[1].parse::<f64>() {
                        if line.contains("folder-send") || line.contains("send:") {
                            perf_send.push(v);
                        } else {
                            perf_recv.push(v);
                        }
                    }
                }
                match perf_path_kind(line) {
                    Some(true) => direct_uses += 1,
                    Some(false) => relay_uses += 1,
                    None => {}
                }
                continue; // perf is aggregated, not grouped as an "issue"
            }

            let msg = redact(message_of(line));
            let sig = signature(&msg);
            let e = groups.entry(sig).or_insert_with(|| Group {
                level: kind,
                sample: msg.clone(),
                count: 0,
                last: cur_ts.clone(),
            });
            e.count += 1;
            e.last = cur_ts.clone();
        }
    }

    if total_notable == 0 && relay_uses == 0 && direct_uses == 0 {
        return None; // nothing new worth sending
    }

    // Sort issues: errors first, then by frequency. Cap the list.
    let mut issues: Vec<(&String, &Group)> = groups.iter().collect();
    issues.sort_by(|a, b| {
        let rank = |g: &Group| match g.level {
            "error" => 0,
            "warn" => 1,
            _ => 2,
        };
        rank(a.1)
            .cmp(&rank(b.1))
            .then(b.1.count.cmp(&a.1.count))
    });
    issues.truncate(MAX_GROUPS);

    let avg = |v: &[f64]| -> f64 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };

    let issues_json: Vec<serde_json::Value> = issues
        .iter()
        .map(|(_sig, g)| {
            serde_json::json!({
                "level": g.level,
                "msg": g.sample,
                "count": g.count,
                "last": g.last,
            })
        })
        .collect();

    let digest = serde_json::json!({
        "v": 1,
        "header": header,
        "window": { "since": since, "until": newest },
        "totals": {
            "notable": total_notable,
            "errors": groups.values().filter(|g| g.level == "error").count(),
            "warnings": groups.values().filter(|g| g.level == "warn").count(),
            "distinctIssues": groups.len(),
        },
        "perf": {
            "sendAvgMBps": (avg(&perf_send) * 10.0).round() / 10.0,
            "recvAvgMBps": (avg(&perf_recv) * 10.0).round() / 10.0,
            "sendSamples": perf_send.len(),
            "recvSamples": perf_recv.len(),
            "directPaths": direct_uses,
            "relayPaths": relay_uses,
        },
        "issues": issues_json,
    });
    Some((digest, newest))
}

/// Background loop: wait, then upload a digest every ~12h while the app runs (the
/// opt-out is re-checked each cycle, so toggling it off in Settings stops uploads).
pub async fn run(app: AppHandle, config_dir: PathBuf, log_dir: Option<PathBuf>) {
    let Some(log_dir) = log_dir else {
        return; // no log dir → nothing to report
    };
    tokio::time::sleep(std::time::Duration::from_secs(FIRST_DELAY_SECS)).await;
    loop {
        // Read BOTH the opt-in toggle and the operator-configured endpoint each
        // cycle. No URL set → upload nowhere (the safe default for a public build).
        let (enabled, configured) = app
            .try_state::<Arc<crate::AppState>>()
            .map(|st| {
                let s = st.settings.lock().unwrap();
                (s.share_diagnostics, s.diagnostics_url.clone())
            })
            .unwrap_or((false, String::new()));
        let endpoint = endpoint_for(&configured);
        if enabled && endpoint.starts_with("https://") {
            let header = {
                let name = app
                    .try_state::<Arc<crate::AppState>>()
                    .map(|st| st.settings.lock().unwrap().display_name.clone())
                    .unwrap_or_default();
                serde_json::json!({
                    "deviceId": device_id(&config_dir),
                    "name": name,
                    "appVersion": app.package_info().version.to_string(),
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                })
            };
            let since = read_watermark(&config_dir);
            if let Some((digest, newest)) = build_digest(&log_dir, &since, &header) {
                if upload(&endpoint, &digest).await.is_ok() {
                    write_watermark(&config_dir, &newest);
                    log::info!("telemetry: uploaded diagnostics digest ({} → {newest})", if since.is_empty() { "start" } else { &since });
                }
            }
        }
        // Bound disk every cycle — the startup sweep alone can't keep up on a session
        // that never restarts. Runs regardless of the share_diagnostics opt-out.
        prune_logs(&log_dir);
        // Wake on the normal 12h cadence OR early when a transfer just failed
        // (debounced so a burst of failures = one upload) — so a real problem
        // reaches the dashboard in ~2 min instead of up to half a day.
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(UPLOAD_INTERVAL_SECS)) => {}
            _ = failure_nudge().notified() => {
                tokio::time::sleep(std::time::Duration::from_secs(FAILURE_DEBOUNCE_SECS)).await;
            }
        }
    }
}

/// One-shot upload for the "Send a test now" button: builds a digest over the FULL
/// available logs (ignores the watermark so there's always something to look at) and
/// posts it, without disturbing the daily cadence's watermark. Returns a summary or
/// an error string for the UI.
pub async fn run_once(app: &AppHandle, config_dir: &Path, log_dir: Option<&Path>) -> Result<String, String> {
    let Some(log_dir) = log_dir else {
        return Err("No log directory available.".into());
    };
    let (enabled, configured, name) = app
        .try_state::<Arc<crate::AppState>>()
        .map(|st| {
            let s = st.settings.lock().unwrap();
            (s.share_diagnostics, s.diagnostics_url.clone(), s.display_name.clone())
        })
        .unwrap_or((false, String::new(), String::new()));
    if !enabled {
        return Err("Diagnostics sharing is turned off.".into());
    }
    let endpoint = endpoint_for(&configured);
    if !endpoint.starts_with("https://") {
        return Err("Diagnostics endpoint is not a valid https URL.".into());
    }
    let header = serde_json::json!({
        "deviceId": device_id(config_dir),
        "name": name,
        "appVersion": app.package_info().version.to_string(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "test": true,
    });
    match build_digest(log_dir, "", &header) {
        Some((digest, _newest)) => {
            upload(&endpoint, &digest).await?;
            let n = digest["totals"]["distinctIssues"].as_u64().unwrap_or(0);
            Ok(format!("Sent a test digest ({n} distinct issues) to your endpoint."))
        }
        None => {
            // Nothing notable in the logs — still prove the endpoint works with a ping.
            upload(&endpoint, &serde_json::json!({ "v": 1, "header": header, "ping": true })).await?;
            Ok("Endpoint reachable. No notable issues in the logs yet.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_strips_personal_data() {
        // Absolute paths under ANY prefix (/Users, /Volumes, firmlinks) → <path>.
        for p in [
            "/Users/minkyung/Desktop/Secret/a.mov",
            "/Volumes/MyDrive/Quarterly Layoffs/plan.pdf",
            "/System/Volumes/Data/Users/x/Desktop/Mong DropBeam/clip.mov",
        ] {
            let r = redact(&format!("REFUSED delete of {p}"));
            assert!(r.contains("<path>"), "path not redacted: {r}");
            assert!(!r.contains("Minkyung") && !r.contains("Layoffs") && !r.contains("Mong DropBeam"), "leaked: {r}");
        }
        // Any quoted string (file OR folder/peer name, even without an extension).
        for q in ["\"beach-sunset.jpg\"", "\"Mong DropBeam\"", "\"KINFAI-DESKTOP\""] {
            let r = redact(&format!("stalled on {q}"));
            assert!(!r.contains("beach") && !r.contains("Mong") && !r.contains("KINFAI"), "quoted leaked: {r}");
        }
        // IPv4 + IPv6 → <ip>.
        assert!(!redact("peer=116.39.54.87:65195").contains("116.39.54.87"));
        assert!(redact("addr fe80::ce81:b1c:bd2c:69e ok").contains("<ip>"));
        // sha256 hex (64) AND iroh z-base-32 node id (52) → <id>.
        let r = redact("connection from 5d0f9908705cdc722d0ae488739602a2740c2f57e696fd795b6776c90354951e closed");
        assert!(!r.contains("5d0f9908"), "hex id leaked: {r}");
        let zbase = " d2k4nq8rj7m3wv6abch5ftue9psyx2gz4lq7nm8rj3kv6wd5ab2 "; // 52 z-base-32
        let r = redact(&format!("ignoring folder-invite from{zbase}"));
        assert!(r.contains("<id>"), "z-base-32 node id not redacted: {r}");
    }

    #[test]
    fn signature_groups_by_shape() {
        assert_eq!(signature("re-queued 1 file"), signature("re-queued 12 files").replace('s', "").trim());
        // digits collapse to #
        assert_eq!(signature("re-queued 7 file(s) the peer was missing"), "re-queued # file(s) the peer was missing");
    }

    #[test]
    fn timestamp_and_message_parse() {
        let line = "[2026-06-13][03:41:03][app_lib::sync][WARN] folder send stalled";
        assert_eq!(line_ts(line).as_deref(), Some("2026-06-13 03:41:03"));
        assert_eq!(message_of(line), "folder send stalled");
        assert!(line_ts("    continuation line with no stamp").is_none());
    }

    #[test]
    fn notable_picks_signals_only() {
        assert_eq!(notable("[ERROR] boom"), Some("error"));
        assert_eq!(notable("[WARN] hmm"), Some("warn"));
        assert_eq!(notable("PERF[folder-send] send: 4.0 MB/s"), Some("perf"));
        assert_eq!(notable("re-queued 1 file(s) the peer was missing"), Some("signal"));
        assert_eq!(notable("[INFO] routine chatter"), None);
        // "canary" is a signal only in OUR logs, not iroh's relay-host chatter.
        assert_eq!(
            notable("[2026-06-18][07:59:25][app_lib::iroh_net][INFO] falling back to canary relay"),
            Some("signal")
        );
        assert_eq!(
            notable("[2026-06-18][07:59:25][iroh_relay::client][DEBUG] Dialing relay dial_url=wss://x.iroh-canary.iroh.link"),
            None
        );
        // Tauri's asset-protocol 404 (a missing avatar/thumbnail) is logged at ERROR
        // but is benign UI noise — it must NOT count as an app error (it was the
        // largest [ERROR] bucket on the dashboard, ~194×, burying real failures).
        assert_eq!(
            notable("[2026-06-18][07:59:25][ERROR] Asset protocol: File does not exist at path: /x/y.png"),
            None
        );
    }

    #[test]
    fn redaction_strips_network_identifiers() {
        // iroh short node id (10-hex fmt_short) logged as a structured field — the
        // {40,}-char rule misses it, so the keyword-anchored rule must catch it.
        for line in [
            "dst_endpoint=3b8f2a9c1d alpn=dropbeam",
            "remote_id=a17f0b9c2e closed",
            "connected to node 5d0f9908a1",
        ] {
            let r = redact(line);
            assert!(r.contains("<id>"), "short id not redacted: {r}");
            assert!(
                !r.contains("3b8f2a9c1d") && !r.contains("a17f0b9c2e") && !r.contains("5d0f9908a1"),
                "short id leaked: {r}"
            );
        }
        // Relay URL + bare iroh host + pkarr record (coarse geo / topology).
        for line in [
            "home relay is https://use1.relay.iroh.network./",
            "dialing relay use1.relay.iroh.network",
            "publishing pkarr record _iroh.abc123def456.dns.iroh.link",
        ] {
            let r = redact(line);
            assert!(
                !r.contains("use1.relay.iroh.network") && !r.contains("dns.iroh.link"),
                "relay host leaked: {r}"
            );
        }
        // .local mDNS hostname (often contains the user's real name).
        let r = redact("discovered Mong-MacBook-Pro.local on the LAN");
        assert!(!r.contains("Mong-MacBook"), ".local hostname leaked: {r}");
        // Windows path WITHOUT a drive letter (UNC / relative) leaks the OS username.
        for line in [
            r"queued path \Users\minkyung\AppData\Local\Temp\x.tmp",
            r"skipping \\nas\share\minkyung\budget.xlsx",
        ] {
            let r = redact(line);
            assert!(!r.contains("minkyung"), "win path leaked: {r}");
        }
        // A module path's "::" must NOT be mistaken for an IPv6 address.
        let r = redact("app_lib::sync reconcile done");
        assert!(r.contains("sync") && !r.contains("<ip>"), "module path mangled: {r}");
        // MAC / hardware address (both colon and hyphen forms) → <mac>.
        assert_eq!(redact("iface en0 hwaddr 3c:22:fb:8a:9d:1e"), "iface en0 hwaddr <mac>");
        assert_eq!(redact("iface en0 hwaddr 3c-22-fb-8a-9d-1e"), "iface en0 hwaddr <mac>");
        // Short id after a non-id keyword ("to"/"from") is still scrubbed.
        assert!(!redact("sending to 3b8f2a9c1d now").contains("3b8f2a9c1d"));
        // A plain decimal counter is NOT mistaken for an id (no a-f letters).
        let r = redact("transferred 12345678 bytes");
        assert!(r.contains("12345678"), "decimal over-redacted: {r}");
    }

    #[test]
    fn perf_path_classification() {
        // The PERF line tags uppercase RELAY/internet or DIRECT/p2p — count BOTH
        // (a naive contains("relay") never matched "RELAY", zeroing the relay signal).
        assert_eq!(
            perf_path_kind("recv: 5.1 MB/s (276.6 MB in 54s) · DIRECT/p2p · rtt=89ms"),
            Some(true)
        );
        assert_eq!(
            perf_path_kind("send: 2.0 MB/s (10 MB in 5s) · RELAY/internet · rtt=80ms"),
            Some(false)
        );
        assert_eq!(perf_path_kind("send: 4.0 MB/s"), None);
    }
}

/// POST the digest. Returns Ok(()) on a 2xx, else a human-readable reason. A hard
/// outer timeout guarantees this can NEVER hang the caller — important because some
/// networks (e.g. China for *.workers.dev, or a dead corporate proxy) blackhole the
/// connection instead of refusing it, which would otherwise spin "Send test" forever.
async fn upload(endpoint: &str, digest: &serde_json::Value) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;
    let fut = client.post(endpoint).json(digest).send();
    let resp = match tokio::time::timeout(std::time::Duration::from_secs(25), fut).await {
        Ok(r) => r,
        Err(_) => {
            log::warn!("telemetry: upload timed out (hard cap) for {endpoint}");
            return Err("Couldn't reach the diagnostics server within 25s — this network may be blocking it.".into());
        }
    };
    match resp {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => {
            let code = r.status().as_u16();
            log::warn!("telemetry: upload rejected HTTP {code} by {endpoint}");
            Err(format!("Server rejected the upload (HTTP {code})."))
        }
        Err(e) => {
            let why = if e.is_timeout() {
                "timed out — this network may be blocking it"
            } else if e.is_connect() {
                "couldn't connect — the host may be blocked on this network"
            } else {
                "the request failed"
            };
            log::warn!("telemetry: upload failed for {endpoint}: {e}");
            Err(format!("Couldn't reach the diagnostics server ({why})."))
        }
    }
}
