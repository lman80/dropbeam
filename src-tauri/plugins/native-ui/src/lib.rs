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

#[tauri::command]
async fn pick_folder<R: Runtime>(app: AppHandle<R>) -> Result<Value, String> {
    app.state::<NativeUI<R>>().0.run_mobile_plugin("pickFolder", json!({})).map_err(|e| e.to_string())
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
            Ok(())
        })
        .build()
}
