//! Tauri commands invoked from the React frontend.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::models::{
    DeleteMode, FolderStatus, Friend, HistoryEntry, HistoryItem, Pair, Settings, TransferUpdate,
};
use crate::sync::SyncManager;
use crate::{croc, folder_history, friends, history, pairing, settings, AppState};

#[tauri::command]
pub fn send_files(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    paths: Vec<String>,
) -> Result<TransferUpdate, String> {
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.trim().is_empty()).collect();
    if paths.is_empty() {
        return Err("No files selected.".into());
    }
    for p in &paths {
        if !std::path::Path::new(p).exists() {
            return Err(format!("File not found: {p}"));
        }
    }
    Ok(croc::start_send(app, state.inner().clone(), paths, None, None))
}

#[tauri::command]
pub fn receive_files(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    code: String,
) -> Result<TransferUpdate, String> {
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err("Enter a code to receive.".into());
    }
    let configured = { state.settings.lock().unwrap().download_dir.clone() };
    let out = if configured.trim().is_empty() {
        app.path()
            .download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| format!("No download folder available: {e}"))?
    } else {
        configured
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("Can't write to download folder: {e}"))?;
    Ok(croc::start_receive(app, state.inner().clone(), code, out))
}

#[tauri::command]
pub fn cancel_transfer(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
) {
    use crate::iroh_net::CancelKind;
    match iroh.cancel(&id) {
        // A staged send isn't running a loop, so report Canceled here.
        CancelKind::Staged => crate::iroh_net::emit_canceled_send(&app, &id),
        // An in-flight iroh transfer reports Canceled from its own loop.
        CancelKind::Active => {}
        // Not an iroh transfer — cancel the croc one.
        CancelKind::Unknown => {
            if let Some(notify) = state.transfers.lock().unwrap().get(&id) {
                notify.notify_one();
            }
        }
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    settings: Settings,
) -> Result<Settings, String> {
    apply_autostart(&app, settings.launch_at_login);
    {
        let mut guard = state.settings.lock().unwrap();
        *guard = settings.clone();
    }
    settings::save(&state.config_dir, &settings)?;
    // Turning Direct mode on starts the P2P engine right away (no restart). The
    // `app` OnceCell is set the first time we spawn, so this won't double-start.
    if settings.direct_mode && iroh.app.get().is_none() {
        crate::iroh_net::spawn(state.config_dir.clone(), iroh.inner().clone(), app.clone());
    }
    Ok(settings)
}

#[tauri::command]
pub fn get_history(state: State<'_, Arc<AppState>>) -> Vec<HistoryEntry> {
    history::load(&state.config_dir)
}

#[tauri::command]
pub fn clear_history(state: State<'_, Arc<AppState>>) {
    history::clear(&state.config_dir);
}

/// Native multi-file picker (the "+" / choose-files affordance).
///
/// IMPORTANT: this is `async` on purpose. A sync command runs on the main UI
/// thread, and a *blocking* file dialog would then deadlock that thread (the
/// panel appears but the whole app freezes). Running async + the non-blocking
/// callback lets the dialog live on the main loop while we await off-thread.
#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_files(move |paths| {
        let _ = tx.send(paths);
    });
    match rx.await {
        Ok(Some(list)) => list
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .map(|pb| pb.to_string_lossy().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Native folder picker (download folder, shared folder selection).
#[tauri::command]
pub async fn pick_directory(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    match rx.await {
        Ok(Some(p)) => p.into_path().ok().map(|pb| pb.to_string_lossy().to_string()),
        _ => None,
    }
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Bring the main window forward (from the menu-bar popover or the HUD) and
/// tuck the popover away.
#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(pop) = app.get_webview_window("popover") {
        let _ = pop.hide();
    }
}

/// Hide the menu-bar popover (its own close affordance).
#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    if let Some(pop) = app.get_webview_window("popover") {
        let _ = pop.hide();
    }
}

