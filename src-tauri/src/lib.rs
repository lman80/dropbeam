mod chat;
mod commands;
mod folder_history;
mod friends;
mod history;
mod iroh_net;
mod models;
mod pairing;
mod settings;
mod sync;
#[cfg(target_os = "macos")]
mod tray_drag;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// When the popover last hid itself on blur — so a tray click that *caused* that
/// blur doesn't immediately re-open it (clicking the icon should toggle).
static LAST_POPOVER_HIDE: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// A file the app was launched to send (Windows "Send with DropBeam" right-click,
/// or a second launch forwarded by single-instance). The UI drains it on load.
static LAUNCH_FILE: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
use tauri_plugin_autostart::MacosLauncher;
use tokio::sync::Notify;

use models::Settings;

/// Diagnostic: let the popover JS write a line into the Rust log file so we can
/// trace the native drag → drop → send handoff. Temporary debugging aid.
#[tauri::command]
fn traydrag_debug(msg: String) {
    log::info!("[traydrag-js] {msg}");
}

/// Let the frontend write a line into the native app log file — so we can
/// diagnose startup/runtime issues on machines we can't access (e.g. a tester's
/// Windows box: the log lands in %APPDATA%\com.dropbeam.app\logs\DropBeam.log).
#[tauri::command]
fn frontend_log(msg: String) {
    log::info!("[ui] {msg}");
}

/// First argument (after the executable) that points to an existing file — the
/// path the "Send with DropBeam" right-click menu passes (`DropBeam.exe "%1"`).
fn file_from_args(argv: &[String]) -> Option<String> {
    argv.iter()
        .skip(1)
        .find(|a| std::path::Path::new(a.as_str()).is_file())
        .cloned()
}

/// The UI calls this on launch to pick up a file the app was opened to send
/// (cold start via the Windows right-click menu). Returns it and clears it.
#[tauri::command]
fn take_launch_file() -> Option<String> {
    LAUNCH_FILE.lock().unwrap().take()
}

