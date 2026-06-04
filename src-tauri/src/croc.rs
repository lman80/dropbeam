//! The croc transfer engine.
//!
//! We bundle the `croc` binary as a Tauri external binary and drive it directly
//! via `tokio::process` (rather than the shell plugin) so we can read stderr as
//! raw bytes. croc prints its code phrase, peer endpoint, and progress bar to
//! **stderr**, repainting the progress line with carriage returns (`\r`) ~10x/s.
//! Splitting on `\r`/`\n` lets us surface true live progress.
//!
//! Success is authoritative on the process exit code: `croc send` only exits 0
//! after the receiver confirms full receipt (its TypeFinished handshake), which
//! is exactly the guarantee Shared Drop Folder auto-delete relies on.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::Notify;

use crate::history;
use crate::models::{Direction, HistoryEntry, Locality, Settings, TransferState, TransferUpdate};
use crate::AppState;

/// A unit of work for the engine.
pub enum Job {
    /// Quick Send (code = None → random) or paired send (code = Some).
    Send {
        paths: Vec<String>,
        /// Fixed code phrase via CROC_SECRET; None = let croc generate one.
        code: Option<String>,
    },
    Receive {
        code: String,
        out_dir: String,
    },
}

/// Locate the bundled croc binary across dev and packaged layouts.
pub fn croc_binary_path() -> PathBuf {
    let triple = option_env!("DROPBEAM_TARGET_TRIPLE").unwrap_or("");
    let exe_name = if cfg!(windows) { "croc.exe" } else { "croc" };
    let suffixed = if cfg!(windows) {
        format!("croc-{triple}.exe")
    } else {
        format!("croc-{triple}")
    };

    // 1. Next to the running executable (packaged: Tauri strips the triple).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let plain = dir.join(exe_name);
            if plain.exists() {
                return plain;
            }
            if !triple.is_empty() {
                let s = dir.join(&suffixed);
                if s.exists() {
                    return s;
                }
            }
        }
    }

    // 2. Development: src-tauri/binaries/croc-<triple>.
    if !triple.is_empty() {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&suffixed);
        if dev.exists() {
            return dev;
        }
    }

    // 3. Fall back to PATH.
    PathBuf::from(exe_name)
}

