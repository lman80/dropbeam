//! Tauri commands for the Transfer Server settings (owner side) and the
//! "Servers I can use" list (user side).

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{client, server};
use crate::iroh_net::IrohState;
use crate::AppState;

fn changed(app: &AppHandle, net: &Arc<IrohState>) {
    let _ = app.emit("mailbox://server", ());
    // Friends learn what changed (access, pause, name) from our next hello.
    crate::iroh_net::broadcast_profile(app.clone(), net.clone());
}

/// This device as a candidate server: can it host, does it stay awake, room.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCheck {
    pub supported: bool,
    pub os: String,
    /// The device is set to sleep on its own (items wait until it wakes).
    pub sleeps: Option<bool>,
    pub on_battery: Option<bool>,
    pub default_root: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub suggested_cap: u64,
}

fn sleeps() -> (Option<bool>, Option<bool>) {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("pmset").arg("-g").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let sleep = text.lines().find(|l| l.trim_start().starts_with("sleep ")).and_then(|l| {
            l.split_whitespace().nth(1)?.parse::<u32>().ok()
        });
        let batt = std::process::Command::new("pmset").args(["-g", "batt"]).output().ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("Battery Power"));
        (sleep.map(|m| m > 0), batt)
    }
    #[cfg(target_os = "linux")]
    {
        // GNOME's automatic suspend on AC ("nothing" = never).
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.settings-daemon.plugins.power", "sleep-inactive-ac-type"]).output().ok();
        let v = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_owned());
        let batt = std::fs::read_dir("/sys/class/power_supply").ok().map(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                std::fs::read_to_string(p.join("type")).map(|t| t.trim() == "Battery").unwrap_or(false)
                    && std::fs::read_to_string(p.join("status")).map(|t| t.trim() == "Discharging").unwrap_or(false)
            })
        });
        (v.filter(|s| !s.is_empty()).map(|s| s != "nothing"), batt)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        (None, None)
    }
}

/// min(100 GB, 50% of free), rounded down to a whole GB (at least 1 GB).
pub fn suggested_cap(free: Option<u64>) -> u64 {
    const GB: u64 = 1_000_000_000;
    let cap = free.map_or(100 * GB, |f| (100 * GB).min(f / 2));
    (cap / GB).max(1) * GB
}

