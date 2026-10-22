//! The local control API: how the `dropbeam` command and MCP server reach the
//! running app. Only the same OS user can connect:
//! - macOS/Linux: a Unix socket `<config>/agent/agent.sock` inside a folder only
//!   the owner can open (0700), the socket itself 0600, and every connection's
//!   peer uid checked against ours;
//! - Windows: a named pipe whose random name is kept in `<config>\agent\pipe`
//!   (only the user can read it) and whose security descriptor grants access to
//!   the current user's SID alone; remote clients are refused.
//!
//! No TCP anywhere.
//!
//! Protocol: one JSON object per line.
//!   → {"id": 1, "op": "chat.send", "input": {…}, "agent": {"name": "claude-code", "reason": "…"}, "stream": false}
//!   ← {"id": 1, "ok": true, "data": …}  |  {"id": 1, "ok": false, "error": {"code", "message", "preview"?}}
//! Streaming ops (events.subscribe, exec.run with "stream": true) first send
//!   ← {"id": 1, "event": {…}}  lines, then the final line above. While one runs
//! the client may send {"id": 1, "stdin": "<base64>"} / {"id": 1, "eof": true} /
//! {"id": 1, "cancel": true}.
//!
//! Every call is appended to `<config>/agent-actions.jsonl` (rotated at 1 MB)
//! and shown in Settings → Agents, where the person can turn agent access off.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

use crate::agent_ops::{self, Ctx, OpError, Risk, R};
use crate::AppState;

const LOG_FILE: &str = "agent-actions.jsonl";
const LOG_CAP: u64 = 1 << 20;
const SETTINGS_FILE: &str = "agents.json";
/// Longest request line accepted (a big stdin chunk in base64 fits easily).
const LINE_CAP: usize = 8 << 20;

// ── who sent this chat message ──────────────────────────────────────────────

/// The agent a chat message was sent through ("via Claude"). Travels on the
/// wire as `fromAgent`; older builds ignore the field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTag { pub name: String }

impl AgentTag {
    /// Bounded, printable, never empty — it's shown in someone else's window.
    pub fn sanitized(name: &str) -> Option<AgentTag> {
        let n: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(40).collect();
        (!n.is_empty()).then_some(AgentTag { name: n })
    }
}

tokio::task_local! {
    static AGENT: Option<AgentTag>;
}

/// Set while an agent's op runs: chat messages it creates carry the tag.
pub fn current_agent_tag() -> Option<AgentTag> {
    AGENT.try_with(|a| a.clone()).ok().flatten()
}

// ── settings (agents.json — only the window changes them) ──────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSettings {
    /// Agents may use this device through the local socket (default on: only
    /// this OS user can reach it).
    pub access: bool,
    /// "Let my other devices run commands here": 0 = off, u64::MAX = until
    /// turned off, otherwise until this time (ms).
    pub exec_until: u64,
}

impl Default for AgentSettings {
    fn default() -> Self { AgentSettings { access: true, exec_until: 0 } }
}

