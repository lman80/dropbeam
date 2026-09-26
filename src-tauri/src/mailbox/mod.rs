//! Transfer Server: store-and-forward of chat + friend file sends through a
//! DropBeam device the user chooses (docs/TRANSFER-SERVER-PLAN.md).
//!
//! - `seal`   — end-to-end envelope (the server can't read what it carries)
//! - `keys`   — this device's mailbox key + what peers advertised in hellos
//! - `server` — hosting: storage, access, quotas, expiry, delivery pokes
//! - `client` — choosing a server, depositing, fetching, receipts
//! - `push`   — iOS notification relay registration (phase 2)
//!
//! Wire: `mailbox.*` stream kinds on the shared `dropbeam/1` ALPN. Every
//! server-side handler authenticates by `conn.remote_id()` only.

pub mod client;
pub mod cmds;
pub mod keys;
pub mod push;
pub mod seal;
pub mod server;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde_json::{json, Value};

use crate::iroh_net::IrohState;

pub const VERSION: u64 = 1;

/// Route one `mailbox.*` stream. Server-side kinds go to `server`; the
/// server→recipient poke goes to `client`.
pub async fn serve(state: &IrohState, conn: &Connection, send: &mut SendStream, recv: &mut RecvStream, kind: &str, req: &Value) -> Result<()> {
    let Ok(config) = crate::iroh_net::location_config(state) else {
        crate::iroh_net::write_frame(send, &json!({"ok": false, "reason": "starting"})).await?;
        let _ = send.finish();
        return Ok(());
    };
    match kind {
        "mailbox.notify" => {
            let from = conn.remote_id().to_string();
            let accepted = client::on_notify(state, &config, &from);
            crate::iroh_net::write_frame(send, &json!({"ok": accepted})).await?;
            let _ = send.finish();
            Ok(())
        }
        _ => {
            let me = state.get().map(|e| e.id().to_string());
            let result = server::serve(&config, me.as_deref(), conn, send, recv, kind, req).await;
            // The owner's management page shows items/usage live.
            if matches!(kind, "mailbox.deposit" | "mailbox.ack" | "mailbox.cancel") {
                if let Some(app) = state.app.get() {
                    use tauri::Emitter;
                    let _ = app.emit("mailbox://server", ());
                }
            }
            result
        }
    }
}

// ── presence: a cheap "is this device reachable right now?" hint ─────────────

static SEEN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// An authenticated connection from/to `eid` just showed life.
pub fn note_seen(eid: &str) {
    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    m.insert(eid.to_owned(), Instant::now());
    if m.len() > 4096 {
        m.retain(|_, t| t.elapsed() < Duration::from_secs(3600));
    }
    server::device_seen(eid);
}

/// Seen within `window`.
pub fn seen_within(eid: &str, window: Duration) -> bool {
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        .and_then(|m| m.get(eid)).is_some_and(|t| t.elapsed() < window)
}

/// The fields every friend-hello (request AND reply) carries: our signed
/// mailbox key, the servers that hold messages for us, and — when this device
/// is a Transfer Server — what `who` may do with it.
pub fn hello_fields(config: &Path, signer: &iroh::SecretKey, who: &str) -> Value {
    let mut v = json!({
        "v": VERSION,
        "inbox": client::my_inbox(config),
        "sends": client::my_sends(config),
        "grant": server::grant_for(config, who),
    });
    if let Some(pk) = keys::public(config) {
        v["key"] = json!(seal::b64(&pk));
        v["sig"] = json!(seal::sign_mailbox_key(signer, &pk));
    }
    if let Some((pk, sig)) = push::push_key_advert(config, signer) {
        v["push_key"] = json!(pk);
        v["push_sig"] = json!(sig);
    }
    v
}

/// Apply the mailbox part of an authenticated hello (request or reply) from
/// `who`. Old peers send nothing, which changes nothing.
pub fn on_hello(state: &IrohState, config: &Path, who: &str, hello: &Value) {
    let Some(m) = hello.get("mailbox").filter(|m| m.is_object()) else { return };
    // Only people we already trust may tell us about keys and servers: a
    // stranger's hello must never become a route for our messages.
    let own = crate::account::is_own_device(config, who);
    if !own && crate::friends::chat_sender(config, who).is_none() {
        return;
    }
    keys::learn(config, who, m);
    let change = client::learn_grant(config, who, m.get("grant"), own);
    if let Some(app) = state.app.get() {
        use tauri::Emitter;
        if change.any() {
            let _ = app.emit("mailbox://servers", ());
        }
        if change.new_own {
            // Tell friends where to leave things for us from now on.
            if let Some(net) = tauri::Manager::try_state::<std::sync::Arc<IrohState>>(app) {
                crate::iroh_net::broadcast_profile(app.clone(), net.inner().clone());
            }
        }
    }
    if change.any() {
        client::wake();
    }
}
