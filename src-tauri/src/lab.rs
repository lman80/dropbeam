//! Lab Mode — a gated remote test-and-update surface built into the shipping app.
//!
//! When the user turns Lab Mode ON in Settings AND names a trusted operator
//! device, that ONE device can drive automated end-to-end tests against the real
//! app (shared folders, chat, quick-send, friends) and push new builds — all over
//! the same encrypted iroh link the app already uses. This lets the author verify
//! every feature between real devices (including a real Windows machine, which a
//! Mac can't stand in for) and iterate device-to-device after a single release.
//!
//! SECURITY: every lab command is refused unless
//!   (1) `settings.lab_mode_enabled` is true, AND
//!   (2) the QUIC peer's iroh node id == `settings.lab_operator_id`.
//! The node id is cryptographically bound to the connection, so (2) is real
//! authentication — a random peer can never drive Lab Mode even if they somehow
//! learn the frame format. Off + no-operator (the defaults) accept nothing.

use anyhow::{bail, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};

use crate::iroh_net::{read_frame_cap, write_frame, IrohState};

/// THE security gate, as a pure function so it can be exhaustively tested. A
/// peer may drive Lab Mode iff the feature is enabled AND a non-empty operator
/// id is configured AND the connection's authenticated node id matches it
/// exactly (case-insensitively, since node ids are hex/z-base-32).
pub(crate) fn gate(enabled: bool, operator_id: &str, remote_id: &str) -> bool {
    let op = operator_id.trim();
    enabled && !op.is_empty() && op.eq_ignore_ascii_case(remote_id.trim())
}

/// Is this connection allowed to drive Lab Mode right now? Reads settings fresh
/// each time so toggling the setting takes effect without a restart.
fn authorized(conn: &Connection, state: &IrohState) -> bool {
    let Some(app) = state.app.get() else {
        return false;
    };
    use tauri::Manager;
    let Some(app_state) = app.try_state::<std::sync::Arc<crate::AppState>>() else {
        return false;
    };
    let s = crate::settings::load(&app_state.config_dir, "", "");
    gate(s.lab_mode_enabled, &s.lab_operator_id, &conn.remote_id().to_string())
}

/// Handle a `{kind:"lab"}` stream. The first frame already read is `req`. Any
/// unauthorized attempt gets a single refusal frame and nothing else runs.
pub(crate) async fn handle_lab(
    conn: &Connection,
    send: &mut SendStream,
    recv: &mut RecvStream,
    req: &serde_json::Value,
    state: &IrohState,
) -> Result<()> {
    if !authorized(conn, state) {
        // Deliberately terse — don't leak whether lab mode is on or who the
        // operator is to an unauthorized caller.
        write_frame(send, &serde_json::json!({ "ok": false, "error": "unauthorized" })).await?;
        send.finish()?;
        bail!("lab: unauthorized peer {}", conn.remote_id());
    }
    let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
    let reply = dispatch(cmd, req, recv, state).await;
    let frame = match reply {
        Ok(v) => {
            let mut v = v;
            v["ok"] = serde_json::Value::Bool(true);
            v
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    };
    write_frame(send, &frame).await?;
    send.finish()?;
    let _ = send.stopped().await;
    Ok(())
}

/// The device's config dir — where friends/chat/pairing/settings live.
fn config_dir(state: &IrohState) -> Result<std::path::PathBuf> {
    use tauri::Manager;
    let app = state.app.get().ok_or_else(|| anyhow::anyhow!("app not ready"))?;
    let app_state = app
        .try_state::<std::sync::Arc<crate::AppState>>()
        .ok_or_else(|| anyhow::anyhow!("app state not ready"))?;
    Ok(app_state.config_dir.clone())
}

fn str_field<'a>(req: &'a serde_json::Value, k: &str) -> Result<&'a str> {
    req.get(k)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing string field '{k}'"))
}