pub fn load_settings(dir: &Path) -> AgentSettings {
    std::fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_settings(dir: &Path, s: &AgentSettings) -> Result<(), String> {
    let tmp = dir.join(format!("{SETTINGS_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(SETTINGS_FILE)).map_err(|e| e.to_string())
}

pub fn access_on(dir: &Path) -> bool { load_settings(dir).access }

/// Whether commands from own devices may run here now, and until when
/// (None while on = until turned off).
pub fn exec_allowed_now(dir: &Path) -> (bool, Option<u64>) {
    let until = load_settings(dir).exec_until;
    let on = cfg!(desktop) && until > crate::chat::now_ms();
    (on, (on && until != u64::MAX).then_some(until))
}

// ── activity log ────────────────────────────────────────────────────────────

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn log_action(app: Option<&AppHandle>, dir: &Path, mut entry: Value) {
    use std::io::Write;
    entry["t"] = json!(crate::chat::now_ms());
    {
        let _g = LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let path = dir.join(LOG_FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_CAP) {
            let _ = std::fs::rename(&path, dir.join(format!("{LOG_FILE}.1")));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(format!("{entry}\n").as_bytes());
        }
    }
    if let Some(app) = app { let _ = app.emit("agents://action", &entry); }
}

pub fn read_log(dir: &Path, limit: usize) -> Vec<Value> {
    let _g = LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut lines: Vec<Value> = [format!("{LOG_FILE}.1"), LOG_FILE.to_owned()].iter()
        .filter_map(|f| std::fs::read_to_string(dir.join(f)).ok())
        .flat_map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<Value>>())
        .collect();
    if lines.len() > limit { lines.drain(..lines.len() - limit); }
    lines
}

// ── events + what transfers are doing ───────────────────────────────────────

static HUB: LazyLock<broadcast::Sender<Value>> = LazyLock::new(|| broadcast::channel(1024).0);
/// Latest update of each transfer since launch, newest first (bounded).
static TRANSFERS: LazyLock<Mutex<VecDeque<Value>>> = LazyLock::new(Default::default);
/// Sends an agent started, so transfers.retry can replay them: id → (op, input).
/// (transfer id, op, input) of a send an agent started.
type RetryEntry = (String, &'static str, Value);
static RETRY: LazyLock<Mutex<VecDeque<RetryEntry>>> = LazyLock::new(Default::default);

/// Live events (the same stream events.subscribe gives).
pub fn events() -> broadcast::Receiver<Value> { HUB.subscribe() }

pub fn remember_retry(id: &str, op: &'static str, input: &Value) {
    let mut r = RETRY.lock().unwrap();
    r.retain(|e| e.0 != id);
    r.push_back((id.to_owned(), op, input.clone()));
    while r.len() > 100 { r.pop_front(); }
}

pub fn retry_payload(id: &str) -> Option<(&'static str, Value)> {
    RETRY.lock().unwrap().iter().find(|e| e.0 == id).map(|e| (e.1, e.2.clone()))
}

fn terminal(state: &str) -> bool { matches!(state, "completed" | "failed" | "canceled" | "paused" | "held") }

pub fn transfers(active_only: bool) -> Value {
    let t = TRANSFERS.lock().unwrap();
    Value::Array(t.iter().filter(|u| !active_only || !terminal(u["state"].as_str().unwrap_or(""))).cloned().collect())
}

pub fn transfer(id: &str) -> Option<Value> {
    TRANSFERS.lock().unwrap().iter().find(|u| u["id"] == id).cloned()
}

fn note_transfer(u: &Value) {
    let mut t = TRANSFERS.lock().unwrap();
    t.retain(|x| x["id"] != u["id"]);
    t.push_front(u.clone());
    t.truncate(200);
}

pub async fn transfer_wait(id: &str, timeout_secs: u64) -> R {
    let mut rx = HUB.subscribe();
    let done = |u: &Value| terminal(u["state"].as_str().unwrap_or(""));
    if let Some(u) = transfer(id).filter(done) { return Ok(u); }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs.clamp(1, 86_400));
    // An id nobody has reported within a minute is a mistake, not a slow start.
    let unknown_by = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let known = transfer(id).is_some() || retry_payload(id).is_some();
        let until = if known { deadline } else { deadline.min(unknown_by) };
        match tokio::time::timeout_at(until, rx.recv()).await {
            Err(_) if !known && transfer(id).is_none() && until < deadline => return Err(OpError::new("not_found", "no transfer with that id (see transfers.list)")),
            Err(_) => return Ok(json!({"timedOut": true, "transfer": transfer(id)})),
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                if let Some(u) = transfer(id).filter(done) { return Ok(u); }
            }
            Ok(Err(_)) => return Err(OpError::new("failed", "DropBeam is shutting down")),
            Ok(Ok(ev)) if ev["type"] == "transfer" && ev["data"]["id"] == id && done(&ev["data"]) => return Ok(ev["data"].clone()),
            Ok(Ok(_)) => {}
        }
    }
}

/// Long-poll for messages from a person (or anyone) newer than `since`.
pub async fn chat_wait(app: &AppHandle, i: &Value) -> R {
    let st = app.state::<Arc<AppState>>().inner().clone();
    let dir = st.config_dir.clone();
    let mut rx = HUB.subscribe();
    let since = i["since"].as_u64().unwrap_or_else(crate::chat::now_ms);
    let person = match i["from"].as_str() {
        Some(w) => Some(crate::agent_ops::resolve(&dir, w)?),
        None => None,
    };
    let owner_of = |id: &str| crate::friends::thread_owner(&dir, id).map(|o| o.id).unwrap_or_else(|| id.to_owned());
    let want = person.as_ref().map(|p| owner_of(&p.id));
    let name_of = |id: &str| crate::friends::get(&dir, id).map(|f| f.name).unwrap_or_default();
    let shape = |m: &crate::chat::ChatMessage| {
        let mut v = json!(m);
        v["fromName"] = json!(name_of(&m.peer_id));
        v
    };
    // Already here (arrived between the agent's last look and this call)?
    let threads: Vec<String> = match &want {
        Some(w) => vec![w.clone()],
        None => crate::friends::load(&dir).into_iter().map(|f| owner_of(&f.id)).collect::<std::collections::BTreeSet<_>>().into_iter().collect(),
    };
    let mut found: Vec<Value> = vec![];
    // Every message already here: a later event about one of them (an edit, a
    // reaction, a status change) is not a new message. A NEW one counts even if
    // the sender's clock is behind ours.
    let mut seen = std::collections::HashSet::new();
    for t in &threads {
        for m in crate::chat::messages(&dir, t) {
            if !m.from_me && m.ts > since && !m.deleted { found.push(shape(&m)); }
            seen.insert(m.id);
        }
    }
    if !found.is_empty() { return Ok(json!({"messages": found, "timedOut": false})); }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(i["timeoutSecs"].as_u64().unwrap_or(60).clamp(1, 3600));
    loop {
        let wait_until = if found.is_empty() { deadline } else { (tokio::time::Instant::now() + Duration::from_millis(400)).min(deadline) };
        match tokio::time::timeout_at(wait_until, rx.recv()).await {
            Err(_) => return Ok(json!({"messages": found, "timedOut": found.is_empty()})),
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err(OpError::new("failed", "DropBeam is shutting down")),
            Ok(Ok(ev)) if ev["type"] == "chat" => {
                let Ok(m) = serde_json::from_value::<crate::chat::ChatMessage>(ev["data"].clone()) else { continue };
                if m.from_me || m.deleted { continue; }
                if want.as_ref().is_some_and(|w| *w != owner_of(&m.peer_id)) { continue; }
                if seen.insert(m.id.clone()) { found.push(shape(&m)); }
            }
            Ok(Ok(_)) => {}
        }
    }
}

