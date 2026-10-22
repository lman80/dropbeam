//! Run a command on one of your OWN devices (#86): "install DropBeam on the new
//! computer, link it to your account by QR, turn on 'Let my other devices run
//! commands here' — now your agent can set it up (SSH keys, packages…)".
//!
//! A new stream kind `exec` on the existing iroh connection:
//!   → {"kind":"exec","v":1,"command","shell","cwd","timeoutSecs","agent":{name,reason}}
//!   ← {"kind":"exec-refused","code","message"}           (and the stream ends), or
//!   ← {"kind":"exec-started"} then {"t":"out"|"err","d":<base64>}… and finally
//!     {"t":"exit","code":N|null,"timedOut","canceled"}
//!   → while it runs: {"t":"in","d":<base64>}, {"t":"eof"}, {"t":"cancel"}
//! An older build answers the header with a bare {"kind":"ok"} (unknown kinds are
//! acknowledged and ignored) → "update DropBeam on that device".
//!
//! The target decides, every time, from facts it holds itself:
//! - the caller is one of THIS account's linked devices — judged by the
//!   connection's authenticated `remote_id()` against the account's device list
//!   (never an id the caller claims), so a friend is always refused;
//! - "Let my other devices run commands here" is on (default off; "for 1 hour"
//!   or "until I turn it off") — a desktop-only switch only the window can set;
//! - not iOS / not the windowless `--server` mode.
//!
//! Each command shows a notification on the target and lands in its agent log.

use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::agent_ops::{Ctx, OpError, R};
use crate::iroh_net::{read_frame, write_frame, IrohState};

const DEFAULT_TIMEOUT: u64 = 300;
const MAX_TIMEOUT: u64 = 86_400;
const MAX_COMMAND: usize = 64 << 10;
const COLLECT_CAP: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal { NotOwnDevice, TurnedOff, Unsupported }

impl Refusal {
    pub fn code(self) -> &'static str {
        match self { Refusal::NotOwnDevice => "not_own_device", Refusal::TurnedOff => "turned_off", Refusal::Unsupported => "unsupported" }
    }
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NotOwnDevice => "That device only runs commands from devices linked to its own DropBeam account.",
            Refusal::TurnedOff => "That device doesn't allow commands right now. On it, open DropBeam → Settings → Agents and turn on “Let my other devices run commands here”.",
            Refusal::Unsupported => "That device can't run commands (only DropBeam on a Mac, Windows or Linux computer with its window app running can).",
        }
    }
}

/// The whole decision, kept pure so every case is tested.
pub fn authorize(own_device: bool, supported: bool, exec_until: u64, now: u64) -> Result<(), Refusal> {
    if !own_device { return Err(Refusal::NotOwnDevice); }
    if !supported { return Err(Refusal::Unsupported); }
    if exec_until <= now { return Err(Refusal::TurnedOff); }
    Ok(())
}

// ── the device that runs it ─────────────────────────────────────────────────

async fn refuse(send: &mut iroh::endpoint::SendStream, r: Refusal) -> anyhow::Result<()> {
    write_frame(send, &json!({"kind": "exec-refused", "code": r.code(), "message": r.message()})).await?;
    let _ = send.finish();
    Ok(())
}

async fn read_some<T: AsyncRead + Unpin>(r: &mut Option<T>, buf: &mut [u8]) -> std::io::Result<usize> {
    match r { Some(r) => r.read(buf).await, None => std::future::pending().await }
}

fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The command runs in its own process group: stop everything it started.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(0x0800_0000).status();
    }
    let _ = child.start_kill();
}

fn shell_command(shell: &str, command: &str) -> Result<tokio::process::Command, String> {
    let (prog, args): (&str, Vec<&str>) = match shell {
        "" if cfg!(windows) => ("cmd", vec!["/C", command]),
        "" | "sh" => ("/bin/sh", vec!["-c", command]),
        "bash" => ("bash", vec!["-c", command]),
        "cmd" => ("cmd", vec!["/C", command]),
        "powershell" => (if cfg!(windows) { "powershell" } else { "pwsh" }, vec!["-NoProfile", "-NonInteractive", "-Command", command]),
        other => return Err(format!("unknown shell {other:?} (sh, bash, cmd or powershell)")),
    };
    let mut c = tokio::process::Command::new(prog);
    c.args(args);
    Ok(c)
}