/// Run one authorized lab command. Returns the JSON payload to reply with (the
/// caller stamps `ok`). New commands slot in here.
async fn dispatch(
    cmd: &str,
    req: &serde_json::Value,
    _recv: &mut RecvStream,
    state: &IrohState,
) -> Result<serde_json::Value> {
    match cmd {
        // Liveness + identity: what build is this device running, on what OS, and
        // what is its own node id (so the operator can confirm reachability).
        "ping" => Ok(serde_json::json!({
            "build": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "nodeId": state.get().map(|ep| ep.id().to_string()).unwrap_or_default(),
        })),

        // Add (or refresh) a friend by their node id — the same call the app's
        // auto-friend path uses. Returns the friend id. Idempotent.
        "friend-add" => {
            let cfg = config_dir(state)?;
            let node = str_field(req, "nodeId")?;
            let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("Lab Peer");
            let f = crate::friends::upsert_by_endpoint(&cfg, node, name);
            Ok(serde_json::json!({ "friendId": f.id }))
        }

        // Send a direct message to a friend (by their node id) through the REAL
        // chat path: record it, then deliver over iroh. Returns msg id + whether
        // delivery succeeded this attempt.
        "chat-send" => {
            let cfg = config_dir(state)?;
            let node = str_field(req, "nodeId")?;
            let text: String = str_field(req, "text")?.chars().take(4000).collect();
            let friend = crate::friends::upsert_by_endpoint(&cfg, node, "Lab Peer");
            let msg = crate::chat::ChatMessage {
                id: uuid::Uuid::new_v4().to_string(),
                peer_id: friend.id.clone(),
                from_me: true,
                kind: "text".into(),
                text,
                files: vec![],
                bytes: 0,
                path: None,
                status: Some("sending".into()),
                ts: crate::chat::now_ms(),
                seq: crate::chat::next_seq(&cfg, &friend.id),
                reply_to: None,
                reply_preview: None,
                reactions: vec![],
                edited: false,
                deleted: false,
                gif: None,
            };
            crate::chat::append(&cfg, &msg);
            let ep = state.get().cloned().ok_or_else(|| anyhow::anyhow!("iroh not ready"))?;
            let my_name = my_display_name(state);
            let payload = crate::iroh_net::chat_payload(&msg, &friend.id, &my_name);
            let delivered = crate::iroh_net::send_chat(&ep, node, payload).await.is_ok();
            if delivered {
                crate::chat::set_status(&cfg, &friend.id, &msg.id, "delivered");
            }
            Ok(serde_json::json!({ "msgId": msg.id, "delivered": delivered }))
        }

        // Return the message log with a peer (by node id) — text + direction +
        // seq — so the operator can confirm what actually landed on this device.
        "chat-log" => {
            let cfg = config_dir(state)?;
            let node = str_field(req, "nodeId")?;
            let friend = crate::friends::upsert_by_endpoint(&cfg, node, "Lab Peer");
            let msgs: Vec<serde_json::Value> = crate::chat::messages(&cfg, &friend.id)
                .into_iter()
                .map(|m| serde_json::json!({
                    "text": m.text, "fromMe": m.from_me, "seq": m.seq,
                    "kind": m.kind, "deleted": m.deleted, "edited": m.edited,
                }))
                .collect();
            Ok(serde_json::json!({ "friendId": friend.id, "count": msgs.len(), "messages": msgs }))
        }

        other => bail!("unknown lab command: {other}"),
    }
}

/// This device's display name (for the chat payload's `fromName`).
fn my_display_name(state: &IrohState) -> String {
    config_dir(state)
        .map(|cfg| crate::settings::load(&cfg, "", "").display_name)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::gate;

    const OP: &str = "825e889fce001f39630c1ddf37a24de39c658ef9f0c466a33d650b1360030b70";

    #[test]
    fn gate_denies_by_default() {
        // Disabled (the default) → never authorized, even with a matching id.
        assert!(!gate(false, OP, OP));
        // Enabled but no operator configured → nothing is accepted.
        assert!(!gate(true, "", OP));
        assert!(!gate(true, "   ", OP));
    }

    #[test]
    fn gate_allows_only_the_exact_operator() {
        assert!(gate(true, OP, OP));
        // Whitespace around the stored id is tolerated (paste artifacts).
        assert!(gate(true, &format!("  {OP}  "), OP));
        // Case-insensitive (hex/z-base-32 ids).
        assert!(gate(true, &OP.to_uppercase(), OP));
    }

    #[test]
    fn gate_denies_any_other_peer() {
        let other = "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(!gate(true, OP, other));
        // A prefix of the operator id must NOT pass (no partial match).
        assert!(!gate(true, OP, &OP[..40]));
        // Empty remote id never matches a real operator.
        assert!(!gate(true, OP, ""));
    }
}
