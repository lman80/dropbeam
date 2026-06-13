mod chat;
mod commands;
mod download_progress;
mod folder_history;
mod friends;
mod history;
mod iroh_net;
#[cfg(target_os = "macos")]
mod mac_service;
mod models;
mod pairing;
mod provenance;
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

/// Stash a file path for the UI to pick up as a pending send. Used by the macOS
/// "Share with DropBeam" Services handler (the app is already running there).
#[cfg(target_os = "macos")]
pub(crate) fn set_launch_file(path: String) {
    *LAUNCH_FILE.lock().unwrap() = Some(path);
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
    /// `None` = declined; `Some(dir)` = accepted (empty string = default download
    /// folder, otherwise save into `dir` — the receive card's "Save to" picker).
    pub offers: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Option<String>>>>,
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
            // Verbose diagnostics (Settings → "Detailed logging"): when on, capture
            // DEBUG-level app breadcrumbs PLUS iroh's connection internals so we can
            // diagnose hard-to-reproduce transfer issues from a tester's machine.
            // Read straight from settings.json — the plugin's level is fixed at
            // startup, so the toggle takes effect on the next launch.
            let verbose = app
                .path()
                .app_config_dir()
                .ok()
                .map(|d| crate::settings::load(&d, "", ""))
                .map(|s| s.verbose_logging)
                .unwrap_or(false);
            let (global_level, app_level, iroh_level) = if verbose {
                (
                    log::LevelFilter::Info,
                    log::LevelFilter::Debug,
                    log::LevelFilter::Debug,
                )
            } else {
                (
                    log::LevelFilter::Warn,
                    log::LevelFilter::Info,
                    log::LevelFilter::Warn,
                )
            };
            let _ = app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(global_level)
                    .level_for("app_lib", app_level)
                    // iroh's transport internals: hole-punch, relay-vs-direct path
                    // selection, connection lifecycle. Noisy, so only in verbose.
                    .level_for("iroh", iroh_level)
                    .level_for("iroh_relay", iroh_level)
                    .level_for("iroh_net", iroh_level)
                    // The default cap is a tiny 40 KB with discard-on-rotate, which
                    // kept deleting exactly the history we need when diagnosing a
                    // user-reported transfer. Keep rotated files, 2 MB each (4 MB in
                    // verbose, since DEBUG fills them faster and we want the history).
                    .max_file_size(if verbose { 4_000_000 } else { 2_000_000 })
                    .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll)
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                            file_name: Some("DropBeam".into()),
                        }),
                    ])
                    .build(),
            );
            // Stamp every log file so it's self-identifying when exported for support.
            log::warn!(
                "DropBeam {} starting on {} {} — verbose diagnostics: {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                if verbose { "ON" } else { "off" }
            );
            // KeepAll preserves diagnostic history but never deletes anything — an
            // always-running menu-bar app would grow ~/Library/Logs without bound.
            // Sweep rotated files beyond the newest 5 (~10 MB of history) at start.
            if let Ok(log_dir) = app.path().app_log_dir() {
                if let Ok(rd) = std::fs::read_dir(&log_dir) {
                    let mut rotated: Vec<(std::time::SystemTime, PathBuf)> = rd
                        .flatten()
                        .filter(|e| {
                            let n = e.file_name().to_string_lossy().to_string();
                            n.starts_with("DropBeam") && n.ends_with(".log") && n != "DropBeam.log"
                        })
                        .filter_map(|e| {
                            e.metadata()
                                .and_then(|m| m.modified())
                                .ok()
                                .map(|t| (t, e.path()))
                        })
                        .collect();
                    rotated.sort_by(|a, b| b.0.cmp(&a.0));
                    for (_, p) in rotated.into_iter().skip(5) {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
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
            let mut loaded = settings::load(&config_dir, &default_download, &default_name);
            // One-time "always ready in the background" migration: existing installs
            // had launch-at-login OFF, so a closed app couldn't receive. Turn it ON
            // once (the app then starts silently in the menu bar at every login).
            // A marker FILE — not a settings field — records that we did, so it
            // can't be reset by the frontend's full-object settings save, and so we
            // never override a later manual choice to turn it off.
            let bg_marker = config_dir.join(".bg-ready-migrated");
            if !bg_marker.exists() {
                loaded.launch_at_login = true;
                let _ = settings::save(&config_dir, &loaded);
                let _ = std::fs::write(&bg_marker, b"1");
            }
            let want_autostart = loaded.launch_at_login;
            // Honor the saved internet upload cap from first launch.
            iroh_net::set_upload_limit_mbps(loaded.upload_limit_mbps);
            iroh_net::set_require_direct(loaded.require_direct);
            // Collapse any duplicate friend records (the same person added via more
            // than one path — folder pairing, permanent code, classic invite) into
            // one canonical entry, migrating chat history onto the survivor. Runs
            // every launch so a friendship can never duplicate or get lost across
            // updates.
            let removed = friends::reconcile(&config_dir);
            if removed > 0 {
                log::info!("startup: reconciled {removed} duplicate friend record(s)");
            }

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
            // Keep retrying undelivered chat messages until they land (reliable chat).
            iroh_net::spawn_chat_outbox_retry(app.handle().clone(), iroh_state.clone());
            // Once iroh has had a moment to bind, re-introduce ourselves to every
            // friend so any profile (name/picture) change made while we were offline
            // propagates to friends who are online now.
            {
                let app2 = app.handle().clone();
                let iroh2 = iroh_state.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    iroh_net::broadcast_profile(app2, iroh2);
                });
            }
            app.manage(iroh_state);
            log::info!("setup: iroh spawned + state managed");

            // Start the Shared Drop Folder sync service.
            let sync = sync::SyncManager::new(app.handle().clone(), config_dir);
            sync.reconcile();
            app.manage(sync);
            log::info!("setup: sync reconciled");

            build_tray(app.handle())?;
            log::info!("setup: tray built");

            // Register (or clear) the login item so DropBeam is ready to receive
            // after every restart without being opened. Re-applied each launch so
            // it survives app updates.
            commands::apply_autostart(app.handle(), want_autostart);

            // Present the window — UNLESS we were auto-started at login (the
            // autostart plugin passes `--minimized`), in which case we stay silent
            // in the menu bar, ready to receive. The main window is created hidden
            // (visible:false in the config) so a login launch never flashes it.
            // The app is born menu-bar-only (LSUIElement in Info.plist on macOS),
            // so a login launch never flashes a Dock icon or the window. On a
            // NORMAL launch, become a regular app FIRST (so the window comes to the
            // front), then show it. On an autostart launch (`--minimized` from the
            // login item) stay quietly in the menu bar, ready to receive.
            let autostart_launch = std::env::args().any(|a| a == "--minimized");
            if autostart_launch {
                log::info!("setup: launched at login — staying in the menu bar");
            } else {
                set_dock_icon_visible(app.handle(), true);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }

            // Native macOS: let a file dragged over the menu-bar icon spring the
            // popover open (Blip-style). The status item appears a beat after the
            // tray is built, so retry on the main thread until the button exists.
            // Best-effort — if it never attaches, the tray still works normally.
            #[cfg(target_os = "macos")]
            {
                // Finder right-click → Services → "Share with DropBeam". setup()
                // runs on the main thread, which is where AppKit wants the
                // provider registered. Best-effort.
                mac_service::install(app.handle());

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
                    // Back to a menu-bar-only app (no Dock icon) while the window
                    // is tucked away — but still running and receiving. Keep the
                    // Dock icon if a transfer card is on screen (it needs one to
                    // minimize into); the card drops it when it's done.
                    let card_up = app
                        .get_webview_window("receive")
                        .and_then(|w| w.is_visible().ok())
                        .unwrap_or(false);
                    if !card_up {
                        set_dock_icon_visible(&app, false);
                    }
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
            commands::set_profile_avatar,
            commands::clear_profile_avatar,
            commands::reveal_path,
            commands::open_path,
            commands::export_diagnostics,
            commands::restart_app,
            commands::lan_network_blocked,
            commands::open_local_network_settings,
            commands::open_url,
            commands::open_main_window,
            commands::hide_popover,
            commands::get_default_download_dir,
            commands::create_pair,
            commands::accept_pair,
            commands::list_pairs,
            commands::update_pair,
            commands::remove_pair,
            commands::verify_folders,
            commands::stop_folder_transfer,
            commands::clear_transfer_cache,
            commands::set_card_active,
            commands::quit_app,
            commands::pair_invite,
            commands::folder_add_person,
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
            commands::my_invite_code,
            commands::add_friend_by_code,
            commands::macos_install_hint,
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
                set_dock_icon_visible(_app, true);
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
    // Become a normal app (Dock icon + proper focus) while the window is open.
    set_dock_icon_visible(app, true);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// macOS: show or hide the Dock icon by switching the activation policy.
/// DropBeam lives in the menu bar with no Dock icon while idle, and becomes a
/// regular app (Dock icon + Cmd-Tab + reliable focus) whenever its main window
/// is open — the standard menu-bar-app pattern. No-op off macOS.
#[cfg(target_os = "macos")]
pub(crate) fn set_dock_icon_visible(app: &AppHandle, visible: bool) {
    let handle = app.clone();
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.run_on_main_thread(move || {
        let _ = handle.set_activation_policy(policy);
    });
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn set_dock_icon_visible(_app: &AppHandle, _visible: bool) {}

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