/// Build the croc command for one attempt. `override_send_code` lets retries reuse
/// the code croc generated on the first attempt, so the code shown to the user
/// stays valid across re-parks.
fn build_run_command(
    bin: &Path,
    settings: &Settings,
    job: &Job,
    override_send_code: Option<&str>,
) -> Command {
    let mut cmd = Command::new(bin);
    // Don't let croc misread our piped stdin as content to send.
    cmd.arg("--ignore-stdin");

    // Custom relay (global flags, before the subcommand).
    if !settings.custom_relay.trim().is_empty() {
        cmd.arg("--relay").arg(settings.custom_relay.trim());
        if !settings.custom_relay_pass.trim().is_empty() {
            cmd.arg("--pass").arg(settings.custom_relay_pass.trim());
        }
    }

    match job {
        Job::Send { paths, code } => {
            // Prefer the captured/reused code, else the job's fixed code, else let
            // croc generate one on this (first) attempt.
            if let Some(c) = override_send_code.or(code.as_deref()) {
                cmd.env("CROC_SECRET", c);
            }
            cmd.arg("--disable-clipboard");
            cmd.arg("send");
            for p in paths {
                cmd.arg(p);
            }
        }
        Job::Receive { code, out_dir } => {
            cmd.env("CROC_SECRET", code);
            cmd.arg("--yes").arg("--overwrite").arg("--out").arg(out_dir);
        }
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.env("NO_COLOR", "1");
    cmd
}

/// Kick off a send. Returns the initial snapshot (with a fresh id) immediately;
/// progress arrives via `transfer://update` events.
pub fn start_send(
    app: AppHandle,
    state: Arc<AppState>,
    paths: Vec<String>,
    code: Option<String>,
    friend_name: Option<String>,
) -> TransferUpdate {
    let names: Vec<String> = paths.iter().map(|p| file_name_of(p)).collect();
    let id = uuid::Uuid::new_v4().to_string();
    let mut update = TransferUpdate::new(id, Direction::Send, names);
    update.friend_name = friend_name;
    let job = Job::Send { paths, code };
    let snapshot = update.clone();
    tauri::async_runtime::spawn(run_transfer(app, state, update, job));
    snapshot
}

/// Kick off a receive.
pub fn start_receive(
    app: AppHandle,
    state: Arc<AppState>,
    code: String,
    out_dir: String,
) -> TransferUpdate {
    let id = uuid::Uuid::new_v4().to_string();
    let mut update = TransferUpdate::new(id, Direction::Receive, Vec::new());
    update.code = Some(code.clone());
    update.out_dir = Some(out_dir.clone());
    let job = Job::Receive { code, out_dir };
    let snapshot = update.clone();
    tauri::async_runtime::spawn(run_transfer(app, state, update, job));
    snapshot
}

const SEND_RETRY_BUDGET_SECS: u64 = 600;
const RECEIVE_RETRY_BUDGET_SECS: u64 = 120;
const RETRY_DELAY_MS: u64 = 1000;

enum RunOutcome {
    Completed,
    Canceled,
    Failed { permanent: bool, error: String },
}

async fn run_transfer(app: AppHandle, state: Arc<AppState>, mut update: TransferUpdate, job: Job) {
    let settings = { state.settings.lock().unwrap().clone() };
    let bin = croc_binary_path();

    // One cancellation handle for the whole transfer (it spans retries).
    let cancel = Arc::new(Notify::new());
    state
        .transfers
        .lock()
        .unwrap()
        .insert(update.id.clone(), cancel.clone());
    emit_update(&app, &update);

    // croc's receiver only waits ~2s for a sender, and a handshake can race, so a
    // single attempt is fragile. Retry both directions: the sender re-parks (reusing
    // its code) and the receiver keeps reaching for the parked sender, until success,
    // a real error (wrong code), cancel, or the time budget runs out.
    let budget = match update.direction {
        Direction::Send => Duration::from_secs(SEND_RETRY_BUDGET_SECS),
        Direction::Receive => Duration::from_secs(RECEIVE_RETRY_BUDGET_SECS),
    };
    let deadline = Instant::now() + budget;

    loop {
        // Reuse the code croc generated on the first send attempt.
        let send_code = if matches!(job, Job::Send { .. }) {
            update.code.clone()
        } else {
            None
        };
        let cmd = build_run_command(&bin, &settings, &job, send_code.as_deref());

        match run_once(&app, &mut update, cmd, &cancel).await {
            RunOutcome::Completed => {
                update.state = TransferState::Completed;
                update.percent = 100.0;
                if update.bytes_total > 0 {
                    update.bytes_done = update.bytes_total;
                }
                update.error = None;
                break;
            }
            RunOutcome::Canceled => {
                update.state = TransferState::Canceled;
                break;
            }
            RunOutcome::Failed { permanent, error } => {
                if permanent || Instant::now() >= deadline {
                    update.state = TransferState::Failed;
                    update.error = Some(if permanent {
                        error
                    } else {
                        friendly_timeout(update.direction)
                    });
                    break;
                }
                reset_for_retry(&mut update);
                emit_update(&app, &update);
                if wait_retry(&cancel).await {
                    update.state = TransferState::Canceled;
                    break;
                }
            }
        }
    }

    emit_update(&app, &update);
    finalize(&app, &state, &settings, &update);
}

/// Run croc once, streaming progress into `update`. Returns the attempt's outcome.
async fn run_once(
    app: &AppHandle,
    update: &mut TransferUpdate,
    mut cmd: Command,
    cancel: &Arc<Notify>,
) -> RunOutcome {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunOutcome::Failed {
                permanent: true,
                error: format!("Could not start the transfer engine: {e}"),
            }
        }
    };

    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout = child.stdout.take().expect("stdout is piped");

    // Drain stdout (data channel; empty for normal transfers) to avoid blocking.
    tauri::async_runtime::spawn(async move {
        let mut reader = stdout;
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    });

    let (tx, mut rx) = mpsc::channel::<ParsedLine>(128);
    let reader = tauri::async_runtime::spawn(read_stderr(stderr, tx));

    let mut canceled = false;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(parsed) => {
                    apply(update, parsed);
                    emit_update(app, update);
                }
                None => break,
            },
            _ = cancel.notified() => {
                let _ = child.start_kill();
                canceled = true;
                break;
            }
        }
    }

    drop(rx);
    let _ = reader.await;
    let status = child.wait().await;

    if canceled {
        return RunOutcome::Canceled;
    }
    match status {
        Ok(s) if s.success() => RunOutcome::Completed,
        Ok(_) => {
            let err = update.error.clone().unwrap_or_default();
            RunOutcome::Failed {
                permanent: is_permanent_error(&err),
                error: if err.is_empty() {
                    friendly_exit_error(None)
                } else {
                    err
                },
            }
        }
        Err(e) => RunOutcome::Failed {
            permanent: false,
            error: format!("Transfer engine error: {e}"),
        },
    }
}