const EVENT_NAMES: &[(&str, &str)] = &[
    ("transfer://update", "transfer"), ("chat://message", "chat"), ("friend://presence", "presence"),
    ("chat://typing", "typing"), ("friend-requests://changed", "requests"), ("friends://changed", "friends"),
    ("link://confirm", "link"), ("link://progress", "link"), ("link://linked", "link"), ("link://failed", "link"),
];

/// The last "do the safety codes match?" prompt (cleared when a link ends).
static LINK_PENDING: Mutex<Option<(Value, Instant)>> = Mutex::new(None);

pub fn link_pending() -> Value {
    LINK_PENDING.lock().unwrap().as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(600)).map(|(v, _)| v.clone()).unwrap_or(Value::Null)
}

/// Bridge the window's events into the hub (and remember transfer states).
fn listen_events(app: &AppHandle) {
    for (event, ty) in EVENT_NAMES {
        let ty = *ty;
        app.listen_any(*event, move |e| {
            let data = serde_json::from_str::<Value>(e.payload()).unwrap_or(Value::Null);
            if ty == "transfer" { note_transfer(&data); }
            if ty == "link" {
                *LINK_PENDING.lock().unwrap() = (*event == "link://confirm").then(|| (data.clone(), Instant::now()));
            }
            let _ = HUB.send(json!({"type": ty, "data": data}));
        });
    }
}

// ── one call ────────────────────────────────────────────────────────────────

struct Caller { agent: String, reason: Option<String> }

fn caller(req: &Value) -> Caller {
    let name = req["agent"]["name"].as_str().map(|n| n.chars().filter(|c| !c.is_control()).take(60).collect::<String>())
        .filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "an agent".into());
    let reason = req["agent"]["reason"].as_str().map(|r| r.chars().take(300).collect());
    Caller { agent: name, reason }
}

/// Ops that only describe DropBeam itself aren't worth a log line.
fn quiet(op: &str) -> bool { matches!(op, "status" | "ops.describe" | "agents.log" | "exec.allowedHere") }

/// Checks shared by every path: the op exists, access is on, input fits the
/// schema, destructive ops are confirmed.
fn admit(dir: &Path, op: &str, input: &Value) -> Result<(&'static agent_ops::OpSpec, bool), OpError> {
    let spec = agent_ops::find(op).ok_or_else(|| OpError::new("unknown_op", format!("no op {op:?} (see ops.describe)")))?;
    if !access_on(dir) {
        return Err(OpError::new("forbidden", "Agent access is turned off on this device (DropBeam → Settings → Agents)."));
    }
    agent_ops::validate(spec, input)?;
    let dry = input["dryRun"].as_bool() == Some(true);
    if spec.confirm && !dry && input["confirm"].as_bool() != Some(true) {
        let mut e = OpError::new("confirmation_required", format!("{} needs confirm: true. Ask the person first.", spec.name));
        e.preview = Some(json!({"wouldDo": agent_ops::summarize(op, input), "risk": spec.risk}));
        return Err(e);
    }
    Ok((spec, dry))
}

#[allow(clippy::too_many_arguments)]
fn record(app: &AppHandle, dir: &Path, who: &Caller, op: &str, input: &Value, risk: Option<Risk>, result: &Result<Value, OpError>, started: Instant) {
    if quiet(op) { return; }
    let mut e = json!({"agent": who.agent, "reason": who.reason, "op": op, "risk": risk, "summary": agent_ops::summarize(op, input),
        "ok": result.is_ok(), "ms": started.elapsed().as_millis() as u64});
    if input["dryRun"].as_bool() == Some(true) { e["dryRun"] = json!(true); }
    if let Err(err) = result { e["error"] = json!(err.code); e["message"] = json!(err.message); }
    log_action(Some(app), dir, e);
}

