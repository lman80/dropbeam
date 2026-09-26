//! `DropBeam --server`: run only the engine — Transfer Server hosting, delivery,
//! expiry — with no window, for an always-on box that may sit at a login screen
//! (docs/TRANSFER-SERVER-PLAN.md, phase 3). Shipped as a systemd unit in the .deb
//! (`dropbeam-server@.service`), off until the owner enables it.
//!
//! One identity must never run twice (two endpoints with the same key fight
//! over every connection), so the GUI and the service share an exclusive lock
//! (`engine.lock`). The service waits for it; the GUI, when it starts, asks a
//! running service to step aside and takes over — and when the GUI quits, the
//! (restarted) service picks the lock back up.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const IDENTIFIER: &str = "com.dropbeam.app";

/// The desktop app's config folder (same as tauri's app_config_dir).
pub fn default_config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config"));
    base.join(IDENTIFIER)
}

/// An exclusive hold on this identity's engine. Released when dropped / on exit.
pub struct EngineLock {
    #[allow(dead_code)]
    file: std::fs::File,
}

#[cfg(unix)]
fn try_flock(file: &std::fs::File) -> bool {
    use std::os::unix::io::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

fn lock_path(config: &Path) -> PathBuf {
    config.join("engine.lock")
}

/// Try once. `mode` ("app" | "server") + pid are recorded for the other side.
pub fn try_lock(config: &Path, mode: &str) -> Option<EngineLock> {
    #[cfg(unix)]
    {
        let _ = std::fs::create_dir_all(config);
        let mut file = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false)
            .open(lock_path(config)).ok()?;
        if !try_flock(&file) {
            return None;
        }
        let _ = file.set_len(0);
        let _ = write!(file, "{} {}", std::process::id(), mode);
        let _ = file.flush();
        Some(EngineLock { file })
    }
    #[cfg(not(unix))]
    {
        let _ = (config, mode);
        None
    }
}

/// Who holds the lock right now (pid, mode), if anyone wrote it.
fn holder(config: &Path) -> Option<(i32, String)> {
    let text = std::fs::read_to_string(lock_path(config)).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.to_owned()))
}

/// The GUI's side: take the engine, asking a background service to stop
/// first. Never blocks the app for long — after ~10s it proceeds regardless.
pub fn lock_for_app(config: &Path) -> Option<EngineLock> {
    if !cfg!(unix) {
        return None;
    }
    if let Some(l) = try_lock(config, "app") {
        return Some(l);
    }
    // Never run the same identity twice: keep asking the background service to
    // stop until it has (it always does; systemd's restart then waits on us).
    let mut tries = 0u32;
    loop {
        #[cfg(unix)]
        if tries % 20 == 0 {
            if let Some((pid, mode)) = holder(config) {
                if mode == "server" && pid > 1 {
                    log::info!("engine: the background Transfer Server is running; taking over for the app");
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
        if let Some(l) = try_lock(config, "app") {
            return Some(l);
        }
        tries += 1;
        if tries == 240 {
            log::warn!("engine: still waiting for the background Transfer Server to stop");
        }
    }
}

struct StderrLog;
impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info && (m.target().starts_with("app") || m.level() <= log::Level::Warn)
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}", r.level(), crate::telemetry::redact_paths_only(&r.args().to_string()));
        }
    }
    fn flush(&self) {}
}

/// Entry point for `DropBeam --server [--config DIR]`.
pub fn run(args: &[String]) {
    let _ = log::set_boxed_logger(Box::new(StderrLog)).map(|()| log::set_max_level(log::LevelFilter::Info));
    let config = args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)).map(PathBuf::from)
        .unwrap_or_else(default_config_dir);
    let c = crate::mailbox::server::load_config(&config);
    if !c.enabled {
        log::warn!("DropBeam --server: this device isn't set up as a Transfer Server yet (Settings → Server in the app).");
    }
    // Wait for the app to be closed (it owns the engine while it's open).
    let _lock = loop {
        if let Some(l) = try_lock(&config, "server") {
            break l;
        }
        std::thread::sleep(Duration::from_secs(5));
    };
    log::info!("DropBeam --server: running the Transfer Server in the background");
    crate::mailbox::set_headless();
    tauri::async_runtime::block_on(async move {
        let state = Arc::new(crate::iroh_net::IrohState::default());
        let _ = state.location_config.set(config.clone());
        let ep = match crate::iroh_net::start(&config).await {
            Ok(ep) => ep,
            Err(e) => {
                log::error!("DropBeam --server: the network engine didn't start: {e:#}");
                std::process::exit(1);
            }
        };
        let _ = state.endpoint.set(ep.clone());
        crate::mailbox::server::spawn_delivery(config.clone(), state.clone());
        crate::mailbox::client::spawn(state.clone());
        let accept = tauri::async_runtime::spawn(crate::iroh_net::accept_loop(ep.clone(), state.clone()));
        wait_for_stop().await;
        log::info!("DropBeam --server: stopping");
        accept.abort();
        ep.close().await;
    });
}

async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn one_engine_per_identity() {
        let dir = std::env::temp_dir().join(format!("dropbeam-lock-{}", uuid::Uuid::new_v4()));
        let a = try_lock(&dir, "server").expect("first lock");
        assert_eq!(holder(&dir).map(|h| h.1), Some("server".into()));
        // flock is per open file description: a second open in-process conflicts too.
        assert!(try_lock(&dir, "app").is_none());
        drop(a);
        assert!(try_lock(&dir, "app").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