/// Errors that won't fix themselves on retry — fail fast on these.
fn is_permanent_error(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("wrong code")
        || e.contains("too short")
        || e.contains("could not be found")
        || e.contains("no such file")
        || e.contains("declined")
        // Receiver chose to decline a manual-accept offer — don't keep re-offering.
        || e.contains("refused")
        || e.contains("rejected")
}

fn reset_for_retry(u: &mut TransferUpdate) {
    u.percent = 0.0;
    u.bytes_done = 0;
    u.speed_bps = 0.0;
    u.eta_seconds = None;
    u.error = None;
    u.state = match u.direction {
        Direction::Send => {
            if u.code.is_some() {
                TransferState::WaitingForPeer
            } else {
                TransferState::Starting
            }
        }
        Direction::Receive => TransferState::Connecting,
    };
}

async fn wait_retry(cancel: &Arc<Notify>) -> bool {
    // Jitter the delay so two retrying peers don't phase-lock and keep missing.
    use rand::Rng;
    let ms = RETRY_DELAY_MS + rand::thread_rng().gen_range(0..RETRY_DELAY_MS);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => false,
        _ = cancel.notified() => true,
    }
}

fn friendly_timeout(dir: Direction) -> String {
    match dir {
        Direction::Receive => {
            "Couldn't connect. Double-check the code, and make sure the other person has their Send screen open.".into()
        }
        Direction::Send => {
            "No one received the files in time — the code may have expired. Try sending again.".into()
        }
    }
}

fn finalize(app: &AppHandle, state: &Arc<AppState>, settings: &Settings, update: &TransferUpdate) {
    state.transfers.lock().unwrap().remove(&update.id);

    // Don't record bare canceled receives that never started, but otherwise log.
    let entry = HistoryEntry {
        id: update.id.clone(),
        direction: update.direction,
        file_names: update.file_names.clone(),
        bytes_total: update.bytes_total,
        peer: update.peer.clone(),
        locality: update.locality,
        code: update.code.clone(),
        state: update.state,
        timestamp_ms: now_ms(),
        error: update.error.clone(),
        out_dir: update.out_dir.clone(),
    };
    history::append(&state.config_dir, entry);
    let _ = app.emit("history://changed", ());

    if settings.notify_on_complete && update.state == TransferState::Completed {
        let what = if update.file_count == 1 {
            update
                .file_names
                .first()
                .cloned()
                .unwrap_or_else(|| "File".into())
        } else if update.file_count > 1 {
            format!("{} files", update.file_count)
        } else {
            "Files".into()
        };
        let body = match update.direction {
            Direction::Send => format!("Sent {what}"),
            Direction::Receive => format!("Received {what}"),
        };
        notify(app, "DropBeam", &body);
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

fn emit_update(app: &AppHandle, u: &TransferUpdate) {
    let _ = app.emit("transfer://update", u);
}

// ---------------------------------------------------------------------------
// stderr reading + parsing
// ---------------------------------------------------------------------------

enum ParsedLine {
    Code(String),
    Peer {
        ip: String,
        locality: Locality,
    },
    Progress {
        percent: f64,
        done: u64,
        total: u64,
        speed_bps: Option<f64>,
        eta: Option<f64>,
    },
    FileName(String),
    Error(String),
}

async fn read_stderr(mut stderr: tokio::process::ChildStderr, tx: mpsc::Sender<ParsedLine>) {
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => {
                if !line.is_empty() {
                    if let Some(p) = parse_segment(&line) {
                        let _ = tx.send(p).await;
                    }
                }
                break;
            }
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        if !line.is_empty() {
                            if let Some(p) = parse_segment(&line) {
                                if tx.send(p).await.is_err() {
                                    return; // receiver gone (canceled)
                                }
                            }
                            line.clear();
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
        }
    }
}

fn apply(u: &mut TransferUpdate, p: ParsedLine) {
    match p {
        ParsedLine::Code(c) => {
            u.code = Some(c);
            if u.state == TransferState::Starting {
                u.state = TransferState::WaitingForPeer;
            }
        }
        ParsedLine::Peer { ip, locality } => {
            u.peer = Some(ip);
            u.locality = locality;
            if matches!(u.state, TransferState::Starting | TransferState::WaitingForPeer) {
                u.state = TransferState::Connecting;
            }
        }
        ParsedLine::Progress {
            percent,
            done,
            total,
            speed_bps,
            eta,
        } => {
            u.state = TransferState::Transferring;
            u.percent = percent;
            u.bytes_done = done;
            if total > 0 {
                u.bytes_total = total;
            }
            if let Some(s) = speed_bps {
                u.speed_bps = s;
            }
            u.eta_seconds = eta;
        }
        ParsedLine::FileName(name) => {
            if !u.file_names.iter().any(|n| n == &name) {
                u.file_names.push(name);
                u.file_count = u.file_names.len();
            }
        }
        ParsedLine::Error(e) => {
            u.error = Some(clean_error(&e));
        }
    }
}

static RE_ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").unwrap());
static RE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Code is:\s*(\S+)").unwrap());
// Progress line, e.g. "sample.bin  85% |████   | (45/52 MB, 410 MB/s) [0s:0s]"
// or the very first frame "sample.bin   0% |     | ( 0 B/52 MB) [0s:0s]" (no speed,
// and a per-side unit on the transferred value). The line is prefixed by the
// filename, not the peer — peer/locality comes from RE_PEER below.
static RE_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<pct>\d+)%\s+\|[^|]*\|\s+\(\s*(?P<done>[\d.]+)\s*(?P<dunit>[A-Za-z]*)\s*/\s*(?P<total>[\d.]+)\s*(?P<unit>[A-Za-z]+)\s*(?:,\s*(?P<spd>[\d.]+)\s*(?P<spdunit>[A-Za-z]+)/s)?\s*\)(?:\s*\[(?P<elapsed>[^:\]]+):(?P<eta>[^\]]+)\])?",
    )
    .unwrap()
});
// Standalone peer line: "Sending (->192.168.0.19:56167)" / "Receiving (<-127.0.0.1:5)".
static RE_PEER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:Sending|Receiving)\s*\([-<>]+\s*([^)]+?)\)\s*$").unwrap()
});
// Summary line carrying the file name: "Receiving 'sample.bin' (50.0 MB)".
static RE_NAMED_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:Receiving|Sending)\s+'([^']+)'").unwrap()
});