/// Run one request end to end (used by the socket, and so by CLI and MCP).
async fn call(app: &AppHandle, op: &str, input: Value, who: &Caller) -> Result<Value, OpError> {
    let dir = app.state::<Arc<AppState>>().config_dir.clone();
    let started = Instant::now();
    let admitted = admit(&dir, op, &input);
    let risk = admitted.as_ref().ok().map(|(s, _)| s.risk);
    let result = match admitted {
        Err(e) => Err(e),
        Ok((_, dry)) => {
            let ctx = Ctx { agent: who.agent.clone(), reason: who.reason.clone(), dry_run: dry };
            AGENT.scope(AgentTag::sanitized(&who.agent), agent_ops::run(app, op, &input, &ctx)).await
        }
    };
    record(app, &dir, who, op, &input, risk, &result, started);
    result
}

// ── the socket ──────────────────────────────────────────────────────────────

fn reply(id: &Value, r: Result<Value, OpError>) -> Value {
    match r {
        Ok(data) => json!({"id": id, "ok": true, "data": data}),
        Err(e) => json!({"id": id, "ok": false, "error": e}),
    }
}

type Controls = Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>;

async fn serve_conn<S: AsyncRead + AsyncWrite + Send + 'static>(app: AppHandle, stream: S) {
    let (rd, mut wr) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::channel::<Value>(512);
    let writer = tauri::async_runtime::spawn(async move {
        while let Some(v) = rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            if wr.write_all(line.as_bytes()).await.is_err() { break; }
            let _ = wr.flush().await;
        }
    });
    let controls: Controls = Default::default();
    let mut reader = BufReader::new(rd);
    let mut line = Vec::new();
    let mut tasks = vec![];
    loop {
        line.clear();
        match (&mut reader).take(LINE_CAP as u64 + 1).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() > LINE_CAP => { let _ = tx.send(json!({"ok": false, "error": {"code": "too_large", "message": "request line too long"}})).await; break; }
            Ok(_) => {}
        }
        let Ok(req) = serde_json::from_slice::<Value>(&line) else {
            let _ = tx.send(json!({"id": null, "ok": false, "error": {"code": "invalid_input", "message": "each line must be one JSON object"}})).await;
            continue;
        };
        let key = req["id"].to_string();
        if req.get("op").is_none() {
            // stdin / eof / cancel for a running streaming op.
            let ctl = controls.lock().unwrap().get(&key).cloned();
            if let Some(c) = ctl { let _ = c.send(req).await; }
            continue;
        }
        let (app, tx, controls) = (app.clone(), tx.clone(), controls.clone());
        // Waits and streams end with the connection; a change already asked for
        // (a send, a message) is allowed to finish.
        let waits = matches!(req["op"].as_str(), Some("chat.wait" | "transfers.wait" | "events.subscribe" | "exec.run"));
        let task = tauri::async_runtime::spawn(async move {
            let id = req["id"].clone();
            let op = req["op"].as_str().unwrap_or("").to_owned();
            let input = if req["input"].is_null() { json!({}) } else { req["input"].clone() };
            let who = caller(&req);
            let streaming = req["stream"].as_bool() == Some(true);
            let out = match op.as_str() {
                "events.subscribe" => subscribe(&app, &id, input, &who, if streaming { Some(tx.clone()) } else { None }).await,
                "exec.run" if streaming => {
                    let (ctl_tx, ctl_rx) = mpsc::channel(64);
                    controls.lock().unwrap().insert(key.clone(), ctl_tx);
                    let r = exec_streaming(&app, &id, input, &who, tx.clone(), ctl_rx).await;
                    controls.lock().unwrap().remove(&key);
                    r
                }
                _ => call(&app, &op, input, &who).await,
            };
            let _ = tx.send(reply(&id, out)).await;
        });
        if waits { tasks.push(task); }
    }
    // The client went away: stop what it was waiting on (streams, long polls).
    for t in tasks { t.abort(); }
    drop(tx);
    let _ = writer.await;
}

