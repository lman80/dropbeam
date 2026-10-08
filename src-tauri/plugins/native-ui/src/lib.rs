#![cfg(target_os = "ios")]

use serde_json::{json, Value};
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    AppHandle, Manager, Runtime,
};

tauri::ios_plugin_binding!(init_plugin_native_ui);

struct NativeUI<R: Runtime>(PluginHandle<R>);

// Mobile calls block waiting for Swift's resolve; async commands keep the main
// thread free to actually execute Swift and evaluate the WKWebView callbacks.
#[tauri::command]
async fn activate<R: Runtime>(app: AppHandle<R>) -> Result<Value, String> {
    app.state::<NativeUI<R>>()
        .0
        .run_mobile_plugin("activate", json!({}))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reply<R: Runtime>(
    app: AppHandle<R>,
    id: u64,
    ok: bool,
    value: Value,
) -> Result<Value, String> {
    app.state::<NativeUI<R>>()
        .0
        .run_mobile_plugin("reply", json!({"id": id, "ok": ok, "value": value}))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn state<R: Runtime>(
    app: AppHandle<R>,
    key: String,
    value: Value,
) -> Result<Value, String> {
    app.state::<NativeUI<R>>()
        .0
        .run_mobile_plugin("state", json!({"key": key, "value": value}))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn event<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    payload: Value,
) -> Result<Value, String> {
    app.state::<NativeUI<R>>()
        .0
        .run_mobile_plugin("event", json!({"name": name, "payload": payload}))
        .map_err(|e| e.to_string())
}

/// `purpose`: "upload" = a folder to send (private copy, swept once sent); anything
/// else = a Shared Folder's local copy (Documents/Shared Folders, visible in Files).
#[tauri::command]
async fn pick_folder<R: Runtime>(app: AppHandle<R>, purpose: Option<String>) -> Result<Value, String> {
    app.state::<NativeUI<R>>().0.run_mobile_plugin("pickFolder", json!({"purpose": purpose})).map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_files<R: Runtime>(app: AppHandle<R>) -> Result<Value, String> {
    app.state::<NativeUI<R>>().0.run_mobile_plugin("pickFiles", json!({})).map_err(|e| e.to_string())
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("native-ui")
        .invoke_handler(tauri::generate_handler![activate, reply, state, event, pick_folder, pick_files])
        .setup(|app, api| {
            app.manage(NativeUI(api.register_ios_plugin(init_plugin_native_ui)?));
            activity::start(app.clone());
            Ok(())
        })
        .build()
}

/// Live transfer activity for the native shell's background handling (keep the app
/// running while bytes move, report progress to iOS 26's continued-processing UI).
///
/// Forwarded straight from the engine's `transfer://update` events — NOT through the
/// hidden WebView, whose JavaScript iOS suspends soon after the app leaves the screen,
/// exactly when this is needed. At most ~1.5 updates a second reach Swift.
mod activity {
    use super::NativeUI;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Listener, Manager, Runtime};

    #[derive(Default)]
    struct Live {
        /// id → (sending?, bytes done, bytes total, last update)
        transfers: HashMap<String, (bool, u64, u64, Instant)>,
        dirty: bool,
    }

    /// Bytes are moving (or about to): a code nobody has used yet, or a send waiting
    /// for the other side's OK, doesn't need the app kept awake.
    fn is_moving(state: &str) -> bool {
        matches!(state, "starting" | "connecting" | "transferring")
    }

    pub fn start<R: Runtime>(app: AppHandle<R>) {
        let live = Arc::new(Mutex::new(Live::default()));
        let sink = live.clone();
        app.listen_any("transfer://update", move |event| {
            let Ok(update) = serde_json::from_str::<Value>(event.payload()) else { return };
            let Some(id) = update["id"].as_str() else { return };
            let mut g = sink.lock().unwrap_or_else(|p| p.into_inner());
            if is_moving(update["state"].as_str().unwrap_or("")) {
                let done = update["bytesDone"].as_u64().unwrap_or(0);
                let total = update["bytesTotal"].as_u64().unwrap_or(0);
                g.transfers.insert(id.to_owned(), (update["direction"].as_str() == Some("send"), done, total, Instant::now()));
            } else if g.transfers.remove(id).is_none() {
                return;
            }
            g.dirty = true;
        });
        std::thread::Builder::new()
            .name("native-ui-activity".into())
            .spawn(move || {
                let mut last_sent = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_millis(700));
                    let summary = {
                        let mut g = live.lock().unwrap_or_else(|p| p.into_inner());
                        // A card that stopped reporting (engine restarted it under a new
                        // id, crashed task) must not keep the phone awake forever.
                        let before = g.transfers.len();
                        g.transfers.retain(|_, t| t.3.elapsed() < Duration::from_secs(45));
                        if g.transfers.len() != before { g.dirty = true; }
                        // Re-send at least every 10 s while active (Swift times out stale data).
                        if !g.dirty && (g.transfers.is_empty() || last_sent.elapsed() < Duration::from_secs(10)) { continue; }
                        g.dirty = false;
                        let (mut done, mut total, mut sending) = (0u64, 0u64, 0usize);
                        for t in g.transfers.values() {
                            done = done.saturating_add(t.1.min(t.2.max(t.1)));
                            total = total.saturating_add(t.2.max(t.1));
                            if t.0 { sending += 1; }
                        }
                        json!({"active": g.transfers.len(), "sending": sending, "bytesDone": done, "bytesTotal": total})
                    };
                    last_sent = Instant::now();
                    if let Some(ui) = app.try_state::<NativeUI<R>>() {
                        let _ = ui.0.run_mobile_plugin::<Value>("transferActivity", summary);
                    }
                }
            })
            .ok();
    }
}