#[tauri::command]
pub fn get_default_download_dir(app: AppHandle) -> String {
    app.path()
        .download_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn apply_autostart(app: &AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let _ = if enable {
        manager.enable()
    } else {
        manager.disable()
    };
}

// ---------------------------------------------------------------------------
// Shared Drop Folders
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePairResult {
    pub pair: Pair,
    pub invite: String,
}

#[tauri::command]
pub fn create_pair(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    folder: String,
    two_way: bool,
    peer_name: Option<String>,
    mirror: Option<bool>,
) -> Result<CreatePairResult, String> {
    let name = state.settings.lock().unwrap().display_name.clone();
    let (pair, invite) = pairing::create(
        &state.config_dir,
        folder,
        name,
        two_way,
        peer_name.unwrap_or_default(),
        mirror.unwrap_or(false),
    )?;
    sync.reconcile();
    Ok(CreatePairResult { pair, invite })
}

#[tauri::command]
pub fn accept_pair(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    invite: String,
    folder: String,
) -> Result<Pair, String> {
    let pair = pairing::accept(&state.config_dir, &invite, folder)?;
    sync.reconcile();
    Ok(pair)
}

#[tauri::command]
pub fn list_pairs(state: State<'_, Arc<AppState>>) -> Vec<Pair> {
    pairing::load(&state.config_dir)
}

#[tauri::command]
pub fn update_pair(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
    two_way: Option<bool>,
    auto_delete: Option<bool>,
    delete_mode: Option<DeleteMode>,
    peer_name: Option<String>,
    mirror: Option<bool>,
) -> Result<Pair, String> {
    let pair = pairing::update(
        &state.config_dir,
        &id,
        two_way,
        auto_delete,
        delete_mode,
        peer_name,
        mirror,
    )?;
    sync.reconcile();
    Ok(pair)
}

#[tauri::command]
pub fn remove_pair(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
) -> Result<(), String> {
    pairing::remove(&state.config_dir, &id)?;
    sync.reconcile();
    Ok(())
}

#[tauri::command]
pub fn pair_invite(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    let name = state.settings.lock().unwrap().display_name.clone();
    let pairs = pairing::load(&state.config_dir);
    let pair = pairs.iter().find(|p| p.id == id).ok_or("Pair not found.")?;
    Ok(pairing::invite_for(pair, &name))
}

#[tauri::command]
pub fn get_folder_statuses(sync: State<'_, Arc<SyncManager>>) -> Vec<FolderStatus> {
    sync.statuses()
}

// ---------------------------------------------------------------------------
// Friends — named peers you send to directly, no per-transfer code.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFriendResult {
    pub friend: Friend,
    pub invite: String,
}

/// Create a friend invite (this device is A). `friendName` is your label for them;
/// the invite carries your own display name so they see who added them.
#[tauri::command]
pub fn create_friend(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    friend_name: String,
) -> Result<CreateFriendResult, String> {
    let my_name = state.settings.lock().unwrap().display_name.clone();
    let (friend, invite) = friends::create(&state.config_dir, my_name, friend_name)?;
    sync.reconcile_friends();
    Ok(CreateFriendResult { friend, invite })
}

/// Accept a friend invite (this device is B). You're named after the inviter.
#[tauri::command]
pub fn accept_friend(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    invite: String,
) -> Result<Friend, String> {
    let friend = friends::accept(&state.config_dir, &invite)?;
    sync.reconcile_friends();
    Ok(friend)
}

#[tauri::command]
pub fn list_friends(state: State<'_, Arc<AppState>>) -> Vec<Friend> {
    friends::load(&state.config_dir)
}

#[tauri::command]
pub fn rename_friend(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
    name: String,
) -> Result<(), String> {
    friends::rename(&state.config_dir, &id, name)?;
    sync.reconcile_friends();
    Ok(())
}

#[tauri::command]
pub fn remove_friend(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
) -> Result<(), String> {
    friends::remove(&state.config_dir, &id)?;
    sync.reconcile_friends();
    Ok(())
}

/// Toggle whether a friend's files arrive automatically (true) or require the
/// user to accept each one (false). Restarts that friend's listener.
#[tauri::command]
pub fn set_friend_auto_accept(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    id: String,
    auto_accept: bool,
) -> Result<(), String> {
    friends::set_auto_accept(&state.config_dir, &id, auto_accept)?;
    sync.reconcile_friends();
    Ok(())
}

/// Answer a pending manual-accept offer (the user tapped Accept or Decline).
#[tauri::command]
pub fn respond_to_offer(state: State<'_, Arc<AppState>>, id: String, accept: bool) {
    if let Some(tx) = state.offers.lock().unwrap().remove(&id) {
        let _ = tx.send(accept);
    }
}

/// Actively check whether a friend is online: beam them a tiny throwaway marker
/// and report whether it was delivered (their app caught it). Can take up to ~70s
/// if their app is idle; returns false if they don't answer in that window.
#[tauri::command]
pub async fn ping_friend(state: State<'_, Arc<AppState>>, id: String) -> Result<bool, String> {
    let friend = friends::get(&state.config_dir, &id).ok_or("Friend not found.")?;
    let code = friends::friend_inbox_code(&friend);
    let settings = { state.settings.lock().unwrap().clone() };
    let config_dir = state.config_dir.clone();
    Ok(crate::sync::ping_send(&settings, &code, &config_dir).await)
}