async fn subscribe(app: &AppHandle, id: &Value, input: Value, who: &Caller, sink: Option<mpsc::Sender<Value>>) -> Result<Value, OpError> {
    let dir = app.state::<Arc<AppState>>().config_dir.clone();
    let started = Instant::now();
    if let Err(e) = admit(&dir, "events.subscribe", &input) {
        let r = Err(e);
        record(app, &dir, who, "events.subscribe", &input, Some(Risk::Read), &r, started);
        return r;
    }
    record(app, &dir, who, "events.subscribe", &input, Some(Risk::Read), &Ok(Value::Null), started);
    let types: Vec<String> = input["types"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)).collect();
    let limit = input["timeoutSecs"].as_u64().or(if sink.is_some() { None } else { Some(60) });
    let deadline = limit.map(|s| tokio::time::Instant::now() + Duration::from_secs(s.clamp(1, 86_400)));
    let mut rx = HUB.subscribe();
    let mut collected = vec![];
    loop {
        let next = match deadline { Some(d) => tokio::time::timeout_at(d, rx.recv()).await.ok(), None => Some(rx.recv().await) };
        match next {
            None => return Ok(json!({"events": collected, "timedOut": true})),
            Some(Err(broadcast::error::RecvError::Lagged(n))) => {
                if let Some(s) = &sink { let _ = s.send(json!({"id": id, "event": {"type": "lagged", "data": {"missed": n}}})).await; }
            }
            Some(Err(_)) => return Ok(json!({"events": collected})),
            Some(Ok(ev)) => {
                if !types.is_empty() && !types.iter().any(|t| ev["type"] == t.as_str()) { continue; }
                match &sink {
                    Some(s) => if s.send(json!({"id": id, "event": ev})).await.is_err() { return Ok(Value::Null); },
                    None => { collected.push(ev); if collected.len() >= 500 { return Ok(json!({"events": collected})); } }
                }
            }
        }
    }
}

async fn exec_streaming(app: &AppHandle, id: &Value, input: Value, who: &Caller, sink: mpsc::Sender<Value>, ctl: mpsc::Receiver<Value>) -> Result<Value, OpError> {
    let dir = app.state::<Arc<AppState>>().config_dir.clone();
    let started = Instant::now();
    let r = match admit(&dir, "exec.run", &input) {
        Err(e) => Err(e),
        Ok((_, dry)) => {
            let ctx = Ctx { agent: who.agent.clone(), reason: who.reason.clone(), dry_run: dry };
            let id = id.clone();
            crate::agent_exec::run_remote(app, &input, &ctx, Some(ctl), move |ev| {
                let _ = sink.try_send(json!({"id": id, "event": ev}));
            }).await
        }
    };
    record(app, &dir, who, "exec.run", &input, Some(Risk::Destructive), &r, started);
    r
}

/// Where the CLI finds the running app.
#[cfg(unix)]
pub fn socket_dir(config: &Path) -> PathBuf {
    let dir = config.join("agent");
    // A socket path must fit in sockaddr_un (104 bytes on macOS): with a very
    // long home folder, use /tmp/dropbeam-<uid>, tmux-style (owner-checked).
    if dir.join("agent.sock").as_os_str().len() < 100 { dir } else { PathBuf::from(format!("/tmp/dropbeam-{}", unsafe { libc::geteuid() })) }
}
#[cfg(not(unix))]
pub fn socket_dir(config: &Path) -> PathBuf { config.join("agent") }
#[cfg(unix)]
pub fn socket_path(config: &Path) -> PathBuf { socket_dir(config).join("agent.sock") }
#[cfg(windows)]
pub fn pipe_name_file(config: &Path) -> PathBuf { socket_dir(config).join("pipe") }

/// The socket's folder: ours alone (0700), and really ours — a folder someone
/// else made (or a symlink) at that path is refused.
#[cfg(unix)]
pub(crate) fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let m = std::fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(format!("{} isn't a folder owned by this user", dir.display())));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
async fn listen(app: AppHandle, config: PathBuf) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    private_dir(&socket_dir(&config))?;
    let path = socket_path(&config);
    if path.exists() {
        anyhow::ensure!(tokio::net::UnixStream::connect(&path).await.is_err(), "another DropBeam already answers on the agent socket");
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    log::info!("agents: control socket ready");
    let me = unsafe { libc::geteuid() };
    loop {
        let (stream, _) = listener.accept().await?;
        match stream.peer_cred() {
            Ok(c) if c.uid() == me => {}
            _ => { log::warn!("agents: refused a socket connection from another user"); continue; }
        }
        tauri::async_runtime::spawn(serve_conn(app.clone(), stream));
    }
}

#[cfg(windows)]
mod win {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW};
    use windows::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The signed-in user's SID ("S-1-5-21-…").
    pub fn user_sid() -> anyhow::Result<String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            // u64 storage keeps TOKEN_USER's pointer field aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len);
            let _ = CloseHandle(token);
            got?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut s)?;
            Ok(s.to_string()?)
        }
    }

    /// Security attributes that let only `sid` (full access) open the pipe.
    /// The descriptor is leaked on purpose: it lives as long as the listener.
    pub fn owner_only(sid: &str) -> anyhow::Result<Box<SECURITY_ATTRIBUTES>> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), 1, &mut sd, None)? };
        Ok(Box::new(SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() }))
    }
}