/// Serve an `exec` stream from `who` (the connection's authenticated id).
pub async fn serve(state: &IrohState, who: &str, req: &Value, send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream) -> anyhow::Result<()> {
    let Some(app) = state.app.get().cloned() else { return refuse(send, Refusal::Unsupported).await };
    let Some(st) = app.try_state::<Arc<crate::AppState>>().map(|s| s.inner().clone()) else { return refuse(send, Refusal::Unsupported).await };
    let dir = st.config_dir.clone();
    let own = crate::account::is_own_device(&dir, who);
    let from = crate::friends::label_for_endpoint(&dir, who).unwrap_or_else(|| format!("device {}", &who[..who.len().min(8)]));
    let agent = req["agent"]["name"].as_str().map(|n| n.chars().filter(|c| !c.is_control()).take(60).collect::<String>()).unwrap_or_default();
    let command: String = req["command"].as_str().unwrap_or("").chars().take(MAX_COMMAND).collect();
    let by = if agent.is_empty() { from.clone() } else { format!("{agent} on {from}") };
    let verdict = authorize(own, cfg!(desktop), crate::agent_ctl::load_settings(&dir).exec_until, crate::chat::now_ms());
    if let Err(r) = verdict {
        crate::agent_ctl::log_action(Some(&app), &dir, json!({"agent": by, "op": "exec (from another device)", "risk": "destructive",
            "summary": command_line(&command), "ok": false, "error": r.code()}));
        log::info!("exec: refused a command from {} ({})", if own { "an own device" } else { "a device outside the account" }, r.code());
        return refuse(send, r).await;
    }
    if command.trim().is_empty() {
        write_frame(send, &json!({"kind": "exec-refused", "code": "invalid_input", "message": "No command given."})).await?;
        let _ = send.finish();
        return Ok(());
    }
    let mut cmd = match shell_command(req["shell"].as_str().unwrap_or(""), &command) {
        Ok(c) => c,
        Err(e) => {
            write_frame(send, &json!({"kind": "exec-refused", "code": "invalid_input", "message": e})).await?;
            let _ = send.finish();
            return Ok(());
        }
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from);
    match req["cwd"].as_str().filter(|c| !c.is_empty()) {
        Some(c) => { cmd.current_dir(c); }
        None => if let Some(h) = &home { cmd.current_dir(h); },
    }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            write_frame(send, &json!({"kind": "exec-refused", "code": "failed", "message": format!("Couldn't start it: {e}")})).await?;
            let _ = send.finish();
            return Ok(());
        }
    };
    notify(&app, &by, &command);
    log::info!("exec: running a command from an own device");
    write_frame(send, &json!({"kind": "exec-started"})).await?;
    let timeout = Duration::from_secs(req["timeoutSecs"].as_u64().unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT));
    let started = std::time::Instant::now();

    let (ctl_tx, mut ctl_rx) = mpsc::channel::<Value>(16);
    let reader = async move {
        while let Ok(v) = read_frame(recv).await {
            if ctl_tx.send(v).await.is_err() { break; }
        }
        drop(ctl_tx);
        std::future::pending::<()>().await
    };
    let work = async {
        let (mut out, mut err, mut stdin) = (child.stdout.take(), child.stderr.take(), child.stdin.take());
        let (mut obuf, mut ebuf) = (vec![0u8; 32 << 10], vec![0u8; 32 << 10]);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let (mut timed_out, mut canceled, mut ctl_open) = (false, false, true);
        let status = loop {
            tokio::select! {
                n = read_some(&mut out, &mut obuf) => match n {
                    Ok(0) | Err(_) => out = None,
                    Ok(n) => if write_frame(send, &json!({"t": "out", "d": STANDARD.encode(&obuf[..n])})).await.is_err() { kill_tree(&mut child); canceled = true; out = None; },
                },
                n = read_some(&mut err, &mut ebuf) => match n {
                    Ok(0) | Err(_) => err = None,
                    Ok(n) => if write_frame(send, &json!({"t": "err", "d": STANDARD.encode(&ebuf[..n])})).await.is_err() { kill_tree(&mut child); canceled = true; err = None; },
                },
                c = ctl_rx.recv(), if ctl_open => match c {
                    None => ctl_open = false,
                    Some(v) => match v["t"].as_str() {
                        Some("in") => {
                            let bytes = v["d"].as_str().and_then(|d| STANDARD.decode(d).ok()).unwrap_or_default();
                            if let Some(s) = stdin.as_mut() { if s.write_all(&bytes).await.is_err() { stdin = None; } }
                        }
                        Some("eof") => stdin = None,
                        Some("cancel") => { kill_tree(&mut child); canceled = true; }
                        _ => {}
                    },
                },
                _ = &mut deadline, if !timed_out => { kill_tree(&mut child); timed_out = true; },
                s = child.wait(), if out.is_none() && err.is_none() => break s,
            }
        };
        let code = status.ok().and_then(|s| s.code());
        let _ = write_frame(send, &json!({"t": "exit", "code": code, "timedOut": timed_out, "canceled": canceled})).await;
        let _ = send.finish();
        (code, timed_out, canceled)
    };
    let (code, timed_out, canceled) = tokio::select! { r = work => r, _ = reader => unreachable!() };
    crate::agent_ctl::log_action(Some(&app), &dir, json!({"agent": by, "op": "exec (from another device)", "risk": "destructive",
        "summary": command_line(&command), "ok": code == Some(0), "exitCode": code, "timedOut": timed_out, "canceled": canceled,
        "ms": started.elapsed().as_millis() as u64}));
    Ok(())
}