#[tauri::command]
pub async fn server_check_device(state: State<'_, Arc<AppState>>) -> Result<DeviceCheck, String> {
    let config = state.config_dir.clone();
    tokio::task::spawn_blocking(move || {
        let root = server::default_root(&config);
        let probe = root.parent().map(Path::to_path_buf).unwrap_or(config.clone());
        let (free, total) = crate::locations::volume_bytes(&probe).map_or((None, None), |(f, t)| (Some(f), Some(t)));
        let (sleeps, on_battery) = sleeps();
        DeviceCheck {
            supported: server::hosting_supported(),
            os: std::env::consts::OS.into(),
            sleeps,
            on_battery,
            default_root: root.to_string_lossy().into_owned(),
            free_bytes: free,
            total_bytes: total,
            suggested_cap: suggested_cap(free),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Free space where `path` lives (for the wizard's size slider).
#[tauri::command]
pub async fn server_folder_space(path: String) -> Option<(u64, u64)> {
    tokio::task::spawn_blocking(move || crate::locations::volume_bytes(Path::new(&path))).await.ok().flatten()
}

#[tauri::command]
pub async fn server_status(state: State<'_, Arc<AppState>>) -> Result<server::ServerStatus, String> {
    let config = state.config_dir.clone();
    tokio::task::spawn_blocking(move || server::status(&config)).await.map_err(|e| e.to_string())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerPatch {
    pub name: Option<String>,
    pub root: Option<String>,
    pub cap_bytes: Option<u64>,
    pub file_days: Option<u32>,
    pub chat_days: Option<u32>,
    pub item_max: Option<u64>,
    pub access: Option<String>,
    pub allowed: Option<Vec<String>>,
    pub through: Option<Vec<String>>,
    pub paused: Option<bool>,
    pub enabled: Option<bool>,
}

fn apply_patch(config: &Path, c: &mut server::ServerConfig, p: ServerPatch) -> Result<(), String> {
    if let Some(n) = p.name {
        let n: String = n.trim().chars().take(40).collect();
        if n.is_empty() {
            return Err("Give it a name your friends will recognize.".into());
        }
        c.name = n;
    }
    if let Some(a) = p.access {
        if !matches!(a.as_str(), "me" | "chosen" | "all") {
            return Err("Unknown access setting.".into());
        }
        c.access = a;
    }
    if let Some(v) = p.allowed {
        c.allowed = v;
    }
    if let Some(v) = p.through {
        c.through = v;
    }
    if let Some(d) = p.file_days {
        c.file_days = d.clamp(1, 90);
    }
    if let Some(d) = p.chat_days {
        c.chat_days = d.clamp(1, 90);
    }
    if let Some(m) = p.item_max {
        c.item_max = m.max(1_000_000);
    }
    if let Some(cap) = p.cap_bytes {
        c.cap_bytes = cap.max(100_000_000);
    }
    if let Some(v) = p.paused {
        c.paused = v;
    }
    if let Some(root) = p.root {
        let root = root.trim().to_owned();
        let default = server::default_root(config).to_string_lossy().into_owned();
        let root = if root == default { String::new() } else { root };
        if root != c.root {
            c.root = root;
            c.marker = String::new();
            server::unload(config);
        }
    }
    if let Some(e) = p.enabled {
        c.enabled = e;
    }
    Ok(())
}

/// Turn this device into a Transfer Server (or update its settings).
#[tauri::command]
pub async fn server_configure(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, patch: ServerPatch) -> Result<server::ServerStatus, String> {
    if !server::hosting_supported() {
        return Err("A Transfer Server needs a computer that stays on.".into());
    }
    let config = state.config_dir.clone();
    let default_name = state.settings.lock().unwrap().display_name.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<server::ServerStatus, String> {
        let mut c = server::load_config(&config);
        let first = !c.enabled && c.created_ms == 0;
        apply_patch(&config, &mut c, patch)?;
        if c.enabled {
            if c.name.trim().is_empty() {
                c.name = if default_name.trim().is_empty() { "Transfer Server".into() } else { default_name };
            }
            if first {
                c.created_ms = crate::chat::now_ms();
            }
            if server::root(&config, &c).is_err() {
                server::init_root(&config, &mut c).map_err(|e| {
                    let msg = format!("{e:#}");
                    if msg.contains("Unsafe") || msg.contains("overlap") || msg.contains("Mirror") {
                        "That folder can't be used — pick an empty folder or a drive.".to_owned()
                    } else {
                        "Couldn't use that folder. Check that it exists and you can write to it.".to_owned()
                    }
                })?;
            }
            if c.cap_bytes == 0 {
                let free = crate::locations::volume_bytes(&server::root(&config, &c).map_err(|e| e.to_string())?).map(|(f, _)| f);
                c.cap_bytes = suggested_cap(free);
            }
        }
        server::save_config(&config, &c).map_err(|e| e.to_string())?;
        Ok(server::status(&config))
    })
    .await
    .map_err(|e| e.to_string())??;
    changed(&app, net.inner());
    server::wake_delivery();
    Ok(status)
}

/// Remove a person from this server: their deposits go (except those for this
/// account's own devices, which still deliver), and they can't add more.
#[tauri::command]
pub async fn server_remove_person(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, person_id: String) -> Result<server::ServerStatus, String> {
    let config = state.config_dir.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<server::ServerStatus, String> {
        let mut c = server::load_config(&config);
        if !c.denied.contains(&person_id) {
            c.denied.push(person_id.clone());
        }
        c.allowed.retain(|p| p != &person_id);
        c.through.retain(|p| p != &person_id);
        server::save_config(&config, &c).map_err(|e| e.to_string())?;
        server::remove_person(&config, &person_id);
        Ok(server::status(&config))
    })
    .await
    .map_err(|e| e.to_string())??;
    changed(&app, net.inner());
    Ok(status)
}

/// Let a removed person use the server again.
#[tauri::command]
pub async fn server_restore_person(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, person_id: String) -> Result<server::ServerStatus, String> {
    let config = state.config_dir.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<server::ServerStatus, String> {
        let mut c = server::load_config(&config);
        c.denied.retain(|p| p != &person_id);
        if c.access == "chosen" && !c.allowed.contains(&person_id) {
            c.allowed.push(person_id);
        }
        server::save_config(&config, &c).map_err(|e| e.to_string())?;
        Ok(server::status(&config))
    })
    .await
    .map_err(|e| e.to_string())??;
    changed(&app, net.inner());
    Ok(status)
}

/// Delete everything this server holds.
#[tauri::command]
pub async fn server_wipe(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<server::ServerStatus, String> {
    let config = state.config_dir.clone();
    let status = tokio::task::spawn_blocking(move || {
        server::wipe(&config);
        server::status(&config)
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("mailbox://server", ());
    Ok(status)
}

/// Stop being a Transfer Server. With `delete_items`, everything held is removed
/// first (senders' apps will show those as not delivered).
#[tauri::command]
pub async fn server_disable(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, delete_items: bool) -> Result<server::ServerStatus, String> {
    let config = state.config_dir.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<server::ServerStatus, String> {
        if delete_items {
            server::wipe(&config);
        }
        let mut c = server::load_config(&config);
        c.enabled = false;
        server::save_config(&config, &c).map_err(|e| e.to_string())?;
        server::unload(&config);
        Ok(server::status(&config))
    })
    .await
    .map_err(|e| e.to_string())??;
    changed(&app, net.inner());
    Ok(status)
}

/// Servers this device may use (own + shared by friends).
#[tauri::command]
pub fn mailbox_servers(state: State<'_, Arc<AppState>>) -> Vec<client::UsableServer> {
    client::servers(&state.config_dir)
}

/// The user's choice about a server: use it for sending, hold my messages
/// there, or dismiss the offer.
#[tauri::command]
pub fn mailbox_server_prefs(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>,
    eid: String, use_it: Option<bool>, hold_for_me: Option<bool>, offer: Option<String>) -> Result<Vec<client::UsableServer>, String> {
    let before = client::my_inbox(&state.config_dir);
    client::set_prefs(&state.config_dir, &eid, use_it, hold_for_me, offer.as_deref())?;
    let _ = app.emit("mailbox://servers", ());
    if client::my_inbox(&state.config_dir) != before {
        // Friends learn where to leave things for us.
        crate::iroh_net::broadcast_profile(app.clone(), net.inner().clone());
    }
    client::wake();
    Ok(client::servers(&state.config_dir))
}

/// Stop using a server we were offered.
#[tauri::command]
pub fn mailbox_forget_server(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, eid: String) -> Vec<client::UsableServer> {
    client::forget(&state.config_dir, &eid);
    let _ = app.emit("mailbox://servers", ());
    crate::iroh_net::broadcast_profile(app.clone(), net.inner().clone());
    client::servers(&state.config_dir)
}

/// Pull anything held for us right now (app foregrounded / pulled to refresh).
#[tauri::command]
pub fn mailbox_fetch_now() {
    client::fetch_soon();
}

/// Where a message to this friend would be held if they're offline (the
/// server's name), or None when no Transfer Server can take it.
#[tauri::command]
pub fn mailbox_hold_route(state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, friend_id: String) -> Option<String> {
    let ep = net.get()?;
    client::hold_route(&state.config_dir, &ep.id().to_string(), &friend_id)
}

/// Held file sends from "ask before accepting" friends, waiting for a yes/no.
#[tauri::command]
pub fn mailbox_pending_files(state: State<'_, Arc<AppState>>) -> Vec<client::PendingFile> {
    client::pending_files(&state.config_dir)
}

/// Accept (download) or decline a held file send.
#[tauri::command]
pub fn mailbox_decide_file(app: AppHandle, state: State<'_, Arc<AppState>>, link_id: String, accept: bool) {
    client::decide_file(&state.config_dir, &link_id, accept);
    let _ = app.emit("mailbox://pending", ());
}

/// iOS: the app got its APNs token + notification-extension key.
#[tauri::command]
pub fn push_set_device(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, token: String, env: String, push_key: String) -> Result<(), String> {
    let changed = super::push::set_device(&state.config_dir, &token, &env, &push_key).map_err(|e| e.to_string())?;
    if changed {
        // Friends need our push key; servers need the new token.
        crate::iroh_net::broadcast_profile(app, net.inner().clone());
        client::wake();
    }
    Ok(())
}

/// "Show message text in notifications".
#[tauri::command]
pub fn push_set_previews(app: AppHandle, state: State<'_, Arc<AppState>>, net: State<'_, Arc<IrohState>>, on: bool) {
    super::push::set_previews(&state.config_dir, on);
    crate::iroh_net::broadcast_profile(app, net.inner().clone());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushStatus {
    /// This device has an APNs token (push works on this build).
    pub enabled: bool,
    pub previews: bool,
    /// Servers holding our messages that can wake this phone.
    pub servers: usize,
}

#[tauri::command]
pub fn push_status(state: State<'_, Arc<AppState>>) -> PushStatus {
    let d = super::push::device(&state.config_dir);
    PushStatus {
        enabled: d.as_ref().is_some_and(|d| !d.token.is_empty()),
        previews: d.as_ref().is_none_or(|d| d.previews),
        servers: d.map_or(0, |d| d.registered.len()),
    }
}
