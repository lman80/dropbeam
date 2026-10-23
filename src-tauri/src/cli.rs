//! `dropbeam …` — the command line and the MCP server, for people and AI agents.
//! Both are thin clients of the running app's control API (agent_ctl.rs): every
//! command becomes an op from the one catalog (agent_ops.rs). If the app isn't
//! running, it is started in the background (menu bar / tray) first.
//!
//! The same program file is the app: `main.rs` sends `DropBeam cli …` and any
//! launch through a file named exactly `dropbeam` (the installed command) here,
//! before the window machinery starts, so GUI launches are untouched.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::agent_ops::{Risk, CATALOG};

const HELP: &str = "\
dropbeam — send files, chat and control DropBeam from a terminal or an AI agent.

  dropbeam status                        this device, network, linked devices
  dropbeam friends                       your friends and your own devices
  dropbeam devices                       devices linked to your account
  dropbeam send <who> <paths…>           send files/folders (waits until done)
  dropbeam chat <who> <text…>            send a message (use - to read it from stdin)
  dropbeam read <who> [--since 10m]      show messages
  dropbeam wait [--from <who>] [--timeout 60]   wait for a new message (exit 124 on timeout)
  dropbeam ls <who>:<location>[/path]    list a friend's shared Location
  dropbeam get <who>:<location>/<path>…  download from a Location
  dropbeam put <paths…> <who>:<location>[/path]   upload into a Location
  dropbeam quicksend <paths…>            make a one-time download code
  dropbeam receive <code>                download a Quick Send
  dropbeam transfers                     transfers since DropBeam started
  dropbeam exec <device> -- <command…>   run a command on one of YOUR devices
  dropbeam events [--types chat,transfer]   stream live events as JSON lines
  dropbeam ops                           every op an agent can call
  dropbeam op <name> [--input '{…}'] [--confirm] [--dry-run]   call any op
  dropbeam mcp                           MCP server on stdin/stdout (for AI agents)

Options: --json (machine-readable output), --agent <name> and --reason <why>
(shown in DropBeam → Settings → Agents), --no-wait, --no-start (don't start the app).
<who> is a friend's name, one of your devices' names, or an id.
";

const MCP_INSTRUCTIONS: &str = "\
DropBeam sends files and messages directly between devices (peer to peer, end-to-end encrypted). \
You act for the person who owns this computer, with the same abilities they have in the DropBeam window.

Who is who: `friends_list` shows FRIENDS (other people) and the person's OWN DEVICES (ownDevice: true — \
their other computers and phone, linked to the same DropBeam account). Name anyone by their name or id.

Files: `files_send` {to, paths} sends files or folders (full paths on this computer) to a friend or own device; \
follow it with `transfers_wait`. `quicksend_create` makes a one-time code anyone can download with.

Messages and other agents: `chat_send` {to, text} sends a message — it's shown as sent via your name. \
To talk with another agent (or a person) on another computer, send with `chat_send`, then call `chat_wait` \
{from, timeoutSecs} to wait for the reply; repeat. Messages arrive whenever both devices are online.

Own devices only: `exec_run` {device, command} runs a shell command on one of the person's OWN devices, \
only if that device has 'Let my other devices run commands here' turned on (Settings → Agents on it). \
Never on a friend's device. Use it to set up a new computer of theirs (for example to install SSH keys).

Safety: tools marked destructive need confirm: true — ask the person first. Many accept dryRun: true. \
Add `reason` to any call: it's shown to the person in their activity log with your name.";

// ── arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub cmd: String,
    pub pos: Vec<String>,
    /// --flag value / --flag (as "true").
    pub flags: Vec<(String, String)>,
    /// Everything after `--` (exec's command).
    pub rest: Vec<String>,
}

/// Flags that never take a value.
const SWITCHES: &[&str] = &["json", "no-wait", "no-start", "confirm", "dry-run", "help", "active"];

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        if arg == "--" { a.rest = it.by_ref().cloned().collect(); break; }
        if let Some(f) = arg.strip_prefix("--") {
            let (k, v) = match f.split_once('=') { Some((k, v)) => (k.to_owned(), Some(v.to_owned())), None => (f.to_owned(), None) };
            let v = match v {
                Some(v) => v,
                None if SWITCHES.contains(&k.as_str()) => "true".into(),
                None => it.next().cloned().ok_or_else(|| format!("--{k} needs a value"))?,
            };
            a.flags.push((k, v));
        } else if arg == "-h" {
            a.flags.push(("help".into(), "true".into()));
        } else if a.cmd.is_empty() {
            a.cmd = arg.clone();
        } else {
            a.pos.push(arg.clone());
        }
    }
    Ok(a)
}