#[cfg(windows)]
async fn listen(app: AppHandle, config: PathBuf) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    std::fs::create_dir_all(socket_dir(&config))?;
    let name = format!(r"\\.\pipe\dropbeam-agent-{}", uuid::Uuid::new_v4().simple());
    let sid = win::user_sid()?;
    let sa = win::owner_only(&sid)?;
    // Kept as an address so the listener future stays Send; never freed.
    let sa_addr = Box::into_raw(sa) as usize;
    let create = |first: bool| unsafe {
        ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true)
            .create_with_security_attributes_raw(&name, sa_addr as *mut std::ffi::c_void)
    };
    let mut server = create(true)?;
    std::fs::write(pipe_name_file(&config), &name)?;
    log::info!("agents: control pipe ready");
    loop {
        server.connect().await?;
        let conn = server;
        server = create(false)?;
        tauri::async_runtime::spawn(serve_conn(app.clone(), conn));
    }
}

/// Start the control API (desktop). Mobile has no local agents.
pub fn start(app: AppHandle, config: PathBuf) {
    listen_events(&app);
    #[cfg(desktop)]
    tauri::async_runtime::spawn(async move {
        if let Err(e) = listen(app, config).await {
            log::warn!("agents: the control socket isn't available: {e:#}");
        }
    });
    #[cfg(not(desktop))]
    let _ = (app, config);
}

// ── Settings → Agents (window only) ─────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPanel {
    access: bool,
    exec_on: bool,
    /// When "run commands" turns itself off (ms); None = until turned off.
    exec_until: Option<u64>,
    exec_supported: bool,
    cli: CliStatus,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    installed: bool,
    path: Option<String>,
    /// What to type if it isn't on PATH (the full path of the app's binary).
    direct: Option<String>,
}

#[tauri::command]
pub fn agent_settings(state: State<'_, Arc<AppState>>) -> AgentPanel {
    let (exec_on, exec_until) = exec_allowed_now(&state.config_dir);
    AgentPanel { access: access_on(&state.config_dir), exec_on, exec_until, exec_supported: cfg!(desktop), cli: cli_status() }
}

#[tauri::command]
pub fn agent_set_access(app: AppHandle, state: State<'_, Arc<AppState>>, on: bool) -> Result<AgentPanel, String> {
    let mut s = load_settings(&state.config_dir);
    s.access = on;
    save_settings(&state.config_dir, &s)?;
    log_action(Some(&app), &state.config_dir, json!({"agent": "you", "op": "settings", "summary": if on { "Agent access turned on" } else { "Agent access turned off" }, "ok": true}));
    let _ = app.emit("agents://changed", ());
    Ok(agent_settings(state))
}

/// "Let my other devices run commands here": "off", "hour" or "always".
#[tauri::command]
pub fn agent_set_exec(app: AppHandle, state: State<'_, Arc<AppState>>, mode: String) -> Result<AgentPanel, String> {
    if !cfg!(desktop) { return Err("Running commands isn't available on this device.".into()); }
    let mut s = load_settings(&state.config_dir);
    s.exec_until = match mode.as_str() {
        "off" => 0,
        "hour" => crate::chat::now_ms() + 3_600_000,
        "always" => u64::MAX,
        other => return Err(format!("unknown choice {other:?}")),
    };
    save_settings(&state.config_dir, &s)?;
    let summary = match mode.as_str() { "off" => "Commands from your other devices turned off", "hour" => "Commands from your other devices allowed for 1 hour", _ => "Commands from your other devices allowed until turned off" };
    log_action(Some(&app), &state.config_dir, json!({"agent": "you", "op": "settings", "summary": summary, "ok": true}));
    let _ = app.emit("agents://changed", ());
    Ok(agent_settings(state))
}

#[tauri::command]
pub fn agent_log(state: State<'_, Arc<AppState>>, limit: Option<usize>) -> Vec<Value> {
    read_log(&state.config_dir, limit.unwrap_or(200).min(2000))
}

#[tauri::command]
pub fn agent_clear_log(state: State<'_, Arc<AppState>>) {
    let _g = LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = std::fs::remove_file(state.config_dir.join(LOG_FILE));
    let _ = std::fs::remove_file(state.config_dir.join(format!("{LOG_FILE}.1")));
}

// ── "Install command-line tool" ─────────────────────────────────────────────

/// The binary a `dropbeam` command should run (the AppImage itself when run
/// from one, since its inner path changes every launch).
fn app_binary() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(p) = std::env::var_os("APPIMAGE") { return Some(PathBuf::from(p)); }
    std::env::current_exe().ok().map(|p| p.canonicalize().unwrap_or(p))
}

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