fn command_line(c: &str) -> String {
    let one: String = c.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 160 { format!("{}…", one.chars().take(160).collect::<String>()) } else { one }
}

fn notify(app: &AppHandle, by: &str, command: &str) {
    #[cfg(desktop)]
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("DropBeam ran a command")
            .body(format!("{by}: {}", command_line(command))).show();
    }
    #[cfg(not(desktop))]
    let _ = (app, by, command);
}

// ── the device that asks ────────────────────────────────────────────────────

/// Run `exec.run` on an own device. `on_event` gets {"type":"stdout"|"stderr","b64"}
/// as output arrives; `control` carries {stdin|eof|cancel} from a streaming
/// client. Without `control` the output is also collected into the result.
pub async fn run_remote(app: &AppHandle, i: &Value, ctx: &Ctx, control: Option<mpsc::Receiver<Value>>, mut on_event: impl FnMut(Value) + Send) -> R {
    let st = app.state::<Arc<crate::AppState>>().inner().clone();
    let net = app.state::<Arc<IrohState>>().inner().clone();
    let who = i["device"].as_str().unwrap_or("");
    // Debug builds only: let a test aim at a FRIEND to prove the other side refuses.
    let any_peer = cfg!(debug_assertions) && std::env::var_os("DROPBEAM_TEST_EXEC_TO_ANY").is_some();
    let device = if any_peer { crate::agent_ops::resolve(&st.config_dir, who)? } else { crate::agent_ops::resolve_own_device(&st.config_dir, who)? };
    let endpoint = device.endpoint_id.clone().ok_or_else(|| OpError::new("not_found", "that device has no address yet"))?;
    let command = i["command"].as_str().unwrap_or("").to_owned();
    if command.len() > MAX_COMMAND { return Err(OpError::new("invalid_input", "command is too long")); }
    if ctx.dry_run { return Ok(json!({"dryRun": true, "wouldDo": {"device": device.name, "command": command}})); }
    let conn = crate::iroh_net::friend_connection(&net, &endpoint).await
        .map_err(|e| OpError::new("unreachable", format!("Couldn't reach {}: {e}. Is DropBeam open on it?", device.name)))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| OpError::new("unreachable", e.to_string()))?;
    let failed = |e: anyhow::Error| OpError::new("failed", format!("the connection to {} dropped: {e}", device.name));
    write_frame(&mut send, &json!({"kind": "exec", "v": 1, "command": command, "shell": i["shell"], "cwd": i["cwd"],
        "timeoutSecs": i["timeoutSecs"].as_u64().unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT),
        "agent": {"name": ctx.agent, "reason": ctx.reason}})).await.map_err(failed)?;
    let first = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut recv)).await
        .map_err(|_| OpError::new("timeout", format!("{} didn't answer", device.name)))?.map_err(failed)?;
    match first["kind"].as_str() {
        Some("exec-started") => {}
        Some("exec-refused") => {
            let code = match first["code"].as_str() { Some("not_own_device") => "forbidden", Some("turned_off") => "turned_off", Some("unsupported") => "unsupported", Some("invalid_input") => "invalid_input", _ => "refused" };
            return Err(OpError::new(code, first["message"].as_str().unwrap_or("refused").to_owned()));
        }
        _ => return Err(OpError::new("needs_update", format!("{} runs an older DropBeam that can't run commands. Update it first.", device.name))),
    }
    let collect = control.is_none();
    let (frames_tx, mut frames) = mpsc::channel::<anyhow::Result<Value>>(64);
    let reader = tauri::async_runtime::spawn(async move {
        loop {
            let f = read_frame(&mut recv).await;
            let end = f.is_err() || f.as_ref().is_ok_and(|v| v["t"] == "exit");
            if frames_tx.send(f).await.is_err() || end { break; }
        }
    });
    // Input given up front goes first, then end-of-input (unless the client streams it).
    if let Some(text) = i["stdin"].as_str() {
        for chunk in text.as_bytes().chunks(32 << 10) {
            write_frame(&mut send, &json!({"t": "in", "d": STANDARD.encode(chunk)})).await.map_err(failed)?;
        }
    }
    if control.is_none() || i["stdin"].is_string() { write_frame(&mut send, &json!({"t": "eof"})).await.map_err(failed)?; }
    let mut control = control;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut truncated = false;
    let result = loop {
        let next = tokio::select! {
            f = frames.recv() => f,
            c = async { match control.as_mut() { Some(c) => c.recv().await, None => std::future::pending().await } } => {
                match c {
                    None => { control = None; let _ = write_frame(&mut send, &json!({"t": "cancel"})).await; }
                    Some(v) => {
                        let frame = if v["cancel"] == true { json!({"t": "cancel"}) } else if v["eof"] == true { json!({"t": "eof"}) }
                            else if let Some(d) = v["stdin"].as_str() { json!({"t": "in", "d": d}) } else { continue };
                        let _ = write_frame(&mut send, &frame).await;
                    }
                }
                continue;
            }
        };
        let Some(f) = next else { break Err(OpError::new("failed", format!("the connection to {} closed before the command finished", device.name))) };
        let v = f.map_err(failed)?;
        match v["t"].as_str() {
            Some(t @ ("out" | "err")) => {
                let d = v["d"].as_str().unwrap_or("");
                if collect {
                    let bytes = STANDARD.decode(d).unwrap_or_default();
                    let buf = if t == "out" { &mut out } else { &mut err };
                    let room = COLLECT_CAP.saturating_sub(buf.len());
                    if bytes.len() > room { truncated = true; }
                    buf.extend_from_slice(&bytes[..bytes.len().min(room)]);
                }
                on_event(json!({"type": if t == "out" { "stdout" } else { "stderr" }, "b64": d}));
            }
            Some("exit") => {
                let mut r = json!({"device": device.name, "exitCode": v["code"], "timedOut": v["timedOut"], "canceled": v["canceled"]});
                if collect {
                    r["stdout"] = json!(String::from_utf8_lossy(&out));
                    r["stderr"] = json!(String::from_utf8_lossy(&err));
                    if truncated { r["truncated"] = json!(true); }
                }
                break Ok(r);
            }
            _ => {}
        }
    };
    let _ = send.finish();
    reader.abort();
    result
}