impl Args {
    fn flag(&self, k: &str) -> Option<&str> { self.flags.iter().rev().find(|f| f.0 == k).map(|f| f.1.as_str()) }
    fn on(&self, k: &str) -> bool { self.flag(k).is_some_and(|v| v != "false") }
}

/// `who:location/path` → (who, location, path).
pub fn split_target(t: &str) -> Option<(String, String, String)> {
    let (who, rest) = t.split_once(':')?;
    let (loc, path) = rest.split_once('/').unwrap_or((rest, ""));
    (!who.is_empty() && !loc.is_empty()).then(|| (who.to_owned(), loc.to_owned(), path.trim_matches('/').to_owned()))
}

/// "10m", "2h", "90s", "3d" or milliseconds since 1970 → ms since 1970.
pub fn parse_since(s: &str, now_ms: u64) -> Option<u64> {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().ok()?;
    let secs = match unit { "" => return Some(n), "s" => n, "m" => n * 60, "h" => n * 3600, "d" => n * 86_400, _ => return None };
    Some(now_ms.saturating_sub(secs * 1000))
}

fn absolute(p: &str) -> String {
    std::path::absolute(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_owned())
}

// ── talking to the app ──────────────────────────────────────────────────────

trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

fn config_dir() -> PathBuf { crate::headless::default_config_dir() }

async fn connect_once(config: &Path) -> std::io::Result<Box<dyn Duplex>> {
    // Trust only an endpoint this user made and an app running as this user.
    #[cfg(unix)]
    {
        if let Err(e) = crate::agent_ctl::check_private_dir(&crate::agent_ctl::socket_dir(config)) {
            // Not there yet = not running (start it); there but not ours = refuse.
            return Err(if e.kind() == std::io::ErrorKind::NotFound { e } else { std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()) });
        }
        let s = tokio::net::UnixStream::connect(crate::agent_ctl::socket_path(config)).await?;
        crate::agent_ctl::check_peer(&s)?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let name = std::fs::read_to_string(crate::agent_ctl::pipe_name_file(config))?;
        let name = name.trim();
        if !name.starts_with(r"\\.\pipe\dropbeam-agent-") { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the agent pipe name isn't DropBeam's")); }
        for _ in 0..50 {
            match ClientOptions::new().open(name) {
                Ok(c) => { crate::agent_ctl::check_pipe_server(&c)?; return Ok(Box::new(c)) }
                Err(e) if e.raw_os_error() == Some(231) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("the DropBeam pipe stayed busy"))
    }
}

/// Start the app in the background (no window) the way a login launch does.
/// Returns the started process when we can watch it (not via `open` on macOS).
fn start_app() -> Result<Option<std::process::Child>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    #[cfg(target_os = "macos")]
    if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        let ok = std::process::Command::new("open").args(["-g", "-j"]).arg(bundle).args(["--args", "--minimized"])
            .status().is_ok_and(|s| s.success());
        return if ok { Ok(None) } else { Err("couldn't open DropBeam".into()) };
    }
    #[cfg(target_os = "linux")]
    if ["DISPLAY", "WAYLAND_DISPLAY"].iter().all(|v| std::env::var_os(v).is_none_or(|d| d.is_empty())) {
        return Err("DropBeam isn't running, and this computer has no desktop session to start it in.\n\
            Start it in a session (for example over SSH: `xvfb-run -a DropBeam --minimized &`, or log in \
            to the desktop once with “Open at login” on), then run this again.".into());
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--minimized").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200);
    cmd.spawn().map(Some).map_err(|e| format!("couldn't start DropBeam: {e}"))
}

async fn connect(config: &Path, may_start: bool, quiet: bool) -> Result<Box<dyn Duplex>, String> {
    let untrusted = |e: std::io::Error| format!("Refusing to talk to that DropBeam: {e}.");
    match connect_once(config).await {
        Ok(c) => return Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(untrusted(e)),
        Err(_) => {}
    }
    if !may_start { return Err("DropBeam isn't running (start it, or leave out --no-start).".into()); }
    if !quiet { eprintln!("Starting DropBeam…"); }
    let mut child = start_app()?;
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        match connect_once(config).await {
            Ok(c) => return Ok(c),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(untrusted(e)),
            Err(_) => {}
        }
        if let Some(Ok(Some(status))) = child.as_mut().map(|c| c.try_wait()) {
            return Err(format!("DropBeam stopped right after starting ({status}). Open it once normally to see why."));
        }
    }
    Err("DropBeam started but didn't answer within a minute. Is agent access on (Settings → Agents)?".into())
}

struct Client {
    rd: BufReader<tokio::io::ReadHalf<Box<dyn Duplex>>>,
    wr: tokio::io::WriteHalf<Box<dyn Duplex>>,
    next: u64,
    agent: Value,
}