/// Where `dropbeam` may already be installed, best first.
fn cli_candidates() -> Vec<PathBuf> {
    #[cfg(windows)]
    { vec![windows_bin_dir().join("dropbeam.cmd")] }
    #[cfg(not(windows))]
    { vec![PathBuf::from("/usr/local/bin/dropbeam"), PathBuf::from("/usr/bin/dropbeam"), home().join(".local/bin/dropbeam")] }
}

#[cfg(windows)]
fn windows_bin_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Local")).join("DropBeam").join("bin")
}

pub fn cli_status() -> CliStatus {
    let exe = app_binary();
    let installed = cli_candidates().into_iter().find(|p| {
        if cfg!(windows) { return p.exists(); }
        // A symlink to this app, or the packaged wrapper script.
        match (std::fs::canonicalize(p), &exe) {
            (Ok(t), Some(e)) => &t == e || std::fs::read_to_string(p).is_ok_and(|s| s.starts_with("#!") && s.contains("DropBeam")),
            _ => false,
        }
    });
    CliStatus { installed: installed.is_some(), path: installed.map(|p| p.to_string_lossy().into_owned()),
        direct: exe.map(|e| format!("\"{}\" cli", e.to_string_lossy())) }
}

#[cfg(desktop)]
#[tauri::command]
pub async fn agent_install_cli() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(install_cli).await.map_err(|e| e.to_string())?
}

#[cfg(not(desktop))]
#[tauri::command]
pub async fn agent_install_cli() -> Result<String, String> { Err("Not available on this device.".into()) }

#[cfg(all(desktop, unix))]
fn install_cli() -> Result<String, String> {
    let exe = app_binary().ok_or("Couldn't find DropBeam's own program file.")?;
    let link = |dest: &Path| -> std::io::Result<()> {
        if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
        if dest.symlink_metadata().is_ok() { std::fs::remove_file(dest)?; }
        std::os::unix::fs::symlink(&exe, dest)
    };
    if cfg!(target_os = "linux") && Path::new("/usr/bin/dropbeam").exists() {
        return Ok("The dropbeam command is already installed (/usr/bin/dropbeam).".into());
    }
    let system = Path::new("/usr/local/bin/dropbeam");
    if link(system).is_ok() { return Ok(format!("Installed: {}. Open a new terminal and type dropbeam.", system.display())); }
    // macOS: ask for an administrator password once, the way installers do.
    #[cfg(target_os = "macos")]
    {
        let quoted = |p: &Path| p.to_string_lossy().replace('\'', "'\\''");
        let script = format!("do shell script \"mkdir -p /usr/local/bin && ln -sf '{}' '{}'\" with administrator privileges",
            quoted(&exe).replace('"', "\\\""), quoted(system));
        if std::process::Command::new("osascript").args(["-e", &script]).status().is_ok_and(|s| s.success()) {
            return Ok(format!("Installed: {}. Open a new terminal and type dropbeam.", system.display()));
        }
    }
    let local = home().join(".local/bin/dropbeam");
    link(&local).map_err(|e| format!("Couldn't install the command: {e}"))?;
    let on_path = std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d) == local.parent().unwrap_or(Path::new("")));
    Ok(if on_path { format!("Installed: {}. Open a new terminal and type dropbeam.", local.display()) }
        else { format!("Installed: {}. Add ~/.local/bin to your PATH (or type the full path).", local.display()) })
}

