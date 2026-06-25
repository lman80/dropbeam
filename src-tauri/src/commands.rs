//! Tauri commands invoked from the React frontend.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::chat::{self, ChatMessage};
use crate::models::{
    DeleteMode, FolderStatus, Friend, HistoryEntry, HistoryItem, Pair, Settings, TransferUpdate,
};
use crate::sync::SyncManager;
use crate::{folder_history, friends, history, pairing, settings, AppState};

#[tauri::command]
pub fn cancel_transfer(
    app: AppHandle,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
) {
    use crate::iroh_net::CancelKind;
    match iroh.cancel(&id) {
        // A staged send isn't running a loop, so report Canceled here.
        CancelKind::Staged => crate::iroh_net::emit_canceled_send(&app, &id),
        // An in-flight iroh transfer reports Canceled from its own loop.
        CancelKind::Active => {}
        // Not a known iroh transfer — nothing to cancel.
        CancelKind::Unknown => {}
    }
}

/// Settings "Clear transfer cache": delete every abandoned resumable partial
/// (paused/failed transfer leftovers) that isn't actively being written.
/// Returns the number of bytes freed so the UI can show it.
#[tauri::command]
pub fn clear_transfer_cache(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
) -> u64 {
    iroh.clear_transfer_cache(&state.config_dir)
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
    // Bound the display name before it's stored AND broadcast to every peer — an
    // unbounded name would bloat the control frame and could push past its header
    // limit. 64 chars is ample for a name.
    let mut settings = settings;
    settings.display_name = settings.display_name.trim().chars().take(64).collect();

    // Persist FIRST. If the disk write fails (e.g. Windows write contention), return
    // the error WITHOUT mutating in-memory state or applying any live side effect —
    // so memory never silently diverges from what's on disk (the frontend rolls its
    // optimistic UI back on the Err). Only once it's durably saved do we apply it.
    let (retention_changed, name_changed) = {
        let guard = state.settings.lock().unwrap();
        (
            guard.folder_history_keep_days != settings.folder_history_keep_days
                || guard.folder_history_budget_bytes != settings.folder_history_budget_bytes,
            guard.display_name != settings.display_name,
        )
    };
    settings::save(&state.config_dir, &settings)?;

    // Saved — now apply live (no restart needed) and commit to in-memory state.
    apply_autostart(&app, settings.launch_at_login);
    // Internet upload cap (0 = unlimited) — takes effect on the next chunk.
    crate::iroh_net::set_upload_limit_mbps(settings.upload_limit_mbps);
    crate::iroh_net::set_require_direct(settings.require_direct);
    crate::iroh_net::set_wait_for_direct(settings.wait_for_direct);
    crate::iroh_net::set_parallel_streams(settings.parallel_streams);
    // Recovery-history retention, then sweep so a tightened budget/age frees space
    // immediately (not just on the next delete).
    crate::folder_history::set_policy(
        settings.folder_history_keep_days,
        settings.folder_history_budget_bytes,
    );
    *state.settings.lock().unwrap() = settings.clone();
    if retention_changed {
        sweep_folder_history(&app, state.inner());
    }
    // If the user renamed themselves, push the new name out to all friends.
    if name_changed {
        crate::iroh_net::broadcast_profile(app.clone(), iroh.inner().clone());
    }
    // iroh always runs (it's the only transport); start it if it somehow isn't
    // up yet. The `app` OnceCell guards against double-starting.
    if iroh.app.get().is_none() {
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

/// Let the user pick an image and set it as their profile picture. The chosen
/// file is COPIED into the app config dir under a fresh name (so the webview's
/// asset cache always loads the new image) and any previous avatar is removed.
/// Returns the updated settings. Cancelling leaves settings unchanged.
#[tauri::command]
pub async fn set_profile_avatar(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Settings, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "heic", "bmp"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let picked = match rx.await {
        Ok(Some(p)) => p.into_path().map_err(|e| e.to_string())?,
        _ => return Ok(state.settings.lock().unwrap().clone()),
    };
    let ext = picked
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let config_dir = state.config_dir.clone();
    let _ = std::fs::create_dir_all(&config_dir);
    // Drop any earlier avatar files so they don't accumulate in the config dir.
    if let Ok(rd) = std::fs::read_dir(&config_dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("avatar-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let dest = config_dir.join(format!("avatar-{}.{ext}", crate::chat::now_ms()));
    std::fs::copy(&picked, &dest).map_err(|e| e.to_string())?;
    let updated = {
        let mut g = state.settings.lock().unwrap();
        g.avatar = dest.to_string_lossy().to_string();
        g.clone()
    };
    settings::save(&config_dir, &updated)?;
    broadcast_profile(&app);
    Ok(updated)
}

/// Remove the profile picture, falling back to the initials avatar.
#[tauri::command]
pub fn clear_profile_avatar(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Settings, String> {
    let config_dir = state.config_dir.clone();
    let updated = {
        let mut g = state.settings.lock().unwrap();
        if !g.avatar.is_empty() {
            let _ = std::fs::remove_file(&g.avatar);
        }
        g.avatar = String::new();
        g.clone()
    };
    settings::save(&config_dir, &updated)?;
    broadcast_profile(&app);
    Ok(updated)
}

/// Push the user's current profile (name + picture) to all friends, if iroh is up.
fn broadcast_profile(app: &AppHandle) {
    use tauri::Manager;
    if let Some(iroh) = app.try_state::<Arc<crate::iroh_net::IrohState>>() {
        crate::iroh_net::broadcast_profile(app.clone(), iroh.inner().clone());
    }
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

/// Bundle every `DropBeam*.log` file (oldest → newest) plus a redacted header
/// (version, OS, settings, friend/folder counts) into ONE text file in the
/// Downloads folder, and return its path. The user sends that single file back —
/// over DropBeam itself or AirDrop — for analysis. No secrets are included.
#[tauri::command]
pub fn export_diagnostics(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    use std::fmt::Write as _;
    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let mut out = String::new();
    let _ = writeln!(out, "===== DropBeam diagnostics =====");
    let _ = writeln!(out, "app version : {}", app.package_info().version);
    let _ = writeln!(
        out,
        "os / arch   : {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(out, "exported    : {now_ms} (epoch ms)");
    {
        let s = state.settings.lock().unwrap();
        let _ = writeln!(out, "verbose log : {}", s.verbose_logging);
        let _ = writeln!(
            out,
            "transport   : direct_mode={} require_direct={} upload_cap_mbps={}",
            s.direct_mode, s.require_direct, s.upload_limit_mbps
        );
        let _ = writeln!(
            out,
            "relay       : {}",
            if s.custom_relay.is_empty() {
                "(public)".to_string()
            } else {
                s.custom_relay.clone()
            }
        );
    }
    let friends = friends::load(&state.config_dir);
    let pairs = pairing::load(&state.config_dir);
    let _ = writeln!(out, "friends     : {}", friends.len());
    let _ = writeln!(out, "shared dirs : {}", pairs.len());
    for p in &pairs {
        let _ = writeln!(
            out,
            "  - peer={:?} mirror={} group={:?} folder={:?}",
            p.peer_name, p.mirror, p.group_id, p.folder
        );
    }
    let _ = writeln!(out, "================================");

    // Every DropBeam log file, oldest first, so the story reads top-to-bottom.
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&log_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("DropBeam") && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    if files.is_empty() {
        let _ = writeln!(out, "\n(no log files found in {log_dir:?})");
    }
    for f in &files {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = writeln!(out, "\n\n========== {name} ==========");
        match std::fs::read_to_string(f) {
            Ok(t) => out.push_str(&t),
            Err(e) => {
                let _ = writeln!(out, "(could not read: {e})");
            }
        }
    }

    let downloads = app.path().download_dir().unwrap_or_else(|_| log_dir.clone());
    let dest = downloads.join(format!("DropBeam-diagnostics-{now_ms}.txt"));
    std::fs::write(&dest, out.as_bytes()).map_err(|e| e.to_string())?;
    log::info!("export_diagnostics: wrote {} bytes to {dest:?}", out.len());
    // Reveal it in Finder/Explorer so the user doesn't have to hunt through Downloads
    // for the file they're about to send back. Best-effort — never fail the export.
    {
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().reveal_item_in_dir(&dest);
    }
    Ok(dest.to_string_lossy().to_string())
}

/// Send one diagnostics digest right now (the "Send a test now" button), so the
/// operator can confirm their collector endpoint works without waiting for the
/// daily upload. Returns a human summary or an error.
#[tauri::command]
pub async fn diagnostics_test(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let config_dir = state.config_dir.clone();
    let log_dir = app.path().app_log_dir().ok();
    crate::telemetry::run_once(&app, &config_dir, log_dir.as_deref()).await
}

/// Relaunch the app — used to apply the verbose-logging toggle (the file log's
/// level is fixed at startup).
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

/// Open a path with the system default handler. A folder opens INTO itself (not
/// revealed in its parent); a file launches in its default app. Resilient: if the
/// exact path no longer exists (a received file got a unique name on a collision,
/// or a folder moved), open the nearest folder that DOES exist instead of
/// erroring — so the button always lands somewhere sensible. Logged for diagnosis.
#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let requested = std::path::PathBuf::from(&path);
    let existed = requested.exists();
    let mut target = requested.clone();
    while !target.exists() {
        match target.parent() {
            Some(p) if p != target && !p.as_os_str().is_empty() => target = p.to_path_buf(),
            _ => break,
        }
    }
    log::info!("open_path: requested={requested:?} existed={existed} -> opening={target:?}");
    app.opener()
        .open_path(target.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open a URL in the user's default browser — used as a manual-download fallback
/// when the in-app updater can't reach GitHub (common when GitHub is throttled
/// from China; the friend can grab the installer from the Releases page instead).
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    // Only ever open real web links from this generic command — never file://,
    // smb://, x-apple.systempreferences:, or a custom app handler that could be
    // smuggled in via an untrusted name/filename rendered as a link.
    let lower = url.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("Only http and https links can be opened.".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// True when transfers are stuck on the slow relay despite a peer being on the
/// local network — the fingerprint of the macOS "Local Network" permission being
/// off. Drives the in-app nudge. (See `iroh_net::lan_path_blocked`.)
#[tauri::command]
pub fn lan_network_blocked() -> bool {
    crate::iroh_net::lan_path_blocked()
}

/// Deep-link straight to System Settings → Privacy & Security → Local Network so
/// the user can enable DropBeam in one click (no scavenger hunt).
#[tauri::command]
pub fn open_local_network_settings(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

/// On macOS, return a one-line warning if the app is running from a spot that
/// makes the system forget folder permissions every launch — App Translocation
/// (a quarantined app run from a randomized read-only path) or straight from the
/// disk image / Downloads. The fix is always "move it to /Applications and
/// reopen". Returns None when the app is installed properly (or on other OSes).
#[tauri::command]
pub fn macos_install_hint() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().ok()?;
        let p = exe.to_string_lossy();
        if p.contains("/AppTranslocation/") {
            return Some(
                "DropBeam is running from a temporary, read-only location, so macOS forgets your \
                 folder permission every launch. Drag DropBeam into your Applications folder (by \
                 itself), then reopen it from there."
                    .into(),
            );
        }
        if p.starts_with("/Volumes/") || p.contains("/Downloads/") {
            return Some(
                "DropBeam is running from the disk image or your Downloads folder. Move it into \
                 Applications and reopen it so macOS remembers your permissions."
                    .into(),
            );
        }
    }
    None
}

/// Bring the main window forward (from the menu-bar popover or the HUD) and
/// tuck the popover away.
#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    // Restore the Dock icon + normal focus while the window is open.
    crate::set_dock_icon_visible(&app, true);
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

/// Fully quit DropBeam. As a menu-bar app there's no Dock icon to right-click,
/// so the popover offers an explicit Quit. `force_quit` makes the close-to-tray
/// handler let the app actually exit instead of hiding to the menu bar.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.force_quit.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    app.exit(0);
}

/// The send/receive transfer card became visible (`active=true`) or went away
/// (`active=false`). While a card is on screen we briefly become a regular app
/// (Dock icon), so its native yellow button can minimize it INTO the Dock and
/// you can click it back — just like any Mac window. When the card is gone we
/// drop back to a menu-bar-only app (no Dock icon), unless the main window is
/// open. macOS-only effect; harmless elsewhere.
#[tauri::command]
pub fn set_card_active(app: AppHandle, active: bool) {
    if active {
        crate::set_dock_icon_visible(&app, true);
        return;
    }
    let main_open = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !main_open {
        crate::set_dock_icon_visible(&app, false);
    }
}

#[tauri::command]
pub fn get_default_download_dir(app: AppHandle) -> String {
    app.path()
        .download_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub(crate) fn apply_autostart(app: &AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    // ONLY touch the macOS LaunchAgent plist when the state actually needs to change.
    // The old code called enable()/disable() on every launch + every settings save,
    // and the underlying crate rewrites ~/Library/LaunchAgents/DropBeam.plist via
    // File::create EACH call — which makes macOS post a "Background items added —
    // DropBeam" notification every single time (the user's "over and over" popup).
    // is_enabled() is just a file-exists check (no write, no notification), so this
    // makes both call sites idempotent: a normal launch with autostart already
    // registered does nothing and macOS stays silent. The plist is still (re)written
    // on a genuine first-enable or if something deleted it — so updates are covered.
    let current = manager.is_enabled().unwrap_or(false);
    if enable == current {
        return;
    }
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
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    folder: String,
    two_way: bool,
    peer_name: Option<String>,
    mirror: Option<bool>,
) -> Result<CreatePairResult, String> {
    let name = state.settings.lock().unwrap().display_name.clone();
    // Carry our iroh device key in the invite so the folder syncs directly.
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    let (pair, invite) = pairing::create(
        &state.config_dir,
        folder,
        name,
        two_way,
        peer_name.unwrap_or_default(),
        mirror.unwrap_or(false),
        my_id,
    )?;
    sync.reconcile();
    Ok(CreatePairResult { pair, invite })
}

#[tauri::command]
pub fn accept_pair(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    invite: String,
    folder: String,
) -> Result<Pair, String> {
    let pair = pairing::accept(&state.config_dir, &invite, folder)?;
    // Dial the creator back to hand them our iroh id (the invite gave us theirs),
    // so both directions of this folder can push directly over iroh.
    if let Some(inviter_eid) = pair.endpoint_id.clone() {
        let my_name = state.settings.lock().unwrap().display_name.clone();
        crate::iroh_net::say_hello_folder(
            iroh.inner().clone(),
            pair.id.clone(),
            inviter_eid,
            my_name,
        );
    }
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
    // Let the peer know BEFORE we forget the link, so their side can show
    // "no longer shared by ___" instead of a silently-dead folder.
    sync.announce_unshare(&id);
    pairing::remove(&state.config_dir, &id)?;
    sync.reconcile();
    Ok(())
}

/// Owner action: make a folder member a viewer (read-only) or an editor again.
/// `id` is the owner's link to that member; the role rides the roster beacon to
/// the whole group, so the member stops/starts sending accordingly.
#[tauri::command]
pub fn set_member_role(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
    viewer: bool,
) -> Result<(), String> {
    // Only the folder OWNER may assign roles — enforced in the engine, not just
    // the UI, because every peer trusts the owner_eid relayed on the beacon.
    let my_eid = iroh.get().map(|ep| ep.id().to_string());
    if !pairing::set_peer_viewer(&state.config_dir, &id, viewer, my_eid.as_deref()) {
        return Err("Only the folder's owner can change who can edit.".into());
    }
    sync.reconcile();
    let _ = app.emit("pairs://changed", ());
    Ok(())
}

/// This device's own iroh endpoint id (None until iroh is up). The UI compares it
/// to a folder's `ownerEid` to show role controls only to the actual owner.
#[tauri::command]
pub fn my_endpoint_id(iroh: State<'_, Arc<crate::iroh_net::IrohState>>) -> Option<String> {
    iroh.get().map(|ep| ep.id().to_string())
}

/// Manually trigger the self-heal reconcile for every shared folder NOW — each
/// side re-exchanges its manifest and converges to identical. Drives the
/// "Verify" button.
#[tauri::command]
pub fn verify_folders(sync: State<'_, Arc<SyncManager>>) {
    sync.verify_now();
}

/// Drive a REAL verification for ONE shared folder and report a trustworthy answer:
/// are the two folders identical, how many files match, and how many differences
/// were found (and are being fixed). Reuses the existing reconcile/manifest-exchange
/// — it re-exchanges manifests now, waits for the peer's fresh snapshot, then
/// compares the two using the same per-file signatures the self-heal already uses.
/// Drives the "Verify" button's confirmation. `pair_id` is the folder link to check.
#[tauri::command]
pub async fn verify_folder(
    sync: State<'_, Arc<SyncManager>>,
    pair_id: String,
) -> Result<crate::models::VerifyResult, String> {
    // Clone the Arc out of State so we don't hold the (non-Send) guard across .await.
    let sync = sync.inner().clone();
    Ok(sync.verify_folder(&pair_id).await)
}

/// Abort the in-flight transfer for a shared folder (the "Stop" button) so a
/// stuck send can't trap the queue. The file isn't dropped — it cycles back.
#[tauri::command]
pub fn stop_folder_transfer(sync: State<'_, Arc<SyncManager>>, pair_id: String) {
    sync.stop_folder_transfer(&pair_id);
}

/// Pause or resume sync for a shared folder. A SHARED switch: this flips the whole
/// folder (every group member) and beacons the new state to the peer, where the
/// newest toggle wins. While paused nothing syncs; Resume runs the normal reconcile.
#[tauri::command]
pub fn set_folder_paused(sync: State<'_, Arc<SyncManager>>, pair_id: String, paused: bool) {
    sync.set_paused(&pair_id, paused, chat::now_ms());
}

#[tauri::command]
pub fn pair_invite(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
) -> Result<String, String> {
    let name = state.settings.lock().unwrap().display_name.clone();
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    let pairs = pairing::load(&state.config_dir);
    let pair = pairs.iter().find(|p| p.id == id).ok_or("Pair not found.")?;
    Ok(pairing::invite_for(pair, &name, my_id))
}

/// Invite ANOTHER person into an existing shared folder (making it a group of
/// 3+). Returns a fresh invite; once they accept, the group roster meshes
/// everyone together over the control beacon.
#[tauri::command]
pub fn folder_add_person(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    sync: State<'_, Arc<SyncManager>>,
    app: AppHandle,
    id: String,
) -> Result<String, String> {
    let name = state.settings.lock().unwrap().display_name.clone();
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    let invite = pairing::group_invite(&state.config_dir, &id, name, my_id)?;
    sync.reconcile(); // start the new pending link so the newcomer's hello lands
    let _ = app.emit("pairs://changed", ());
    Ok(invite)
}

/// Invite an existing FRIEND straight into a shared folder — no copy-pasted code.
/// Generates a group invite for `pair_id` and pushes it to the friend over iroh;
/// on their side a prompt appears to accept and pick a local folder. If the friend
/// is offline the push is best-effort (the inviter still has the code to share).
#[tauri::command]
pub fn invite_friend_to_folder(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    sync: State<'_, Arc<SyncManager>>,
    app: AppHandle,
    pair_id: String,
    friend_id: String,
    // An EXISTING invite code to reuse (e.g. the one create_pair just produced for
    // its first pending link) instead of minting a new one — so the folder's
    // original pending link is consumed by this friend rather than left dangling as
    // a stuck "waiting to join" member. None → mint a fresh group invite.
    code: Option<String>,
) -> Result<(), String> {
    let friend = friends::load(&state.config_dir)
        .into_iter()
        .find(|f| f.id == friend_id)
        .ok_or("Friend not found.")?;
    let eid = friend
        .endpoint_id
        .ok_or("That friend hasn't connected yet — share the invite code instead.")?;
    let name = state.settings.lock().unwrap().display_name.clone();
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    let invite = match code {
        Some(c) if !c.trim().is_empty() => c,
        _ => pairing::group_invite(&state.config_dir, &pair_id, name.clone(), my_id)?,
    };
    let folder_name = pairing::load(&state.config_dir)
        .iter()
        .find(|p| p.id == pair_id)
        .map(|p| {
            std::path::Path::new(&p.folder)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    crate::iroh_net::send_folder_invite(iroh.inner().clone(), eid, invite, folder_name, name);
    sync.reconcile();
    let _ = app.emit("pairs://changed", ());
    Ok(())
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
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    friend_name: String,
) -> Result<CreateFriendResult, String> {
    let my_name = state.settings.lock().unwrap().display_name.clone();
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    let (friend, invite) = friends::create(&state.config_dir, my_name, friend_name, my_id)?;
    sync.reconcile_friends();
    Ok(CreateFriendResult { friend, invite })
}

/// Accept a friend invite (this device is B). You're named after the inviter.
#[tauri::command]
pub fn accept_friend(
    state: State<'_, Arc<AppState>>,
    sync: State<'_, Arc<SyncManager>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    invite: String,
) -> Result<Friend, String> {
    let friend = friends::accept(&state.config_dir, &invite)?;
    sync.reconcile_friends();
    // Dial the inviter back to hand them our iroh id, so they can send to us
    // directly too (the invite already gave us theirs).
    if let Some(inviter_id) = friend.endpoint_id.clone() {
        let my_name = state.settings.lock().unwrap().display_name.clone();
        crate::iroh_net::say_hello(iroh.inner().clone(), friend.id.clone(), inviter_id, my_name);
    }
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
    chat::clear(&state.config_dir, &id);
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
/// `dest` is an optional save folder from the receive card's "Save to" picker —
/// empty / None means the default download folder.
#[tauri::command]
pub fn respond_to_offer(
    state: State<'_, Arc<AppState>>,
    id: String,
    accept: bool,
    dest: Option<String>,
) {
    if let Some(tx) = state.offers.lock().unwrap().remove(&id) {
        let _ = tx.send(if accept { Some(dest.unwrap_or_default()) } else { None });
    }
}

/// Actively check whether a friend is online: dial their iroh endpoint and
/// round-trip a ping, reporting whether they answered. Returns false quickly if
/// they have no direct address yet (paired pre-Direct-mode) or don't answer.
#[tauri::command]
pub async fn ping_friend(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
) -> Result<bool, String> {
    let friend = friends::get(&state.config_dir, &id).ok_or("Friend not found.")?;
    let Some(eid) = friend.endpoint_id.clone() else {
        return Ok(false);
    };
    let Some(ep) = iroh.get().cloned() else {
        return Ok(false);
    };
    Ok(crate::iroh_net::ping_endpoint(&ep, &eid).await)
}

/// Probe how we're connected to a friend right now (connection inspector). Returns
/// the live path detail, or None if they're unreachable / not on a direct build.
#[tauri::command]
pub async fn probe_connection(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    friend_id: String,
) -> Result<Option<crate::models::ConnDetail>, String> {
    let friend = friends::get(&state.config_dir, &friend_id).ok_or("Friend not found.")?;
    let (Some(ep), Some(eid)) = (iroh.get().cloned(), friend.endpoint_id) else {
        return Ok(None);
    };
    Ok(crate::iroh_net::probe_conn(&ep, &eid).await)
}

/// "Send over relay anyway": break the wait-for-direct park for one transfer (by
/// transfer id) or one folder (by pair id), so it proceeds on the relay now.
#[tauri::command]
pub fn force_relay(id: String) {
    crate::iroh_net::force_relay(&id);
}

/// Rebuild a friend's invite so the inviter can show it again.
#[tauri::command]
pub fn friend_invite(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    id: String,
) -> Result<String, String> {
    let my_name = state.settings.lock().unwrap().display_name.clone();
    let friend = friends::get(&state.config_dir, &id).ok_or("Friend not found.")?;
    let my_id = iroh.get().map(|ep| ep.id().to_string());
    Ok(friends::invite_for(&friend, &my_name, my_id))
}

// ---------------------------------------------------------------------------
// Chat (experimental) — direct messages + file shares with friends over iroh
// ---------------------------------------------------------------------------

/// Every message in the conversation with a friend, oldest first.
#[tauri::command]
pub fn get_chat_messages(state: State<'_, Arc<AppState>>, friend_id: String) -> Vec<ChatMessage> {
    chat::messages(&state.config_dir, &friend_id)
}

/// A preview of every conversation (last message + count), newest first.
#[tauri::command]
pub fn list_chats(state: State<'_, Arc<AppState>>) -> Vec<chat::ChatOverview> {
    chat::overview(&state.config_dir)
}

/// Send a text message to a friend. Persists + surfaces it immediately, then
/// delivers over iroh in the background (online-only — no store-and-forward yet).
#[tauri::command]
pub async fn send_chat_message(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    text: String,
    reply_to: Option<String>,
    reply_preview: Option<String>,
) -> Result<ChatMessage, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Message is empty.".into());
    }
    // Bound the length so a giant paste can't blow past the frame header limit.
    let body: String = trimmed.chars().take(4000).collect();
    let friend = friends::get(&state.config_dir, &friend_id).ok_or("Friend not found.")?;
    let msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        peer_id: friend_id.clone(),
        from_me: true,
        kind: "text".into(),
        text: body,
        files: vec![],
        bytes: 0,
        path: None,
        status: Some("sending".into()),
        ts: chat::now_ms(),
        seq: chat::next_seq(&state.config_dir, &friend_id),
        reply_to,
        reply_preview: reply_preview.map(|p| p.chars().take(160).collect()),
        reactions: vec![],
        edited: false,
        deleted: false,
        gif: None,
    };
    chat::append(&state.config_dir, &msg);
    let _ = app.emit("chat://message", &msg);
    deliver_chat(&state, &iroh, &app, &friend, &msg);
    // No endpoint yet → leave it "sending"; the outbox retry flushes it once the
    // friend is reachable (self-healing learns their key on first contact).
    Ok(msg)
}

/// Fire-and-forget delivery of a stored chat message over iroh, flipping its
/// status to "delivered"/"failed" and re-emitting. Shared by text/file/gif sends.
fn deliver_chat(
    state: &Arc<AppState>,
    iroh: &Arc<crate::iroh_net::IrohState>,
    app: &AppHandle,
    friend: &crate::models::Friend,
    msg: &ChatMessage,
) {
    if let (Some(ep), Some(eid)) = (iroh.get().cloned(), friend.endpoint_id.clone()) {
        let my_name = state.settings.lock().unwrap().display_name.clone();
        let payload = crate::iroh_net::chat_payload(msg, &friend.id, &my_name);
        let config_dir = state.config_dir.clone();
        let (pid, mid) = (friend.id.clone(), msg.id.clone());
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let status = match crate::iroh_net::send_chat(&ep, &eid, payload).await {
                Ok(_) => "delivered",
                Err(e) => {
                    log::debug!("chat send failed: {e:#}");
                    "failed"
                }
            };
            if let Some(u) = chat::set_status(&config_dir, &pid, &mid, status) {
                let _ = app.emit("chat://message", &u);
            }
        });
    }
}

/// Record + deliver a "file" chat message. The bytes themselves ride the normal
/// friend transfer (`send_to_friend`); this just puts a card in the thread on
/// both sides so a shared file shows up in the conversation.
#[tauri::command]
pub async fn send_chat_file_note(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    names: Vec<String>,
    bytes: u64,
    paths: Vec<String>,
) -> Result<ChatMessage, String> {
    post_file_note(&state, &iroh, &app, &friend_id, names, bytes, paths)
        .ok_or_else(|| "Friend not found.".to_string())
}

/// Post a "sent you a file" note into a friend's chat thread: persist it, echo it to
/// every window (`chat://message`), and deliver it to the peer. Shared by the
/// `send_chat_file_note` command AND the native menu-bar/tray drag-send (which
/// bypasses the JS store), so a direct send shows up in the conversation no matter
/// how it was initiated (GitHub #23).
pub(crate) fn post_file_note(
    state: &Arc<AppState>,
    iroh: &Arc<crate::iroh_net::IrohState>,
    app: &AppHandle,
    friend_id: &str,
    names: Vec<String>,
    bytes: u64,
    paths: Vec<String>,
) -> Option<ChatMessage> {
    let friend = friends::get(&state.config_dir, friend_id)?;
    let msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        peer_id: friend_id.to_string(),
        from_me: true,
        kind: "file".into(),
        text: String::new(),
        files: names,
        bytes,
        // Sender keeps the source path so they can preview/open what they sent.
        path: paths.into_iter().next(),
        status: Some("sending".into()),
        ts: chat::now_ms(),
        seq: chat::next_seq(&state.config_dir, friend_id),
        reply_to: None,
        reply_preview: None,
        reactions: vec![],
        edited: false,
        deleted: false,
        gif: None,
    };
    chat::append(&state.config_dir, &msg);
    let _ = app.emit("chat://message", &msg);
    deliver_chat(state, iroh, app, &friend, &msg);
    Some(msg)
}

// --- New chat capabilities: reactions, edit, delete, typing, read, GIFs -----

/// Build + send a small chat control op (reaction/edit/delete) to a friend over
/// iroh, retrying a few times so it lands even if the first dial misses. These
/// mutate an existing message rather than create one, so they're not queued in
/// the outbox; best-effort with retry is the right reliability tier for them.
fn send_chat_op(
    iroh: &Arc<crate::iroh_net::IrohState>,
    eid: String,
    payload: serde_json::Value,
) {
    let Some(ep) = iroh.get().cloned() else { return };
    tauri::async_runtime::spawn(async move {
        for attempt in 0..3u8 {
            if crate::iroh_net::send_chat(&ep, &eid, payload.clone()).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2 * (attempt as u64 + 1))).await;
        }
    });
}

/// Add or remove an emoji reaction on a message (ours or the friend's). Applies
/// locally immediately, then tells the peer.
#[tauri::command]
pub async fn react_to_message(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    message_id: String,
    emoji: String,
    add: bool,
) -> Result<(), String> {
    if let Some(u) = chat::apply_reaction(&state.config_dir, &friend_id, &message_id, &emoji, true, add) {
        let _ = app.emit("chat://message", &u);
    }
    if let Some(friend) = friends::get(&state.config_dir, &friend_id) {
        if let Some(eid) = friend.endpoint_id {
            let my_name = state.settings.lock().unwrap().display_name.clone();
            let payload = serde_json::json!({
                "kind": "chat", "v": 2, "msgKind": "reaction", "friendId": friend_id,
                "fromName": my_name, "targetId": message_id, "emoji": emoji, "add": add,
            });
            send_chat_op(&iroh, eid, payload);
        }
    }
    Ok(())
}

/// Edit the text of a message we sent. Applies locally, then tells the peer.
#[tauri::command]
pub async fn edit_chat_message(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    message_id: String,
    text: String,
) -> Result<(), String> {
    let body: String = text.trim().chars().take(4000).collect();
    if body.is_empty() {
        return Err("Message is empty.".into());
    }
    if let Some(u) = chat::apply_edit(&state.config_dir, &friend_id, &message_id, &body, true) {
        let _ = app.emit("chat://message", &u);
    }
    if let Some(friend) = friends::get(&state.config_dir, &friend_id) {
        if let Some(eid) = friend.endpoint_id {
            let my_name = state.settings.lock().unwrap().display_name.clone();
            let payload = serde_json::json!({
                "kind": "chat", "v": 2, "msgKind": "edit", "friendId": friend_id,
                "fromName": my_name, "targetId": message_id, "text": body,
            });
            send_chat_op(&iroh, eid, payload);
        }
    }
    Ok(())
}

/// Unsend a message we sent. Applies locally (tombstone), then tells the peer.
#[tauri::command]
pub async fn delete_chat_message(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    message_id: String,
) -> Result<(), String> {
    if let Some(u) = chat::apply_delete(&state.config_dir, &friend_id, &message_id, true) {
        let _ = app.emit("chat://message", &u);
    }
    if let Some(friend) = friends::get(&state.config_dir, &friend_id) {
        if let Some(eid) = friend.endpoint_id {
            let my_name = state.settings.lock().unwrap().display_name.clone();
            let payload = serde_json::json!({
                "kind": "chat", "v": 2, "msgKind": "delete", "friendId": friend_id,
                "fromName": my_name, "targetId": message_id,
            });
            send_chat_op(&iroh, eid, payload);
        }
    }
    Ok(())
}

/// Tell a friend whether we're currently typing to them (ephemeral; not stored).
#[tauri::command]
pub async fn send_typing(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    friend_id: String,
    on: bool,
) -> Result<(), String> {
    if let Some(friend) = friends::get(&state.config_dir, &friend_id) {
        if let (Some(ep), Some(eid)) = (iroh.get().cloned(), friend.endpoint_id) {
            let my_name = state.settings.lock().unwrap().display_name.clone();
            let payload = serde_json::json!({
                "kind": "chat-signal", "signal": "typing", "friendId": friend_id,
                "fromName": my_name, "on": on,
            });
            tauri::async_runtime::spawn(async move {
                let _ = crate::iroh_net::send_chat(&ep, &eid, payload).await;
            });
        }
    }
    Ok(())
}

/// Send a read receipt: tell a friend we've seen everything up to `up_to` (ms).
/// Honors the privacy toggle — if read receipts are off, sends nothing.
#[tauri::command]
pub async fn send_read_receipt(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    friend_id: String,
    up_to: u64,
) -> Result<(), String> {
    if !state.settings.lock().unwrap().send_read_receipts {
        return Ok(());
    }
    if let Some(friend) = friends::get(&state.config_dir, &friend_id) {
        if let (Some(ep), Some(eid)) = (iroh.get().cloned(), friend.endpoint_id) {
            let my_name = state.settings.lock().unwrap().display_name.clone();
            let payload = serde_json::json!({
                "kind": "chat-signal", "signal": "read", "friendId": friend_id,
                "fromName": my_name, "upTo": up_to,
            });
            tauri::async_runtime::spawn(async move {
                let _ = crate::iroh_net::send_chat(&ep, &eid, payload).await;
            });
        }
    }
    Ok(())
}

/// Download a GIF's bytes (from Giphy's CDN) into a temp file so it can be sent
/// over the normal P2P file path. Returns the local path. Fetched in Rust to
/// avoid webview CORS/CSP and keep the key/usage server-agnostic.
#[tauri::command]
pub async fn download_gif(app: AppHandle, url: String, id: String) -> Result<String, String> {
    // Only ever fetch from a real giphy.com host over https (parse the host, don't
    // substring-match — `https://media.evil.com/?giphy.com` must NOT pass).
    let host_ok = url
        .strip_prefix("https://")
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .map(|hostport| hostport.split(':').next().unwrap_or(""))
        .map(|host| host == "giphy.com" || host.ends_with(".giphy.com"))
        .unwrap_or(false);
    if !host_ok {
        return Err("Unexpected GIF URL.".into());
    }
    let bytes = reqwest::get(&url)
        .await
        .map_err(|e| format!("fetch gif: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("read gif: {e}"))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("GIF too large.".into());
    }
    let dir = app
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("gifs");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(40).collect();
    let path = dir.join(format!("giphy-{}.gif", if safe.is_empty() { "x" } else { &safe }));
    std::fs::write(&path, &bytes).map_err(|e| format!("save gif: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Send a GIF as a chat message: it rides the normal friend file transfer (the
/// caller already kicked that off), and this drops a GIF card in the thread on
/// both sides with the Giphy metadata for rich rendering.
#[tauri::command]
pub async fn send_chat_gif(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    app: AppHandle,
    friend_id: String,
    name: String,
    bytes: u64,
    path: String,
    gif: chat::GifMeta,
) -> Result<ChatMessage, String> {
    let friend = friends::get(&state.config_dir, &friend_id).ok_or("Friend not found.")?;
    let msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        peer_id: friend_id.clone(),
        from_me: true,
        kind: "file".into(),
        text: String::new(),
        files: vec![name],
        bytes,
        path: Some(path),
        status: Some("sending".into()),
        ts: chat::now_ms(),
        seq: chat::next_seq(&state.config_dir, &friend_id),
        reply_to: None,
        reply_preview: None,
        reactions: vec![],
        edited: false,
        deleted: false,
        gif: Some(gif),
    };
    chat::append(&state.config_dir, &msg);
    let _ = app.emit("chat://message", &msg);
    deliver_chat(&state, &iroh, &app, &friend, &msg);
    Ok(msg)
}

/// The frontend tells us which conversation is open in the main window (or None).
/// Used to suppress a notification for a chat the user is actively looking at.
#[tauri::command]
pub fn set_active_chat(state: State<'_, Arc<AppState>>, peer_id: Option<String>) {
    if let Ok(mut a) = state.active_chat.lock() {
        *a = peer_id;
    }
}

/// Reflect the total unread chat count on the Dock (macOS) / taskbar (Windows).
#[tauri::command]
pub fn set_unread_badge(app: AppHandle, count: u32) {
    // macOS Dock badge (the window owns the badge API in this Tauri version).
    // Windows has no numeric badge; we leave its taskbar alone.
    #[cfg(target_os = "macos")]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if count == 0 { None } else { Some(count as i64) });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, count);
    }
}

// ---------------------------------------------------------------------------
// Friends: permanent personal code + add-by-code (no re-pairing on update)
// ---------------------------------------------------------------------------

/// Your permanent, reusable DropBeam code. Share it once (text or QR) and anyone
/// can add you and reach you forever — it carries your stable device key + name.
#[tauri::command]
pub fn my_invite_code(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
) -> Result<String, String> {
    let my_name = state.settings.lock().unwrap().display_name.clone();
    let eid = iroh
        .get()
        .map(|ep| ep.id().to_string())
        .ok_or("Direct mode is still starting up — try again in a moment.")?;
    Ok(friends::my_code(&my_name, &eid))
}

/// Add a friend from their permanent code (dedup by device key, name auto-filled),
/// then introduce ourselves back so the friendship is two-way from one share.
#[tauri::command]
pub fn add_friend_by_code(
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
    sync: State<'_, Arc<SyncManager>>,
    app: AppHandle,
    code: String,
) -> Result<Friend, String> {
    let friend = friends::add_by_code(&state.config_dir, &code)?;
    // Reverse direction: tell them who we are so they add us too.
    if let Some(eid) = friend.endpoint_id.clone() {
        let my_name = state.settings.lock().unwrap().display_name.clone();
        crate::iroh_net::say_hello_to_endpoint(iroh.inner().clone(), eid, my_name);
    }
    sync.reconcile_friends();
    let _ = app.emit("friends://changed", ());
    Ok(friend)
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
pub fn forget_folder_item(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    pair_id: String,
    item_id: String,
) {
    if let Some(folder) = folder_for(state.inner(), &pair_id) {
        folder_history::forget(&folder, &item_id);
        let _ = app.emit("folder-history://changed", &pair_id);
    }
}

/// All mirror folders with recovery history, deduped by folder path (a group
/// folder is reached by several pair links but is one archive on disk). Returns
/// (representative pair id, folder path) per unique folder.
fn mirror_folders(state: &Arc<AppState>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for p in pairing::load(&state.config_dir) {
        if !p.mirror {
            continue;
        }
        if out.iter().any(|(_, f)| f == &p.folder) {
            continue;
        }
        out.push((p.id, p.folder));
    }
    out
}

fn folder_display_name(folder: &str) -> String {
    std::path::Path::new(folder)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| folder.to_string())
}

/// Per-folder rollup of recovery-history disk usage, for the storage view.
#[tauri::command]
pub fn folder_history_summary(
    state: State<'_, Arc<AppState>>,
) -> Vec<crate::models::FolderHistorySummary> {
    mirror_folders(state.inner())
        .into_iter()
        .map(|(pair_id, folder)| {
            let items = folder_history::load(&folder);
            let bytes = folder_history::folder_size(&folder);
            let oldest_ms = items.iter().map(|i| i.timestamp_ms).min();
            crate::models::FolderHistorySummary {
                pair_id,
                folder_name: folder_display_name(&folder),
                folder: folder.clone(),
                bytes,
                item_count: items.len() as u64,
                oldest_ms,
            }
        })
        .filter(|s| s.item_count > 0)
        .collect()
}

/// Wipe one folder's recovery history (every saved copy). Returns bytes freed.
#[tauri::command]
pub fn clear_folder_history(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    pair_id: String,
) -> u64 {
    let freed = match folder_for(state.inner(), &pair_id) {
        Some(folder) => folder_history::clear_all(&folder),
        None => 0,
    };
    let _ = app.emit("folder-history://changed", &pair_id);
    freed
}

/// Wipe recovery history across ALL mirror folders. Returns total bytes freed.
#[tauri::command]
pub fn clear_all_folder_history(app: AppHandle, state: State<'_, Arc<AppState>>) -> u64 {
    let mut freed = 0u64;
    for (pair_id, folder) in mirror_folders(state.inner()) {
        freed += folder_history::clear_all(&folder);
        let _ = app.emit("folder-history://changed", &pair_id);
    }
    freed
}

/// Apply current retention to every mirror folder (used at startup + when the
/// user changes retention). Best-effort; emits a refresh so the UI updates.
fn sweep_folder_history(app: &AppHandle, state: &Arc<AppState>) {
    let folders: Vec<String> = mirror_folders(state).into_iter().map(|(_, f)| f).collect();
    if folders.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        folder_history::sweep_all(&folders);
        let _ = app.emit("folder-history://changed", "");
    });
}

/// Send files straight to a friend — no code, no QR. Their inbox listener is
/// already waiting on the derived channel, so it just arrives.
#[tauri::command]
pub fn send_to_friend(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    iroh: State<'_, Arc<crate::iroh_net::IrohState>>,
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
    // iroh-only: dial the friend's endpoint directly (discovery resolves their
    // address). A friend added before Direct mode has no endpoint id and needs a
    // quick re-pair to be reachable.
    let eid = friend.endpoint_id.clone().ok_or(
        "This friend was added before Direct mode — re-pair with them to send directly.",
    )?;
    if iroh.get().is_none() {
        return Err("Direct mode is still starting up — try again in a moment.".into());
    }
    crate::iroh_net::send_to_friend(app, iroh.inner().clone(), friend.name, eid, paths)
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