impl Client {
    fn new(conn: Box<dyn Duplex>, agent: Value) -> Self {
        let (rd, wr) = tokio::io::split(conn);
        Client { rd: BufReader::new(rd), wr, next: 1, agent }
    }
    async fn send_line(&mut self, v: &Value) -> Result<(), String> {
        let mut l = v.to_string();
        l.push('\n');
        self.wr.write_all(l.as_bytes()).await.map_err(|e| format!("lost DropBeam: {e}"))
    }
    async fn read_line(&mut self) -> Result<Value, String> {
        let mut l = String::new();
        if self.rd.read_line(&mut l).await.map_err(|e| e.to_string())? == 0 { return Err("DropBeam closed the connection".into()); }
        serde_json::from_str(&l).map_err(|e| e.to_string())
    }
    /// One op; returns the final reply line ({ok, data|error}).
    async fn call(&mut self, op: &str, input: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        self.send_line(&json!({"id": id, "op": op, "input": input, "agent": self.agent})).await?;
        loop {
            let v = self.read_line().await?;
            if v["id"] == id && v.get("event").is_none() { return Ok(v); }
        }
    }
}

/// An op's data, or its error as one line of text.
async fn data(c: &mut Client, op: &str, input: Value) -> Result<Value, String> {
    let r = c.call(op, input).await?;
    if r["ok"] == true { return Ok(r["data"].clone()); }
    let e = &r["error"];
    let mut msg = e["message"].as_str().unwrap_or("failed").to_owned();
    if e["code"] == "confirmation_required" { msg.push_str(" (add --confirm)"); }
    Err(msg)
}

// ── output helpers ──────────────────────────────────────────────────────────

fn size(b: u64) -> String {
    let f = b as f64;
    match b {
        0..=999 => format!("{b} B"),
        1_000..=999_999 => format!("{:.1} KB", f / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} MB", f / 1e6),
        _ => format!("{:.2} GB", f / 1e9),
    }
}

fn when(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms as i64).single().map(|t| t.format("%b %e %H:%M").to_string()).unwrap_or_default()
}

fn short(id: &str) -> &str { &id[..id.len().min(8)] }

fn pretty(v: &Value) -> String { serde_json::to_string_pretty(v).unwrap_or_default() }

// ── commands ────────────────────────────────────────────────────────────────

/// Entry point: `args` are the words after `dropbeam` (or `DropBeam cli`).
pub fn run(args: &[String]) -> i32 {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => { eprintln!("dropbeam: {e}"); return 2; }
    };
    if a.cmd.is_empty() || a.cmd == "help" || (a.on("help") && a.cmd != "op") { print!("{HELP}"); return 0; }
    if a.cmd == "version" || a.cmd == "--version" { println!("dropbeam {}", env!("CARGO_PKG_VERSION")); return 0; }
    if a.cmd == "ops" { return ops(&a); }
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => { eprintln!("dropbeam: {e}"); return 1; }
    };
    if a.cmd == "mcp" { return rt.block_on(mcp(!a.on("no-start"))); }
    rt.block_on(async {
        match command(&a).await {
            Ok(code) => code,
            Err(e) => { eprintln!("dropbeam: {e}"); 1 }
        }
    })
}

fn ops(a: &Args) -> i32 {
    if a.on("json") { println!("{}", pretty(&json!(&*CATALOG))); return 0; }
    for o in CATALOG.iter() {
        let risk = match o.risk { Risk::Read => "read", Risk::Write => "write", Risk::Destructive => "DESTRUCTIVE" };
        println!("{:<26} {:<11} {}", o.name, risk, o.description);
    }
    0
}

fn agent_of(a: &Args) -> Value {
    let name = a.flag("agent").map(str::to_owned).or_else(|| std::env::var("DROPBEAM_AGENT").ok()).unwrap_or_else(|| "command line".into());
    let reason = a.flag("reason").map(str::to_owned).or_else(|| std::env::var("DROPBEAM_REASON").ok());
    json!({"name": name, "reason": reason})
}