/// `exec.run` without streaming (MCP, `dropbeam op exec.run`).
pub async fn run_collect(app: &AppHandle, i: &Value, ctx: &Ctx) -> R {
    run_remote(app, i, ctx, None, |_| {}).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every combination: only an own device, on a supported device, with the
    /// switch on (and not expired) gets a command run.
    #[test]
    fn authorization_matrix() {
        let now = 1_000_000;
        for own in [false, true] {
            for supported in [false, true] {
                for (until, on) in [(0u64, false), (now - 1, false), (now, false), (now + 3_600_000, true), (u64::MAX, true)] {
                    let got = authorize(own, supported, until, now);
                    let want = if !own { Err(Refusal::NotOwnDevice) } else if !supported { Err(Refusal::Unsupported) }
                        else if !on { Err(Refusal::TurnedOff) } else { Ok(()) };
                    assert_eq!(got, want, "own={own} supported={supported} until={until}");
                }
            }
        }
        // A friend is told the same thing whatever the switch says: nothing leaks.
        assert_eq!(authorize(false, true, u64::MAX, now).unwrap_err().message(), authorize(false, true, 0, now).unwrap_err().message());
    }

    #[test]
    fn shells_are_known() {
        assert!(shell_command("sh", "true").is_ok());
        assert!(shell_command("powershell", "x").is_ok());
        assert!(shell_command("zsh; rm", "x").is_err());
        assert_eq!(command_line("a\n  b"), "a b");
    }
}