/// Rebuild a friend's invite so the inviter can show it again.
#[tauri::command]
pub fn friend_invite(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    let my_name = state.settings.lock().unwrap().display_name.clone();
    let friend = friends::get(&state.config_dir, &id).ok_or("Friend not found.")?;
    Ok(friends::invite_for(&friend, &my_name))
}

// ---------------------------------------------------------------------------
// Shared folder history (total-sync / mirror folders)
// ---------------------------------------------------------------------------

fn folder_for(state: &Arc<AppState>, pair_id: &str) -> Option<String> {
    pairing::load(&state.config_dir)
        .into_iter()
        .find(|p| p.id == pair_id)
        .map(|p| p.folder)
}

#[tauri::command]
pub fn list_folder_history(state: State<'_, Arc<AppState>>, pair_id: String) -> Vec<HistoryItem> {
    match folder_for(state.inner(), &pair_id) {
        Some(folder) => {
            let mut items = folder_history::load(&folder);
            items.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
            items
        }
        None => Vec::new(),
    }
}

/// Restore a deleted/overwritten file back into the folder. In a mirror folder
/// it then re-syncs to the peer automatically.
#[tauri::command]
pub fn restore_folder_item(
    state: State<'_, Arc<AppState>>,
    pair_id: String,
    item_id: String,
) -> Result<(), String> {
    let folder = folder_for(state.inner(), &pair_id).ok_or("Folder not found.")?;
    folder_history::restore(&folder, &item_id)?;
    Ok(())
}

#[tauri::command]
pub fn forget_folder_item(state: State<'_, Arc<AppState>>, pair_id: String, item_id: String) {
    if let Some(folder) = folder_for(state.inner(), &pair_id) {
        folder_history::forget(&folder, &item_id);
    }
}

/// Send files straight to a friend — no code, no QR. Their inbox listener is
/// already waiting on the derived channel, so it just arrives.
#[tauri::command]
pub fn send_to_friend(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    paths: Vec<String>,
) -> Result<TransferUpdate, String> {
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.trim().is_empty()).collect();
    if paths.is_empty() {
        return Err("No files selected.".into());
    }
    for p in &paths {
        if !std::path::Path::new(p).exists() {
            return Err(format!("File not found: {p}"));
        }
    }
    let friend = friends::get(&state.config_dir, &id).ok_or("Friend not found.")?;
    let code = friends::friend_inbox_code(&friend);
    Ok(croc::start_send(
        app,
        state.inner().clone(),
        paths,
        Some(code),
        Some(friend.name),
    ))
}

// ── iroh transport (Phase 1: foundation / diagnostics) ───────────────────────

/// Our stable iroh node id (None during the brief startup window). Lets the UI
/// show "your device id" and confirms the new transport is up.
#[tauri::command]
pub async fn iroh_node_id(
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
) -> Result<Option<String>, String> {
    Ok(iroh.get().map(|ep| ep.id().to_string()))
}

/// Prove the iroh transport works inside the running app (loopback round-trip
/// over a real QUIC stream). Surfaced in Settings as a diagnostic.
#[tauri::command]
pub async fn iroh_selftest(
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
) -> Result<String, String> {
    let ep = iroh
        .get()
        .cloned()
        .ok_or("iroh transport is still starting up")?;
    crate::iroh_net::self_test(&ep).await.map_err(|e| e.to_string())
}

/// Quick Send over the iroh direct engine: stage files, return a ticket-bearing
/// update. The receiver pulls with `iroh_receive`.
#[tauri::command]
pub fn iroh_send(
    app: AppHandle,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    paths: Vec<String>,
) -> Result<TransferUpdate, String> {
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.trim().is_empty()).collect();
    if paths.is_empty() {
        return Err("No files selected.".into());
    }
    for p in &paths {
        if !std::path::Path::new(p).exists() {
            return Err(format!("File not found: {p}"));
        }
    }
    crate::iroh_net::start_send(app, iroh.inner().clone(), paths)
}

/// Receive a Direct Quick Send by pasting its ticket.
#[tauri::command]
pub fn iroh_receive(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    ticket: String,
) -> Result<TransferUpdate, String> {
    let ticket = ticket.trim().to_string();
    if ticket.is_empty() {
        return Err("Paste a Direct ticket to receive.".into());
    }
    let configured = { state.settings.lock().unwrap().download_dir.clone() };
    let out = if configured.trim().is_empty() {
        app.path()
            .download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| format!("No download folder available: {e}"))?
    } else {
        configured
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("Can't write to download folder: {e}"))?;
    crate::iroh_net::start_receive(app, iroh.inner().clone(), ticket, out)
}
