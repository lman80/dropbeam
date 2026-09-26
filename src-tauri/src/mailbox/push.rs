//! iOS push through the owner's relay worker (phase 2). Phase 1 ships the
//! hooks only: no key is advertised and servers answer registrations with
//! "push_off", so everything degrades to deliver-on-open.

use std::path::Path;

use serde_json::{json, Value};

use super::server::{Item, ServerConfig};

pub fn configured() -> bool {
    false
}

pub fn register(_config: &Path, _c: &ServerConfig, _who: &str, _req: &Value) -> Value {
    json!({"ok": false, "reason": "push_off"})
}

pub fn on_stored(_config: &Path, _item: &Item) {}

pub fn push_key_advert(_config: &Path, _signer: &iroh::SecretKey) -> Option<(String, String)> {
    None
}

/// Sealed per-device notification previews for a chat frame (phase 2).
pub fn previews(_config: &Path, _to: &[super::seal::Recipient], _frame: &Value) -> Value {
    json!({})
}

/// Sealed per-device previews for a file send (phase 2).
pub fn file_previews(_config: &Path, _to: &[super::seal::Recipient], _names: &[String]) -> Value {
    json!({})
}