async fn command(a: &Args) -> Result<i32, String> {
    let need = |n: usize, usage: &str| if a.pos.len() < n { Err(format!("usage: dropbeam {usage}")) } else { Ok(()) };
    // Check usage before possibly starting the app.
    match a.cmd.as_str() {
        "send" => need(2, "send <who> <paths…>")?,
        "chat" => need(2, "chat <who> <text…>")?,
        "read" => need(1, "read <who>")?,
        "ls" => need(1, "ls <who>:<location>[/path]")?,
        "get" => need(1, "get <who>:<location>/<path>…")?,
        "put" => need(2, "put <paths…> <who>:<location>[/path]")?,
        "quicksend" => need(1, "quicksend <paths…>")?,
        "receive" => need(1, "receive <code>")?,
        "op" => need(1, "op <name> [--input '{…}']")?,
        "exec" => if a.pos.is_empty() || a.rest.is_empty() { return Err("usage: dropbeam exec <device> -- <command…>".into()); },
        "status" | "friends" | "devices" | "wait" | "transfers" | "events" => {}
        other => return Err(format!("unknown command {other:?} (dropbeam help)")),
    }
    let json_out = a.on("json");
    let conn = connect(&config_dir(), !a.on("no-start"), json_out).await?;
    let mut c = Client::new(conn, agent_of(a));
    let show = |v: &Value| println!("{}", pretty(v));
    match a.cmd.as_str() {
        "status" => {
            let s = data(&mut c, "status", json!({})).await?;
            if json_out { show(&s); return Ok(0); }
            println!("DropBeam {} — {} ({})", s["version"].as_str().unwrap_or("?"), s["deviceName"].as_str().unwrap_or("?"), short(s["endpointId"].as_str().unwrap_or("")));
            println!("Name friends see: {}", s["displayName"].as_str().unwrap_or(""));
            println!("Network: {}", if s["ready"] == true { "ready" } else { "starting" });
            println!("Your other linked devices: {}", s["linkedDevices"]);
            println!("Commands from your other devices: {}", if s["commandsAllowedHere"] == true {
                s["commandsAllowedUntil"].as_u64().map(|u| format!("allowed until {}", when(u))).unwrap_or_else(|| "allowed".into()) } else { "off".into() });
        }
        "friends" => {
            let list = data(&mut c, "friends.list", json!({})).await?;
            if json_out { show(&list); return Ok(0); }
            let rows = list.as_array().cloned().unwrap_or_default();
            if rows.is_empty() { println!("No friends yet. Share your code: dropbeam op friends.myCode"); }
            for f in rows {
                let kind = if f["ownDevice"] == true { "your device" } else { "friend" };
                let model = f["deviceModel"].as_str().or(f["deviceKind"].as_str()).unwrap_or("");
                println!("{:<28} {:<12} {:<14} {}", f["name"].as_str().unwrap_or(""), kind, model, short(f["endpointId"].as_str().unwrap_or("")));
            }
        }
        "devices" => {
            let d = data(&mut c, "devices.list", json!({})).await?;
            if json_out { show(&d); return Ok(0); }
            let devs = d["devices"].as_array().cloned().unwrap_or_default();
            if devs.len() <= 1 { println!("This device isn't linked to others yet (DropBeam → Settings → Devices)."); }
            for x in devs {
                let note = if x["this_device"] == true { " (this device)" } else if x["needs_approval"] == true { " (needs approval)" } else { "" };
                println!("{:<30} {:<10} {}{}", x["name"].as_str().unwrap_or(""), x["device_os"].as_str().unwrap_or(""), short(x["endpoint_id"].as_str().unwrap_or("")), note);
            }
        }
        "send" => {
            let paths: Vec<String> = a.pos[1..].iter().map(|p| absolute(p)).collect();
            let mut input = json!({"to": a.pos[0], "paths": paths});
            if let Some(d) = a.flag("device") { input["device"] = json!(d); }
            if a.on("dry-run") { input["dryRun"] = json!(true); }
            let t = data(&mut c, "files.send", input).await?;
            return finish_transfer(&mut c, a, t, "Sent").await;
        }
        "chat" => {
            let mut text = a.pos[1..].join(" ");
            if text == "-" { text.clear(); std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?; }
            let m = data(&mut c, "chat.send", json!({"to": a.pos[0], "text": text})).await?;
            if json_out { show(&m) } else { println!("Sent."); }
        }
        "read" => {
            let mut input = json!({"who": a.pos[0]});
            if let Some(s) = a.flag("since") { input["since"] = json!(parse_since(s, crate::chat::now_ms()).ok_or("--since: use 10m, 2h, 3d or a time in ms")?); }
            if let Some(l) = a.flag("limit") { input["limit"] = json!(l.parse::<u64>().map_err(|_| "--limit: a number")?); }
            let msgs = data(&mut c, "chat.read", input).await?;
            if json_out { show(&msgs); return Ok(0); }
            for m in msgs.as_array().cloned().unwrap_or_default() { print_message(&m, &a.pos[0]); }
        }
        "wait" => {
            let mut input = json!({"timeoutSecs": a.flag("timeout").and_then(|t| t.parse::<u64>().ok()).unwrap_or(60)});
            if let Some(f) = a.flag("from") { input["from"] = json!(f); }
            if let Some(s) = a.flag("since") { input["since"] = json!(parse_since(s, crate::chat::now_ms()).ok_or("--since: use 10m, 2h or ms")?); }
            let r = data(&mut c, "chat.wait", input).await?;
            if json_out { show(&r); } else {
                for m in r["messages"].as_array().cloned().unwrap_or_default() { let who = m["fromName"].as_str().unwrap_or("").to_owned(); print_message(&m, &who); }
            }
            if r["timedOut"] == true { if !json_out { eprintln!("No new message."); } return Ok(124); }
        }
        "ls" => {
            let (who, loc, path) = split_target(&a.pos[0]).ok_or("use <who>:<location>[/path]")?;
            let mut input = json!({"who": who, "location": loc, "path": path});
            let mut all = vec![];
            loop {
                let page = data(&mut c, "locations.ls", input.clone()).await?;
                all.extend(page["entries"].as_array().cloned().unwrap_or_default());
                match page["nextCursor"].as_str() { Some(n) => input["cursor"] = json!(n), None => break }
            }
            if json_out { show(&json!(all)); return Ok(0); }
            for e in all {
                let name = e["name"].as_str().unwrap_or("");
                if e["isDir"] == true { println!("{:>10}  {:<16} {name}/", "", when(e["modified"].as_u64().unwrap_or(0))); }
                else { println!("{:>10}  {:<16} {name}", size(e["size"].as_u64().unwrap_or(0)), when(e["modified"].as_u64().unwrap_or(0))); }
            }
        }
        "get" => {
            let mut target = None;
            let mut paths = vec![];
            for p in &a.pos {
                let (who, loc, path) = split_target(p).ok_or("use <who>:<location>/<path>")?;
                if target.as_ref().is_some_and(|t: &(String, String)| *t != (who.clone(), loc.clone())) { return Err("get from one Location at a time".into()); }
                target = Some((who, loc));
                paths.push(path);
            }
            let (who, loc) = target.ok_or("nothing to get")?;
            let r = data(&mut c, "locations.download", json!({"who": who, "location": loc, "paths": paths})).await?;
            match (r["receiveId"].as_str(), r["transferId"].as_str()) {
                (Some(id), _) => { let id = id.to_owned(); return finish_transfer(&mut c, a, json!({"id": id}), "Downloaded").await; }
                (None, Some(_)) => { if json_out { show(&r) } else { println!("Started; see dropbeam transfers."); } }
                (None, None) => { if json_out { show(&r) } else { println!("Nothing to download."); } }
            }
        }
        "put" => {
            let (who, loc, path) = split_target(a.pos.last().map(String::as_str).unwrap_or("")).ok_or("the last argument is where: <who>:<location>[/path]")?;
            let paths: Vec<String> = a.pos[..a.pos.len() - 1].iter().map(|p| absolute(p)).collect();
            let t = data(&mut c, "locations.upload", json!({"who": who, "location": loc, "path": path, "paths": paths})).await?;
            return finish_transfer(&mut c, a, t, "Uploaded").await;
        }
        "quicksend" => {
            let paths: Vec<String> = a.pos.iter().map(|p| absolute(p)).collect();
            let t = data(&mut c, "quicksend.create", json!({"paths": paths})).await?;
            if json_out { show(&t); return Ok(0); }
            println!("{}", t["code"].as_str().unwrap_or(""));
            eprintln!("Anyone with DropBeam can download with: dropbeam receive <code>. Keep DropBeam open until they have.");
        }
        "receive" => {
            let t = data(&mut c, "quicksend.receive", json!({"code": a.pos[0]})).await?;
            return finish_transfer(&mut c, a, t, "Received").await;
        }
        "transfers" => {
            let list = data(&mut c, "transfers.list", json!({"active": a.on("active")})).await?;
            if json_out { show(&list); return Ok(0); }
            let rows = list.as_array().cloned().unwrap_or_default();
            if rows.is_empty() { println!("No transfers since DropBeam started."); }
            for t in rows {
                let names = t["fileNames"].as_array().map(|n| n.iter().filter_map(Value::as_str).take(2).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                println!("{}  {:<8} {:<16} {:>5.0}%  {:<10} {}", short(t["id"].as_str().unwrap_or("")), t["direction"].as_str().unwrap_or(""),
                    t["state"].as_str().unwrap_or(""), t["percent"].as_f64().unwrap_or(0.0), size(t["bytesTotal"].as_u64().unwrap_or(0)),
                    t["peer"].as_str().map(|p| format!("{p}: {names}")).unwrap_or(names));
            }
        }
        "events" => {
            let types: Vec<String> = a.flag("types").map(|t| t.split(',').map(|s| s.trim().to_owned()).collect()).unwrap_or_default();
            c.send_line(&json!({"id": 1, "op": "events.subscribe", "stream": true, "input": {"types": types}, "agent": c.agent})).await?;
            loop {
                let v = c.read_line().await?;
                if let Some(ev) = v.get("event") { println!("{ev}"); let _ = std::io::stdout().flush(); }
                else if v["ok"] == false { return Err(v["error"]["message"].as_str().unwrap_or("refused").to_owned()); }
                else { return Ok(0); }
            }
        }
        "op" => {
            let mut input: Value = match a.flag("input").or(a.flag("json-input")) {
                Some(j) => serde_json::from_str(j).map_err(|e| format!("--input isn't JSON: {e}"))?,
                None => json!({}),
            };
            if a.on("confirm") { input["confirm"] = json!(true); }
            if a.on("dry-run") { input["dryRun"] = json!(true); }
            let r = c.call(&a.pos[0], input).await?;
            if r["ok"] == true { show(&r["data"]); } else { eprintln!("{}", pretty(&r["error"])); return Ok(1); }
        }
        "exec" => return exec(c, a).await,
        _ => unreachable!(),
    }
    Ok(0)
}

fn print_message(m: &Value, who: &str) {
    let from = if m["fromMe"] == true { "You".to_owned() } else { who.to_owned() };
    let via = m["fromAgent"]["name"].as_str().map(|n| format!(" (via {n})")).unwrap_or_default();
    let body = if m["deleted"] == true { "(unsent)".to_owned() } else if m["kind"] == "file" {
        format!("[file] {}", m["files"].as_array().map(|f| f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default())
    } else { m["text"].as_str().unwrap_or("").to_owned() };
    println!("[{}] {from}{via}: {body}", when(m["ts"].as_u64().unwrap_or(0)));
}

/// Wait for a transfer to end (unless --no-wait) and report it.
async fn finish_transfer(c: &mut Client, a: &Args, t: Value, verb: &str) -> Result<i32, String> {
    if t["dryRun"] == true || a.on("no-wait") {
        if a.on("json") || t["dryRun"] == true { println!("{}", pretty(&t)); } else { println!("Started transfer {}.", short(t["id"].as_str().unwrap_or(""))); }
        return Ok(0);
    }
    let id = t["id"].as_str().unwrap_or("").to_owned();
    if !a.on("json") { eprintln!("Transferring… (Ctrl-C stops waiting; the transfer continues)"); }
    let done = data(c, "transfers.wait", json!({"id": id, "timeoutSecs": 86_400})).await?;
    if a.on("json") { println!("{}", pretty(&done)); }
    let state = done["state"].as_str().unwrap_or("");
    let ok = matches!(state, "completed" | "held");
    if !a.on("json") {
        match state {
            "completed" => println!("{verb} {} file(s), {}.", done["fileCount"], size(done["bytesTotal"].as_u64().unwrap_or(0))),
            "held" => println!("They're offline: a Transfer Server holds it until they're back."),
            _ => println!("Didn't finish ({}): {}", if state.is_empty() { "timed out" } else { state }, done["error"].as_str().unwrap_or("")),
        }
    }
    Ok(if ok { 0 } else { 1 })
}

async fn exec(mut c: Client, a: &Args) -> Result<i32, String> {
    let command = a.rest.iter().map(|w| if w.contains(char::is_whitespace) && a.rest.len() > 1 { format!("'{}'", w.replace('\'', "'\\''")) } else { w.clone() }).collect::<Vec<_>>().join(" ");
    let mut input = json!({"device": a.pos[0], "command": command});
    for (k, f) in [("shell", "shell"), ("cwd", "cwd")] { if let Some(v) = a.flag(f) { input[k] = json!(v); } }
    if let Some(t) = a.flag("timeout") { input["timeoutSecs"] = json!(t.parse::<u64>().map_err(|_| "--timeout: seconds")?); }
    if a.on("dry-run") { input["dryRun"] = json!(true); }
    input["confirm"] = json!(true); // typing the command is the confirmation
    c.send_line(&json!({"id": 1, "op": "exec.run", "stream": true, "input": input, "agent": c.agent})).await?;
    // Our stdin goes to the command when it's piped; a terminal sends end-of-input.
    let (tx, mut rx) = mpsc::channel::<Value>(16);
    if std::io::stdin().is_terminal() {
        let _ = tx.send(json!({"id": 1, "eof": true})).await;
    } else {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 32 << 10];
            let mut stdin = std::io::stdin();
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => { let _ = tx.blocking_send(json!({"id": 1, "eof": true})); break; }
                    Ok(n) => if tx.blocking_send(json!({"id": 1, "stdin": STANDARD.encode(&buf[..n])})).is_err() { break; },
                }
            }
        });
    }
    let mut canceled = false;
    loop {
        tokio::select! {
            line = c.read_line() => {
                let v = line?;
                if let Some(ev) = v.get("event") {
                    let bytes = STANDARD.decode(ev["b64"].as_str().unwrap_or("")).unwrap_or_default();
                    if ev["type"] == "stderr" { let _ = std::io::stderr().write_all(&bytes); } else { let _ = std::io::stdout().write_all(&bytes); let _ = std::io::stdout().flush(); }
                    continue;
                }
                if v["ok"] != true { return Err(v["error"]["message"].as_str().unwrap_or("refused").to_owned()); }
                let d = &v["data"];
                if a.on("json") || d["dryRun"] == true { println!("{}", pretty(d)); }
                if d["timedOut"] == true { eprintln!("dropbeam: the command ran out of time and was stopped"); return Ok(124); }
                if d["canceled"] == true { return Ok(130); }
                return Ok(d["exitCode"].as_i64().map(|c| c as i32).unwrap_or(if d["dryRun"] == true { 0 } else { 1 }));
            }
            Some(ctl) = rx.recv() => c.send_line(&ctl).await?,
            _ = tokio::signal::ctrl_c(), if !canceled => { canceled = true; c.send_line(&json!({"id": 1, "cancel": true})).await?; }
        }
    }
}