#[cfg(windows)]
fn install_cli() -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let exe = app_binary().ok_or("Couldn't find DropBeam's own program file.")?;
    let dir = windows_bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A batch file waits for the program and passes its exit code back; the
    // app attaches to this console in command-line mode (main.rs).
    let shim = format!("@echo off\r\n\"{}\" cli %*\r\nexit /b %ERRORLEVEL%\r\n", exe.to_string_lossy());
    std::fs::write(dir.join("dropbeam.cmd"), shim).map_err(|e| e.to_string())?;
    let d = dir.to_string_lossy().replace('\'', "''");
    let ps = format!("$d='{d}'; $p=[Environment]::GetEnvironmentVariable('Path','User'); if (-not $p) {{ $p='' }}; \
        if (-not (($p -split ';') -contains $d)) {{ [Environment]::SetEnvironmentVariable('Path', (($p.TrimEnd(';') + ';' + $d).TrimStart(';')), 'User') }}");
    let ok = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(0x0800_0000).status().is_ok_and(|s| s.success());
    if !ok { return Err(format!("Saved {} but couldn't add it to PATH. Add that folder to PATH yourself.", dir.join("dropbeam.cmd").display())); }
    Ok(format!("Installed: {}. Open a new terminal and type dropbeam.", dir.join("dropbeam.cmd").display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dropbeam-agentctl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_default_to_access_on_and_commands_off() {
        let d = tmp();
        assert!(access_on(&d));
        assert_eq!(exec_allowed_now(&d), (false, None));
        save_settings(&d, &AgentSettings { access: false, exec_until: u64::MAX }).unwrap();
        assert!(!access_on(&d));
        assert_eq!(exec_allowed_now(&d), (cfg!(desktop), None));
        let soon = crate::chat::now_ms() + 60_000;
        save_settings(&d, &AgentSettings { access: true, exec_until: soon }).unwrap();
        assert_eq!(exec_allowed_now(&d), (true, Some(soon)));
        save_settings(&d, &AgentSettings { access: true, exec_until: crate::chat::now_ms() - 1 }).unwrap();
        assert!(!exec_allowed_now(&d).0, "an expired hour is off");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn log_rotates_and_reads_newest_last() {
        let d = tmp();
        for i in 0..3 { log_action(None, &d, json!({"op": "x", "n": i})); }
        let l = read_log(&d, 2);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1]["n"], 2);
        std::fs::write(d.join(LOG_FILE), vec![b' '; LOG_CAP as usize + 1]).unwrap();
        log_action(None, &d, json!({"op": "after"}));
        assert!(d.join(format!("{LOG_FILE}.1")).exists());
        assert_eq!(read_log(&d, 10).last().unwrap()["op"], "after");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn admit_refuses_when_off_unknown_or_unconfirmed() {
        let d = tmp();
        assert_eq!(admit(&d, "nope", &json!({})).unwrap_err().code, "unknown_op");
        let e = admit(&d, "friends.remove", &json!({"who": "x"})).unwrap_err();
        assert_eq!(e.code, "confirmation_required");
        assert!(e.preview.is_some());
        assert!(admit(&d, "friends.remove", &json!({"who": "x", "confirm": true})).is_ok());
        assert!(admit(&d, "friends.remove", &json!({"who": "x", "dryRun": true})).unwrap().1);
        save_settings(&d, &AgentSettings { access: false, exec_until: 0 }).unwrap();
        assert_eq!(admit(&d, "friends.list", &json!({})).unwrap_err().code, "forbidden");
        let _ = std::fs::remove_dir_all(d);
    }

    /// "via <agent>" is an optional extra: history from before has none, a
    /// message without it sends no `fromAgent`, and old builds (which read the
    /// chat frame field by field and don't deny unknown fields) skip it.
    #[test]
    fn agent_marker_is_optional_on_the_wire() {
        let old: crate::chat::ChatMessage = serde_json::from_value(json!({"id": "1", "peerId": "p", "fromMe": true, "kind": "text", "text": "hi", "ts": 1})).unwrap();
        assert!(old.from_agent.is_none());
        assert!(crate::iroh_net::chat_payload(&old, "p", "Me").get("fromAgent").is_none());
        assert!(serde_json::to_value(&old).unwrap().get("fromAgent").is_none(), "stored history unchanged");
        let mut m = old.clone();
        m.from_agent = AgentTag::sanitized("Claude");
        assert_eq!(crate::iroh_net::chat_payload(&m, "p", "Me")["fromAgent"]["name"], "Claude");
        let back: crate::chat::ChatMessage = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back.from_agent, m.from_agent);
        assert!(!include_str!("chat.rs").contains("deny_unknown_fields"));
    }

    #[test]
    fn agent_tag_is_bounded() {
        assert_eq!(AgentTag::sanitized("  \n "), None);
        assert_eq!(AgentTag::sanitized("Claude\u{7}Code").unwrap().name, "ClaudeCode");
        assert_eq!(AgentTag::sanitized(&"x".repeat(100)).unwrap().name.len(), 40);
    }

    /// The socket's folder and the socket itself are the owner's alone, and a
    /// request over it gets an answer line.
    #[cfg(unix)]
    #[test]
    fn socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        // A long config folder falls back to a short per-user path.
        assert!(socket_path(&PathBuf::from(format!("/{}", "x".repeat(120)))).to_string_lossy().starts_with("/tmp/dropbeam-"));
        let d = PathBuf::from(format!("/tmp/dbt-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(socket_dir(&d), d.join("agent"));
        let sd = socket_dir(&d);
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::set_permissions(&sd, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&sd).unwrap();
        assert_eq!(std::fs::metadata(&sd).unwrap().permissions().mode() & 0o777, 0o700, "an existing folder is tightened");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let path = socket_path(&d);
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            let client = tokio::spawn(async move { tokio::net::UnixStream::connect(&path).await.unwrap() });
            let (s, _) = listener.accept().await.unwrap();
            assert_eq!(s.peer_cred().unwrap().uid(), unsafe { libc::geteuid() });
            drop(client.await.unwrap());
        });
        let _ = std::fs::remove_dir_all(d);
    }
}