fn parse_segment(raw: &[u8]) -> Option<ParsedLine> {
    let s = String::from_utf8_lossy(raw);
    let s = strip_ansi(&s);
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    if let Some(c) = RE_PROGRESS.captures(s) {
        let pct: f64 = c.name("pct")?.as_str().parse().ok()?;
        let total_unit = c.name("unit")?.as_str();
        let done_unit = c
            .name("dunit")
            .map(|m| m.as_str())
            .filter(|u| !u.is_empty())
            .unwrap_or(total_unit);
        let done = parse_bytes(c.name("done")?.as_str(), done_unit);
        let total = parse_bytes(c.name("total")?.as_str(), total_unit);
        let speed = match (c.name("spd"), c.name("spdunit")) {
            (Some(v), Some(u)) => Some(parse_bytes(v.as_str(), u.as_str()) as f64),
            _ => None,
        };
        let eta = c.name("eta").and_then(|m| parse_go_duration(m.as_str()));
        return Some(ParsedLine::Progress {
            percent: pct.clamp(0.0, 100.0),
            done,
            total,
            speed_bps: speed,
            eta,
        });
    }

    if let Some(c) = RE_PEER.captures(s) {
        let ip = c.get(1)?.as_str().trim().to_string();
        let locality = classify_ip(&ip);
        return Some(ParsedLine::Peer { ip, locality });
    }

    if let Some(c) = RE_NAMED_FILE.captures(s) {
        return Some(ParsedLine::FileName(c.get(1)?.as_str().to_string()));
    }

    if let Some(c) = RE_CODE.captures(s) {
        return Some(ParsedLine::Code(c.get(1)?.as_str().to_string()));
    }

    let lower = s.to_lowercase();
    const ERROR_KEYWORDS: [&str; 9] = [
        "password mismatch",
        "could not secure",
        "refusing",
        "refused files",
        "is too short",
        "no such file",
        "connection refused",
        "context deadline",
        "error:",
    ];
    if ERROR_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return Some(ParsedLine::Error(s.to_string()));
    }

    None
}