/// One friend row's on-screen rectangle (CSS px), reported by the menu JS.
#[derive(serde::Deserialize)]
struct JsRowRect {
    id: String,
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

/// The menu reports its friend rows here (webview → Rust, works even when the
/// menu is inactive) so the native drag-drop handler can map a drop to a person
/// and send entirely in Rust.
#[tauri::command]
fn set_popover_rows(rows: Vec<JsRowRect>) {
    #[cfg(target_os = "macos")]
    {
        let rows = rows
            .into_iter()
            .map(|r| tray_drag::RowRect {
                id: r.id,
                top: r.top,
                bottom: r.bottom,
                left: r.left,
                right: r.right,
            })
            .collect();
        tray_drag::set_rows(rows);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = rows;
    }
}

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
    let builder = tauri::Builder::default();
    // Single-instance MUST be the first plugin. Windows/Linux only: it forwards a
    // second launch — e.g. the "Send with DropBeam" right-click menu, which runs
    // DropBeam.exe with the file path — to the already-running app instead of
    // opening a duplicate. (macOS already single-instances .app bundles and has
    // the menu-bar drag-to-send, so it's skipped there.)
    #[cfg(not(target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        if let Some(f) = file_from_args(&argv) {
            *LAUNCH_FILE.lock().unwrap() = Some(f.clone());
            let _ = tauri::Emitter::emit(app, "open-file-send", f);
        }
    }));
    let builder = builder
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    // macOS: panel plugin so the popover can float over full-screen apps.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(|app| {
            // Always-on logging to a FILE so we can diagnose issues on machines we
            // can't access (e.g. a tester's Windows box — the log lands in
            // %APPDATA%\com.dropbeam.app\logs\DropBeam.log, or ~/Library/Logs/
            // com.dropbeam.app/ on macOS). iroh's per-packet INFO spam stays at
            // Warn so our own [app_lib]/[ui] breadcrumbs survive.
            let _ = app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Warn)
                    .level_for("app_lib", log::LevelFilter::Info)
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                            file_name: Some("DropBeam".into()),
                        }),
                    ])
                    .build(),
            );
            // Log panics (including ones inside commands) so a crash on a remote
            // machine leaves a trace in the file instead of a silent hang.
            std::panic::set_hook(Box::new(|info| {
                log::error!("PANIC: {info}");
            }));
            log::info!("setup: starting v{}", env!("CARGO_PKG_VERSION"));

            // Cold start via the "Send with DropBeam" right-click menu: stash the
            // file path so the UI can open the send chooser for it once loaded.
            if let Some(f) = file_from_args(&std::env::args().collect::<Vec<_>>()) {
                *LAUNCH_FILE.lock().unwrap() = Some(f);
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

            // iroh is the ONLY transport now, so always bring up the direct P2P
            // endpoint in the background (it fills once bound). A user who'd
            // toggled the old "Direct mode" off must still get a working app.
            let iroh_state = Arc::new(iroh_net::IrohState::default());
            iroh_net::spawn(config_dir.clone(), iroh_state.clone(), app.handle().clone());
            app.manage(iroh_state);
            log::info!("setup: iroh spawned + state managed");

            // Start the Shared Drop Folder sync service.
            let sync = sync::SyncManager::new(app.handle().clone(), config_dir);
            sync.reconcile();
            app.manage(sync);
            log::info!("setup: sync reconciled");

            build_tray(app.handle())?;
            log::info!("setup: tray built");

            // Native macOS: let a file dragged over the menu-bar icon spring the
            // popover open (Blip-style). The status item appears a beat after the
            // tray is built, so retry on the main thread until the button exists.
            // Best-effort — if it never attaches, the tray still works normally.
            #[cfg(target_os = "macos")]
            {
                // Make the popover a non-activating panel so it floats over
                // full-screen apps and doesn't steal focus.
                tray_drag::convert_popover_to_panel(app.handle());

                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for delay_ms in [250u64, 500, 1000, 2000, 3500] {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        let h2 = h.clone();
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let posted = h.run_on_main_thread(move || {
                            let _ = tx.send(tray_drag::install(&h2));
                        });
                        if posted.is_ok() && rx.await.unwrap_or(false) {
                            log::info!("tray drag-to-open attached");
                            break;
                        }
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
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
            // The menu-bar popover dismisses itself when it loses focus.
            tauri::WindowEvent::Focused(false) if window.label() == "popover" => {
                *LAST_POPOVER_HIDE.lock().unwrap() = Some(Instant::now());
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::cancel_transfer,
            commands::get_settings,
            commands::update_settings,
            commands::get_history,
            commands::clear_history,
            commands::pick_files,
            commands::pick_directory,
            commands::reveal_path,
            commands::open_path,
            commands::open_main_window,
            commands::hide_popover,
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
            commands::ping_friend,
            commands::friend_invite,
            commands::send_to_friend,
            commands::iroh_node_id,
            commands::iroh_selftest,
            commands::iroh_send,
            commands::iroh_receive,
            commands::get_chat_messages,
            commands::list_chats,
            commands::send_chat_message,
            commands::send_chat_file_note,
            traydrag_debug,
            frontend_log,
            take_launch_file,
            set_popover_rows,
        ])
        .build(tauri::generate_context!())
        .expect("error while building DropBeam")
        .run(|_app, _event| {
            // macOS: clicking the Dock icon (or otherwise re-opening the app) when
            // the main window is hidden/minimized should bring it back. Without
            // this the window "disappears" after it's been hidden to the tray.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = &_event {
                if let Some(w) = _app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
        });
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
                position,
                ..
            } = event
            {
                toggle_popover(tray.app_handle(), position);
            }
        });

    // A dedicated monochrome beam glyph (template) for the menu bar — templating
    // the full app icon just yields a solid square silhouette.
    match tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")) {
        Ok(icon) => {
            builder = builder.icon(icon).icon_as_template(true);
        }
        Err(_) => {
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone()).icon_as_template(true);
            }
        }
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

/// Toggle the menu-bar popover, anchoring it just below the clicked tray icon.
fn toggle_popover(app: &AppHandle, cursor: tauri::PhysicalPosition<f64>) {
    let Some(w) = app.get_webview_window("popover") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        *LAST_POPOVER_HIDE.lock().unwrap() = Some(Instant::now());
        let _ = w.hide();
        return;
    }
    // If a blur from THIS same click just hid it, treat the click as a dismiss.
    let just_hid = LAST_POPOVER_HIDE
        .lock()
        .unwrap()
        .map(|t| t.elapsed() < Duration::from_millis(250))
        .unwrap_or(false);
    if just_hid {
        return;
    }
    // Cursor is physical; convert to logical so layout math matches window size.
    let scale = w.scale_factor().unwrap_or(1.0);
    let cx = cursor.x / scale;
    let cy = cursor.y / scale;
    let pop_w = 300.0;
    let x = (cx - pop_w / 2.0).max(8.0);
    let y = cy + 8.0; // just under the menu bar
    let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    #[cfg(target_os = "macos")]
    tray_drag::show_popover_key(app);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn quit_app(app: &AppHandle) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.force_quit.store(true, Ordering::SeqCst);
    }
    app.exit(0);
}
