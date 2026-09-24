//! Lab-mode automation: drive REAL app transfers from a script, on any OS,
//! without the GUI. Only active while `settings.lab_mode_enabled` is on.
//!
//! Drop a JSON array into `<config>/automation-queue.json`; the app consumes it
//! within ~2 s. Commands:
//!   {"op":"send","to":"<friend endpoint id or name>","paths":["/abs/file", …]}
//!   {"op":"quicksend","paths":[…]}          → the ticket is logged as "code"
//!   {"op":"receive","code":"direct…"}       → lands in the download folder
//!   {"op":"chat","to":"<friend>","text":"…"}  → a chat message
//!   {"op":"addfriend","code":"dropbeam:…"}     → add a friend by their code
//! Every command and every terminal state of a transfer it started is appended
//! to `<config>/automation-results.jsonl` (one JSON object per line) so a test
//! driver on another machine can collect outcomes over ssh.

use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Listener, Manager};

use crate::{friends, iroh_net::IrohState, AppState};

fn results_path(dir: &Path) -> PathBuf {
    dir.join("automation-results.jsonl")
}

fn record(dir: &Path, mut v: Value) {
    use std::io::Write;
    v["t"] = json!(crate::chat::now_ms());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(results_path(dir)) {
        let _ = writeln!(f, "{v}");
    }
}

/// Transfers this consumer started: id → (op, started).
type Started = Arc<Mutex<HashMap<String, (String, Instant)>>>;

pub fn spawn(app: AppHandle, config_dir: PathBuf) {
    let started: Started = Default::default();
    {
        // Terminal states of our transfers → results file.
        let (started, dir) = (started.clone(), config_dir.clone());
        app.listen_any("transfer://update", move |e| {
            let Ok(u) = serde_json::from_str::<Value>(e.payload()) else { return };
            let state = u["state"].as_str().unwrap_or("");
            if !matches!(state, "completed" | "failed" | "canceled") { return; }
            let id = u["id"].as_str().unwrap_or("").to_owned();
            let Some((op, at)) = started.lock().unwrap().remove(&id) else { return };
            record(&dir, json!({"event": "done", "op": op, "id": id, "state": state,
                "bytes": u["bytesTotal"], "files": u["fileCount"], "ms": at.elapsed().as_millis() as u64,
                "locality": u["locality"], "error": u["error"]}));
        });
    }
    tauri::async_runtime::spawn(async move {
        let queue = config_dir.join("automation-queue.json");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let (Some(st), Some(net)) = (app.try_state::<Arc<AppState>>(), app.try_state::<Arc<IrohState>>()) else { continue };
            if !st.settings.lock().unwrap().lab_mode_enabled { continue; }
            let Ok(bytes) = std::fs::read(&queue) else { continue };
            let _ = std::fs::remove_file(&queue);
            let Ok(cmds) = serde_json::from_slice::<Vec<Value>>(&bytes) else {
                record(&config_dir, json!({"event": "error", "error": "automation-queue.json is not a JSON array"}));
                continue;
            };
            for cmd in cmds {
                let (st, net) = (st.inner().clone(), net.inner().clone());
                let result = run(&app, &st, &net, &cmd).await;
                match result {
                    Ok((op, id, code)) => {
                        started.lock().unwrap().insert(id.clone(), (op.clone(), Instant::now()));
                        record(&config_dir, json!({"event": "started", "op": op, "id": id, "code": code, "cmd": cmd}));
                    }
                    Err(e) => record(&config_dir, json!({"event": "error", "cmd": cmd, "error": e})),
                }
            }
        }
    });
}

fn paths_of(cmd: &Value) -> Result<Vec<String>, String> {
    let paths: Vec<String> = cmd["paths"].as_array().into_iter().flatten()
        .filter_map(|p| p.as_str().map(str::to_owned)).collect();
    if paths.is_empty() { return Err("no paths".into()); }
    if let Some(missing) = paths.iter().find(|p| !Path::new(p).exists()) { return Err(format!("not found: {missing}")); }
    Ok(paths)
}

async fn run(app: &AppHandle, st: &Arc<AppState>, net: &Arc<IrohState>, cmd: &Value) -> Result<(String, String, Option<String>), String> {
    match cmd["op"].as_str().unwrap_or("") {
        "chat" => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            let text = cmd["text"].as_str().unwrap_or("").to_owned();
            let m = crate::commands::send_chat_message(app.state(), app.state(), app.clone(), f.id, text, None, None).await?;
            Ok(("chat".into(), m.id, None))
        }
        "addfriend" => {
            let code = cmd["code"].as_str().unwrap_or("");
            let f = friends::add_by_code(&st.config_dir, code)?;
            let name = st.settings.lock().unwrap().display_name.clone();
            if let Some(eid) = f.endpoint_id.clone() { crate::iroh_net::say_hello_to_endpoint(net.clone(), eid, name); }
            Ok(("addfriend".into(), f.id, None))
        }
        "send" => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            let eid = f.endpoint_id.clone().ok_or("friend has no endpoint")?;
            let u = crate::iroh_net::send_to_friend(app.clone(), net.clone(), f.name, eid, paths_of(cmd)?, None, None)?;
            Ok(("send".into(), u.id, None))
        }
        "quicksend" => {
            let u = crate::iroh_net::start_send(app.clone(), net.clone(), paths_of(cmd)?)?;
            Ok(("quicksend".into(), u.id, u.code))
        }
        "receive" => {
            let code = cmd["code"].as_str().unwrap_or("").trim().to_owned();
            if code.is_empty() { return Err("no code".into()); }
            let configured = st.settings.lock().unwrap().download_dir.clone();
            let out = if configured.trim().is_empty() {
                crate::commands::download_directory(app).map(|p| p.to_string_lossy().to_string()).map_err(|e| e.to_string())?
            } else { configured };
            let u = crate::iroh_net::start_receive(app.clone(), net.clone(), code, out)?;
            Ok(("receive".into(), u.id, None))
        }
        other => Err(format!("unknown op {other:?}")),
    }
}