fn strip_ansi(s: &str) -> String {
    RE_ANSI.replace_all(s, "").into_owned()
}

/// Full progress metrics from a croc progress line — reused by the folder sync UI.
pub(crate) struct ProgressMetrics {
    pub percent: f64,
    pub done: u64,
    pub total: u64,
    pub speed_bps: Option<f64>,
    pub eta: Option<f64>,
}

pub(crate) fn parse_progress_metrics(line: &str) -> Option<ProgressMetrics> {
    let s = strip_ansi(line);
    let c = RE_PROGRESS.captures(s.trim())?;
    let pct: f64 = c.name("pct")?.as_str().parse().ok()?;
    let total_unit = c.name("unit")?.as_str();
    let done_unit = c
        .name("dunit")
        .map(|m| m.as_str())
        .filter(|u| !u.is_empty())
        .unwrap_or(total_unit);
    let done = parse_bytes(c.name("done")?.as_str(), done_unit);
    let total = parse_bytes(c.name("total")?.as_str(), total_unit);
    let speed = match (c.name("spd"), c.name("spdunit")) {
        (Some(v), Some(u)) => Some(parse_bytes(v.as_str(), u.as_str()) as f64),
        _ => None,
    };
    let eta = c.name("eta").and_then(|m| parse_go_duration(m.as_str()));
    Some(ProgressMetrics {
        percent: pct.clamp(0.0, 100.0),
        done,
        total,
        speed_bps: speed,
        eta,
    })
}

/// Convert a croc human-readable byte value to raw bytes (decimal: kB=1000).
pub(crate) fn parse_bytes(value: &str, unit: &str) -> u64 {
    let v: f64 = value.parse().unwrap_or(0.0);
    let mult = match unit.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('k') => 1e3,
        Some('m') => 1e6,
        Some('g') => 1e9,
        Some('t') => 1e12,
        _ => 1.0,
    };
    (v * mult) as u64
}

/// Parse a Go duration string ("5s", "1m30s", "1h2m3s", "200ms") to seconds.
fn parse_go_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0f64;
    let mut num = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            i += 1;
        } else {
            let unit = if c == 'm' && i + 1 < bytes.len() && bytes[i + 1] == b's' {
                i += 2;
                "ms"
            } else {
                i += 1;
                match c {
                    'h' => "h",
                    'm' => "m",
                    's' => "s",
                    _ => "s",
                }
            };
            let val: f64 = num.parse().unwrap_or(0.0);
            num.clear();
            total += match unit {
                "h" => val * 3600.0,
                "m" => val * 60.0,
                "s" => val,
                "ms" => val / 1000.0,
                _ => val,
            };
        }
    }
    Some(total)
}

fn classify_ip(s: &str) -> Locality {
    let host = strip_port(s).to_lowercase();
    let private = host.starts_with("192.168.")
        || host.starts_with("10.")
        || host.starts_with("169.254.")
        || host.starts_with("127.")
        || host == "::1"
        || host.starts_with("fe80")
        || host.starts_with("fc")
        || host.starts_with("fd")
        || is_172_private(&host);
    if private {
        Locality::Local
    } else {
        Locality::Internet
    }
}

fn is_172_private(host: &str) -> bool {
    if let Some(rest) = host.strip_prefix("172.") {
        if let Some(second) = rest.split('.').next() {
            if let Ok(n) = second.parse::<u32>() {
                return (16..=31).contains(&n);
            }
        }
    }
    false
}

fn strip_port(s: &str) -> String {
    let s = s.trim();
    // Bracketed IPv6: [::1]:port
    if let Some(end) = s.strip_prefix('[') {
        if let Some(idx) = end.find(']') {
            return end[..idx].to_string();
        }
    }
    // host:port where port is all digits (IPv4 or hostname).
    if let Some((host, port)) = s.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) && !host.is_empty() && host.contains('.') {
            return host.to_string();
        }
    }
    s.to_string()
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn friendly_exit_error(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("The transfer did not complete (code {c}). The code may be wrong, or the other side went offline."),
        None => "The transfer was interrupted.".to_string(),
    }
}

