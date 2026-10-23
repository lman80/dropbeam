//! Tauri commands for the call screen (src/lib/calls.ts) and the iOS shell.

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, State};

use crate::iroh_net::IrohState;
use crate::AppState;

type Iroh<'a> = State<'a, Arc<IrohState>>;

fn e(err: anyhow::Error) -> String { format!("{err:#}") }

/// The call screen says whether this webview can do WebRTC. Until it does,
/// this device answers rings with "unavailable" instead of ringing silently.
#[tauri::command]
pub fn call_capable(iroh: Iroh<'_>, supported: bool) {
    super::calls_of(&iroh).set_capable(supported);
}

#[tauri::command]
pub async fn call_start(iroh: Iroh<'_>, friend_id: String, video: bool) -> Result<Value, String> {
    super::start(&iroh, &friend_id, video).await.map_err(e)
}

#[tauri::command]
pub async fn call_offer(iroh: Iroh<'_>, call_id: String, sdp: String) -> Result<(), String> {
    super::send_offer(iroh.inner(), &call_id, &sdp).await.map_err(e)
}

#[tauri::command]
pub fn call_accept(iroh: Iroh<'_>, call_id: String) -> Result<Value, String> {
    super::accept(&iroh, &call_id).map_err(e)
}

#[tauri::command]
pub fn call_answer(iroh: Iroh<'_>, call_id: String, sdp: String) -> Result<(), String> {
    super::answer(&iroh, &call_id, &sdp).map_err(e)
}

#[tauri::command]
pub fn call_decline(iroh: Iroh<'_>, call_id: String) { super::decline(&iroh, &call_id) }

#[tauri::command]
pub fn call_hangup(iroh: Iroh<'_>, call_id: String) { super::hangup(&iroh, &call_id) }

#[tauri::command]
pub fn call_media_up(iroh: Iroh<'_>, call_id: String) { super::media_up(&iroh, &call_id) }

#[tauri::command]
pub fn call_signal(iroh: Iroh<'_>, call_id: String, frame: Value) -> Result<(), String> {
    super::signal_out(&iroh, &call_id, &frame).map_err(e)
}

#[tauri::command]
pub fn call_status(iroh: Iroh<'_>) -> Value { super::calls_of(&iroh).status() }

#[tauri::command]
pub fn call_stats(iroh: Iroh<'_>, call_id: String) -> Result<Value, String> {
    super::stats(&iroh, &call_id).map_err(e)
}

fn downloads(app: &AppHandle, state: &AppState) -> Result<std::path::PathBuf, String> {
    let configured = state.settings.lock().unwrap().download_dir.clone();
    if !configured.trim().is_empty() { return Ok(configured.into()); }
    crate::commands::download_directory(app).map_err(|e| format!("No download folder available: {e}"))
}

/// Start recording: refused until the other person allowed it for this call.
#[tauri::command]
pub fn call_record_open(app: AppHandle, state: State<'_, Arc<AppState>>, iroh: Iroh<'_>, call_id: String) -> Result<Value, String> {
    let dir = downloads(&app, &state)?;
    super::record::open(&super::calls_of(&iroh), &call_id, &dir).map_err(e)
}

/// Recorded bytes (raw body) for one track; headers x-rec-id, x-track, x-ext.
#[tauri::command]
pub fn call_record_chunk(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let h = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return Err("Expected recorded bytes.".into()) };
    if bytes.len() > 32 << 20 { return Err("Chunk too large.".into()); }
    super::record::chunk(&h("x-rec-id"), &h("x-track"), &h("x-ext"), bytes).map_err(e)
}

#[tauri::command]
pub fn call_record_close(iroh: Iroh<'_>, rec_id: String) -> Result<Value, String> {
    super::record::close(Some(&super::calls_of(&iroh)), &rec_id).map_err(e)
}

/// Saved call recordings, newest first.
#[tauri::command]
pub fn call_recordings(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<Vec<Value>, String> {
    Ok(super::record::list(&downloads(&app, &state)?))
}

/// Call history lines (all threads, or one person's), newest first.
#[tauri::command]
pub fn call_history(state: State<'_, Arc<AppState>>, friend_id: Option<String>, limit: Option<usize>) -> Vec<Value> {
    super::history(&state.config_dir, friend_id.as_deref(), limit.unwrap_or(100).clamp(1, 1000))
}
