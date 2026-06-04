mod commands;
mod croc;
mod folder_history;
mod friends;
mod history;
mod models;
mod pairing;
mod settings;
mod sync;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::MacosLauncher;
use tokio::sync::Notify;

use models::Settings;

/// Shared application state, managed by Tauri as `Arc<AppState>`.
pub struct AppState {
    /// ~/Library/Application Support/com.dropbeam.app (or platform equivalent).
    pub config_dir: PathBuf,
    pub settings: Mutex<Settings>,
    /// Cancellation handles for in-flight transfers, keyed by transfer id.
    pub transfers: Mutex<HashMap<String, Arc<Notify>>>,
    /// Pending manual-accept offers: a friend transfer awaiting the user's
    /// accept/decline, keyed by transfer id. `respond_to_offer` resolves these.
    pub offers: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// Set when the user really wants to quit (vs. close-to-tray).
    pub force_quit: AtomicBool,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let _ = std::fs::create_dir_all(&config_dir);

            let default_download = app
                .path()
                .download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let default_name = default_display_name();
            let loaded = settings::load(&config_dir, &default_download, &default_name);

            app.manage(Arc::new(AppState {
                config_dir: config_dir.clone(),
                settings: Mutex::new(loaded),
                transfers: Mutex::new(HashMap::new()),
                offers: Mutex::new(HashMap::new()),
                force_quit: AtomicBool::new(false),
            }));

            // Start the Shared Drop Folder sync service.
            let sync = sync::SyncManager::new(app.handle().clone(), config_dir);
            sync.reconcile();
            app.manage(sync);

            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let force = app
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.force_quit.load(Ordering::SeqCst))
                    .unwrap_or(false);
                let minimize = app
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.lock().unwrap().minimize_to_tray)
                    .unwrap_or(true);
                if !force && minimize {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_files,
            commands::receive_files,
            commands::cancel_transfer,
            commands::get_settings,
            commands::update_settings,
            commands::get_history,
            commands::clear_history,
            commands::pick_files,
            commands::pick_directory,
            commands::reveal_path,
            commands::open_path,
            commands::get_default_download_dir,
            commands::create_pair,
            commands::accept_pair,
            commands::list_pairs,
            commands::update_pair,
            commands::remove_pair,
            commands::pair_invite,
            commands::get_folder_statuses,
            commands::list_folder_history,
            commands::restore_folder_item,
            commands::forget_folder_item,
            commands::create_friend,
            commands::accept_friend,
            commands::list_friends,
            commands::rename_friend,
            commands::remove_friend,
            commands::set_friend_auto_accept,
            commands::respond_to_offer,
            commands::friend_invite,
            commands::send_to_friend,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DropBeam");
}

fn default_display_name() -> String {
    let name = whoami::devicename();
    if name.trim().is_empty() {
        "My Computer".to_string()
    } else {
        name
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let open_i = MenuItem::with_id(app, "tray_open", "Open DropBeam", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "tray_quit", "Quit DropBeam", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open_i, &sep, &quit_i])?;

    let mut builder = TrayIconBuilder::with_id("dropbeam-tray")
        .tooltip("DropBeam")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray_open" => show_main_window(app),
            "tray_quit" => quit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).icon_as_template(true);
    }

    builder.build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn quit_app(app: &AppHandle) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.force_quit.store(true, Ordering::SeqCst);
    }
    app.exit(0);
}