fn clean_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("password mismatch") {
        "Wrong code — the code phrase didn't match.".into()
    } else if lower.contains("could not secure") {
        "Couldn't establish a secure connection to the peer.".into()
    } else if lower.contains("refusing") || lower.contains("refused files") {
        "The other side declined the files.".into()
    } else if lower.contains("is too short") {
        "Code is too short (needs at least 6 characters).".into()
    } else if lower.contains("no such file") {
        "A file to send could not be found.".into()
    } else {
        raw.trim().trim_start_matches("error:").trim().to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(s: &str) -> Option<ParsedLine> {
        parse_segment(s.as_bytes())
    }

    #[test]
    fn parses_code_line() {
        match seg("Code is: testcode619415342") {
            Some(ParsedLine::Code(c)) => assert_eq!(c, "testcode619415342"),
            _ => panic!("expected Code"),
        }
    }

    #[test]
    fn parses_peer_and_locality() {
        match seg("Sending (->192.168.0.19:56167)") {
            Some(ParsedLine::Peer { ip, locality }) => {
                assert_eq!(ip, "192.168.0.19:56167");
                assert_eq!(locality, Locality::Local);
            }
            _ => panic!("expected Peer"),
        }
        match seg("Receiving (<-127.0.0.1:56165)") {
            Some(ParsedLine::Peer { locality, .. }) => assert_eq!(locality, Locality::Local),
            _ => panic!("expected Peer"),
        }
    }

    #[test]
    fn classifies_ips() {
        assert_eq!(classify_ip("8.8.8.8:443"), Locality::Internet);
        assert_eq!(classify_ip("203.0.113.5:9009"), Locality::Internet);
        assert_eq!(classify_ip("10.1.2.3:5"), Locality::Local);
        assert_eq!(classify_ip("172.16.5.4:5"), Locality::Local);
        assert_eq!(classify_ip("172.32.5.4:5"), Locality::Internet);
        assert_eq!(classify_ip("192.168.0.19:56167"), Locality::Local);
    }

    #[test]
    fn parses_progress_with_speed() {
        match seg("sample.bin  85% |█████   | (45/52 MB, 410 MB/s) [0s:0s]") {
            Some(ParsedLine::Progress { percent, done, total, speed_bps, .. }) => {
                assert_eq!(percent, 85.0);
                assert_eq!(done, 45_000_000);
                assert_eq!(total, 52_000_000);
                assert_eq!(speed_bps, Some(410_000_000.0));
            }
            _ => panic!("expected Progress"),
        }
    }

    #[test]
    fn parses_zero_frame_dual_unit_no_speed() {
        match seg("sample.bin   0% |          | ( 0 B/52 MB) [0s:0s]") {
            Some(ParsedLine::Progress { percent, done, total, speed_bps, .. }) => {
                assert_eq!(percent, 0.0);
                assert_eq!(done, 0);
                assert_eq!(total, 52_000_000);
                assert_eq!(speed_bps, None);
            }
            _ => panic!("expected Progress"),
        }
    }

    #[test]
    fn parses_complete_frame_with_leading_space() {
        match seg(" sample.bin 100% |████████| (52/52 MB, 357 MB/s)") {
            Some(ParsedLine::Progress { percent, done, total, .. }) => {
                assert_eq!(percent, 100.0);
                assert_eq!(done, 52_000_000);
                assert_eq!(total, 52_000_000);
            }
            _ => panic!("expected Progress"),
        }
    }

    #[test]
    fn parses_received_filename() {
        match seg("Receiving 'sample.bin' (50.0 MB) ") {
            Some(ParsedLine::FileName(n)) => assert_eq!(n, "sample.bin"),
            _ => panic!("expected FileName"),
        }
    }

    #[test]
    fn detects_errors() {
        assert!(matches!(seg("password mismatch"), Some(ParsedLine::Error(_))));
        assert!(matches!(
            seg("could not secure channel"),
            Some(ParsedLine::Error(_))
        ));
    }

    #[test]
    fn go_duration_parsing() {
        assert_eq!(parse_go_duration("0s"), Some(0.0));
        assert_eq!(parse_go_duration("45s"), Some(45.0));
        assert_eq!(parse_go_duration("1m30s"), Some(90.0));
        assert_eq!(parse_go_duration("1h2m3s"), Some(3723.0));
        assert_eq!(parse_go_duration("200ms"), Some(0.2));
    }

    #[test]
    fn strips_ansi() {
        let s = strip_ansi("\x1b[32mCode is: abc-def\x1b[0m");
        assert_eq!(s, "Code is: abc-def");
    }
}
