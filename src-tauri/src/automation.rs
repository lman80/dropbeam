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
    // One write per line: listeners on other threads append concurrently.
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(results_path(dir)) {
        let _ = f.write_all(format!("{v}\n").as_bytes());
    }
}

/// Transfers this consumer started: id → (op, started).
type Started = Arc<Mutex<HashMap<String, (String, Instant)>>>;

pub fn spawn(app: AppHandle, config_dir: PathBuf) {
    let started: Started = Default::default();
    {
        // Terminal states of our transfers → results file.
        let (started, dir) = (started.clone(), config_dir.clone());
        let app_for_seen = app.clone();
        app.listen_any("transfer://update", move |e| {
            let Ok(u) = serde_json::from_str::<Value>(e.payload()) else { return };
            let state = u["state"].as_str().unwrap_or("");
            if !matches!(state, "completed" | "failed" | "canceled") { return; }
            let id = u["id"].as_str().unwrap_or("").to_owned();
            let Some((op, at)) = started.lock().unwrap().remove(&id) else {
                // Not ours (e.g. the RECEIVING side of a test send): still log the
                // terminal state while Lab Mode is on, so a driver can check both
                // ends ("Canceled by X" etc.).
                let lab = app_for_seen.try_state::<Arc<AppState>>().is_some_and(|st| st.settings.lock().unwrap().lab_mode_enabled);
                if lab {
                    record(&dir, json!({"event": "seen", "id": id, "state": state, "direction": u["direction"],
                        "peer": u["peer"], "friend": u["friendName"], "bytes": u["bytesTotal"], "done": u["bytesDone"],
                        "files": u["fileCount"], "locality": u["locality"], "error": u["error"], "detail": u["detail"]}));
                }
                return;
            };
            record(&dir, json!({"event": "done", "op": op, "id": id, "state": state,
                "bytes": u["bytesTotal"], "files": u["fileCount"], "ms": at.elapsed().as_millis() as u64,
                "locality": u["locality"], "error": u["error"], "detail": u["detail"]}));
        });
    }
    {
        // Typing indicators are ephemeral: log them (Lab Mode only) for drivers.
        let (dir, app2) = (config_dir.clone(), app.clone());
        app.listen_any("chat://typing", move |e| {
            if app2.try_state::<Arc<AppState>>().is_some_and(|st| st.settings.lock().unwrap().lab_mode_enabled) {
                record(&dir, json!({"event": "typing", "payload": serde_json::from_str::<Value>(e.payload()).unwrap_or_default()}));
            }
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
                        if !matches!(op.as_str(), "cancel" | "accept-request" | "block") && !op.starts_with("chat-") { started.lock().unwrap().insert(id.clone(), (op.clone(), Instant::now())); }
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
        // Chat actions on a message (friend by endpoint id or name):
        //   {"op":"chat-edit","to":…,"id":msgId,"text":…} / chat-unsend / chat-react
        //   {"op":"chat-reply","to":…,"id":msgId,"text":…} / chat-read {"upTo":ms}
        //   chat-typing {"on":bool} / chat-log → records the thread in the results.
        op @ ("chat-edit" | "chat-unsend" | "chat-react" | "chat-reply" | "chat-read" | "chat-typing" | "chat-log") => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            let mid = cmd["id"].as_str().unwrap_or("").to_owned();
            let text = cmd["text"].as_str().unwrap_or("").to_owned();
            match op {
                "chat-edit" => crate::commands::edit_chat_message(app.state(), app.clone(), f.id.clone(), mid.clone(), text).await?,
                "chat-unsend" => crate::commands::delete_chat_message(app.state(), app.clone(), f.id.clone(), mid.clone()).await?,
                "chat-react" => crate::commands::react_to_message(app.state(), app.clone(), f.id.clone(), mid.clone(),
                    cmd["emoji"].as_str().unwrap_or("👍").to_owned(), cmd["add"].as_bool().unwrap_or(true)).await?,
                "chat-reply" => {
                    let m = crate::commands::send_chat_message(app.state(), app.state(), app.clone(), f.id.clone(), text, Some(mid), Some("reply".into())).await?;
                    return Ok((op.into(), m.id, None));
                }
                "chat-read" => crate::commands::send_read_receipt(app.state(), app.state(), f.id.clone(), cmd["upTo"].as_u64().unwrap_or_else(crate::chat::now_ms)).await?,
                "chat-typing" => crate::commands::send_typing(app.state(), app.state(), f.id.clone(), cmd["on"].as_bool().unwrap_or(true)).await?,
                _ => {
                    let owner = friends::thread_owner(&st.config_dir, &f.id).map(|o| o.id).unwrap_or(f.id.clone());
                    let n = cmd["last"].as_u64().unwrap_or(10) as usize;
                    let msgs = crate::chat::messages(&st.config_dir, &owner);
                    let tail: Vec<Value> = msgs.iter().rev().take(n).rev().map(|m| json!({"id": m.id, "text": m.text,
                        "fromMe": m.from_me, "status": m.status, "edited": m.edited, "deleted": m.deleted,
                        "reactions": m.reactions, "replyTo": m.reply_to, "kind": m.kind, "heldOn": m.held_on, "ts": m.ts})).collect();
                    record(&st.config_dir, json!({"event": "chatlog", "to": to, "messages": tail}));
                }
            }
            Ok((op.into(), mid, None))
        }
        // Friend requests + blocking: {"op":"accept-request","eid":…},
        // {"op":"block","to":<friend eid or name>}.
        "accept-request" => {
            let eid = cmd["eid"].as_str().unwrap_or("").to_owned();
            let f = crate::commands::accept_friend_request(app.clone(), app.state(), app.state(), app.state(), eid)?;
            Ok(("accept-request".into(), f.id, None))
        }
        "block" => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            let ids = crate::commands::block_friend(app.clone(), app.state(), app.state(), f.id.clone())?;
            Ok(("block".into(), ids.join(","), None))
        }
        // Cleanup for request/block tests: {"op":"unblock","id":<eid>},
        // {"op":"remove-friend","to":<friend eid or name>}.
        "unblock" => {
            let id = cmd["id"].as_str().unwrap_or("").to_owned();
            crate::commands::unblock_person(app.clone(), app.state(), id.clone())?;
            Ok(("unblock".into(), id, None))
        }
        "remove-friend" => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            crate::commands::remove_friend(app.clone(), app.state(), app.state(), f.id.clone())?;
            Ok(("remove-friend".into(), f.id, None))
        }
        "addfriend" => {
            let code = cmd["code"].as_str().unwrap_or("");
            let f = friends::add_by_code(&st.config_dir, code)?;
            let name = st.settings.lock().unwrap().display_name.clone();
            if let Some(eid) = f.endpoint_id.clone() { crate::iroh_net::say_hello_to_endpoint(net.clone(), eid, name); }
            Ok(("addfriend".into(), f.id, None))
        }
        // "send" = exactly what the UI and the menu-bar drag do (transfer + its
        // chat note); "send-nonote" = a transfer whose note never arrives.
        op @ ("send" | "send-nonote") => {
            let to = cmd["to"].as_str().unwrap_or("");
            let f = friends::load(&st.config_dir).into_iter()
                .find(|f| f.endpoint_id.as_deref() == Some(to) || f.name == to)
                .ok_or_else(|| format!("no friend {to:?}"))?;
            let eid = f.endpoint_id.clone().ok_or("friend has no endpoint")?;
            let paths = paths_of(cmd)?;
            let u = crate::iroh_net::send_to_friend(app.clone(), net.clone(), f.name.clone(), eid, paths.clone(), None, None)?;
            if op == "send" {
                let names = paths.iter().map(|p| Path::new(p).file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())).collect();
                crate::commands::post_file_note(st, net, app, &f.id, names, u.bytes_total, paths, None, Some(u.id.clone()));
            }
            Ok((op.into(), u.id, None))
        }
        // Cancel one transfer by id, or every running one with "*".
        "cancel" => {
            let id = cmd["id"].as_str().unwrap_or("");
            let ids = if id == "*" { net.active_transfer_ids() } else { vec![id.to_owned()] };
            for id in &ids {
                if let crate::iroh_net::CancelKind::Staged = net.cancel(id) {
                    crate::iroh_net::emit_canceled_send(app, id);
                }
            }
            Ok(("cancel".into(), ids.join(","), None))
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