// ── MCP (Model Context Protocol) over stdio ─────────────────────────────────

/// MCP tool name for an op ("chat.send" → "chat_send").
pub fn tool_name(op: &str) -> String { op.replace('.', "_") }

/// The tool list, generated from the catalog.
pub fn mcp_tools() -> Vec<Value> {
    CATALOG.iter().map(|o| {
        let mut schema = o.input.clone();
        schema["properties"]["reason"] = json!({"type": "string", "description": "Why you're doing this, in a few words. Shown to the person in DropBeam's activity log next to your name."});
        let risk = match o.risk { Risk::Read => "read-only", Risk::Write => "changes something", Risk::Destructive => "destructive" };
        let mut d = format!("{}\nRisk: {risk}.", o.description);
        if o.confirm { d.push_str(" Needs confirm: true — ask the person first."); }
        if o.dry_run { d.push_str(" Accepts dryRun: true."); }
        if !o.output.is_empty() { d.push_str(&format!("\nReturns: {}", o.output)); }
        json!({"name": tool_name(o.name), "description": d, "inputSchema": schema,
            "annotations": {"title": o.name, "readOnlyHint": o.risk == Risk::Read, "destructiveHint": o.risk == Risk::Destructive, "openWorldHint": true}})
    }).collect()
}

async fn mcp_call(name: &str, mut args: Value, agent: Value, may_start: bool) -> Value {
    let Some(spec) = CATALOG.iter().find(|o| tool_name(o.name) == name) else {
        return json!({"content": [{"type": "text", "text": format!("No tool {name:?}.")}], "isError": true});
    };
    let reason = args.as_object_mut().and_then(|o| o.remove("reason"));
    let mut agent = agent;
    if let Some(r) = reason { agent["reason"] = r; }
    if args.is_null() { args = json!({}); }
    let reply = match connect(&config_dir(), may_start, true).await {
        Err(e) => Err(e),
        Ok(conn) => { let mut c = Client::new(conn, agent); c.call(spec.name, args).await }
    };
    match reply {
        Ok(r) if r["ok"] == true => json!({"content": [{"type": "text", "text": pretty(&r["data"])}], "isError": false}),
        Ok(r) => json!({"content": [{"type": "text", "text": format!("{}: {}{}", r["error"]["code"].as_str().unwrap_or("error"),
            r["error"]["message"].as_str().unwrap_or(""), r["error"].get("preview").map(|p| format!("\nPreview: {p}")).unwrap_or_default())}], "isError": true}),
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

/// Answer one JSON-RPC message (None for notifications). `agent` is filled in
/// from the client's `initialize`.
pub async fn mcp_handle(msg: &Value, agent: &mut Value, may_start: bool) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            if let Some(n) = msg["params"]["clientInfo"]["name"].as_str() { agent["name"] = json!(n); }
            let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            Ok(json!({"protocolVersion": version, "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "dropbeam", "title": "DropBeam", "version": env!("CARGO_PKG_VERSION")}, "instructions": MCP_INSTRUCTIONS}))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": mcp_tools()})),
        "tools/call" => Ok(mcp_call(msg["params"]["name"].as_str().unwrap_or(""), msg["params"]["arguments"].clone(), agent.clone(), may_start).await),
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ if id.is_none() => return None, // notifications (initialized, cancelled…)
        m => Err(json!({"code": -32601, "message": format!("method not found: {m}")})),
    };
    let id = id?;
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

async fn mcp(may_start: bool) -> i32 {
    let (tx, mut rx) = mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(v) = rx.recv().await {
            let mut l = v.to_string();
            l.push('\n');
            if out.write_all(l.as_bytes()).await.is_err() { break; }
            let _ = out.flush().await;
        }
    });
    let mut agent = json!({"name": "an MCP client"});
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() { continue; }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            let _ = tx.send(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})).await;
            continue;
        };
        // Tool calls can wait a long time (chat_wait): run them side by side.
        if msg["method"] == "tools/call" {
            let (tx, agent) = (tx.clone(), agent.clone());
            tokio::spawn(async move {
                let mut agent = agent;
                if let Some(r) = mcp_handle(&msg, &mut agent, may_start).await { let _ = tx.send(r).await; }
            });
        } else if let Some(r) = mcp_handle(&msg, &mut agent, may_start).await {
            let _ = tx.send(r).await;
        }
    }
    drop(tx);
    let _ = writer.await;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn parses_flags_positionals_and_rest() {
        let a = parse(&v(&["send", "Alex", "a.txt", "--json", "--device", "iPhone", "b.txt"])).unwrap();
        assert_eq!((a.cmd.as_str(), a.pos.clone()), ("send", v(&["Alex", "a.txt", "b.txt"])));
        assert!(a.on("json"));
        assert_eq!(a.flag("device"), Some("iPhone"));
        let a = parse(&v(&["exec", "mac", "--timeout=5", "--", "ls", "-la", "--json"])).unwrap();
        assert_eq!(a.pos, v(&["mac"]));
        assert_eq!(a.flag("timeout"), Some("5"));
        assert_eq!(a.rest, v(&["ls", "-la", "--json"]));
        assert!(!a.on("json"), "flags after -- belong to the command");
        assert!(parse(&v(&["wait", "--from"])).is_err());
        assert!(parse(&v(&["-h"])).unwrap().on("help"));
    }

    #[test]
    fn targets_and_times() {
        assert_eq!(split_target("Alex:Photos/2024/a.jpg"), Some(("Alex".into(), "Photos".into(), "2024/a.jpg".into())));
        assert_eq!(split_target("Alex:Photos"), Some(("Alex".into(), "Photos".into(), "".into())));
        assert_eq!(split_target("Photos/a"), None);
        assert_eq!(split_target(":x"), None);
        assert_eq!(parse_since("10m", 1_000_000), Some(400_000));
        assert_eq!(parse_since("123", 9), Some(123));
        assert_eq!(parse_since("5y", 9), None);
    }

    /// Every tool has a valid name and a JSON-schema object that accepts a
    /// `reason`; names are unique and map back to their op.
    #[test]
    fn mcp_tool_list_is_valid() {
        let tools = mcp_tools();
        assert_eq!(tools.len(), CATALOG.len());
        let mut names = std::collections::HashSet::new();
        for t in &tools {
            let n = t["name"].as_str().unwrap();
            assert!(n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{n}");
            assert!(names.insert(n.to_owned()), "duplicate {n}");
            let s = &t["inputSchema"];
            assert_eq!(s["type"], "object");
            assert!(s["properties"].is_object() && s["properties"]["reason"]["type"] == "string");
            assert!(s["required"].is_array());
            for (_, p) in s["properties"].as_object().unwrap() { assert!(p["description"].is_string(), "{n}"); }
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
        assert!(tools.iter().any(|t| t["name"] == "chat_wait"));
        assert!(tools.iter().any(|t| t["name"] == "exec_run" && t["annotations"]["destructiveHint"] == true));
    }

    #[test]
    fn mcp_handshake_without_the_app() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut agent = json!({"name": "x"});
            let init = mcp_handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26", "clientInfo": {"name": "claude-code", "version": "1"}, "capabilities": {}}}), &mut agent, false).await.unwrap();
            assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
            assert!(init["result"]["instructions"].as_str().unwrap().contains("chat_wait"));
            assert_eq!(agent["name"], "claude-code");
            assert!(mcp_handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &mut agent, false).await.is_none());
            let list = mcp_handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}), &mut agent, false).await.unwrap();
            assert!(list["result"]["tools"].as_array().unwrap().len() > 100);
            let bad = mcp_handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}), &mut agent, false).await.unwrap();
            assert_eq!(bad["error"]["code"], -32601);
            let unknown = mcp_handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "nope", "arguments": {}}}), &mut agent, false).await.unwrap();
            assert_eq!(unknown["result"]["isError"], true);
        });
    }
}
