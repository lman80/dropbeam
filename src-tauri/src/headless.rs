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

#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const IDENTIFIER: &str = "com.dropbeam.app";

/// The desktop app's config folder (same as tauri's app_config_dir).
pub fn default_config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Application Support");
    // tauri's app_config_dir on Windows is the Roaming AppData folder; the XDG
    // fallback below pointed `--server` at a different identity/config entirely.
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("AppData").join("Roaming"));
    #[cfg(not(any(target_os = "macos", windows)))]
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

/// Where the holder's "pid mode" is recorded: inside the lock file on Unix
/// (advisory flock), beside it on Windows (mandatory LockFileEx).
fn owner_path(config: &Path) -> PathBuf {
    if cfg!(windows) { config.join("engine.owner") } else { lock_path(config) }
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
        // Windows: std's exclusive lock (LockFileEx), same contract.
        let _ = std::fs::create_dir_all(config);
        let file = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false)
            .open(lock_path(config)).ok()?;
        if file.try_lock().is_err() {
            return None;
        }
        // Windows locks are mandatory: nobody else could read the holder from
        // the locked file itself, so it is recorded beside it.
        let _ = std::fs::write(owner_path(config), format!("{} {}", std::process::id(), mode));
        Some(EngineLock { file })
    }
}

/// Ask (then make) a background `--server` holding the engine stop.
fn stop_server(pid: i32, force: bool) {
    #[cfg(unix)]
    unsafe { libc::kill(pid, if force { libc::SIGKILL } else { libc::SIGTERM }) };
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string()]);
        if force { cmd.arg("/F"); }
        let _ = cmd.output();
    }
}

/// How long the app waits for a background service to hand the engine over
/// before forcing it, and the hard ceiling after which the app starts anyway
/// (a wedged service must never leave the app with no network at all).
const TAKEOVER_FORCE_AFTER: u32 = 80; // × 250 ms = 20 s
const TAKEOVER_GIVE_UP_AFTER: u32 = 120; // × 250 ms = 30 s

/// Who holds the lock right now (pid, mode), if anyone wrote it.
fn holder(config: &Path) -> Option<(i32, String)> {
    let text = std::fs::read_to_string(owner_path(config)).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.to_owned()))
}

/// The GUI's side: take the engine, asking a background service to stop
/// first. Bounded: a polite stop, then a forced one after 20 s, and after 30 s
/// the app proceeds regardless (logged) instead of never starting its network.
pub fn lock_for_app(config: &Path) -> Option<EngineLock> {
    lock_for_app_with(config, Duration::from_millis(250))
}

fn lock_for_app_with(config: &Path, tick: Duration) -> Option<EngineLock> {
    if let Some(l) = try_lock(config, "app") {
        return Some(l);
    }
    let mut tries = 0u32;
    loop {
        if tries % 20 == 0 || tries == TAKEOVER_FORCE_AFTER {
            if let Some((pid, mode)) = holder(config) {
                if mode == "server" && pid > 1 && pid as u32 != std::process::id() {
                    let force = tries >= TAKEOVER_FORCE_AFTER;
                    log::info!("engine: the background Transfer Server is running; {} it for the app",
                        if force { "force-stopping" } else { "taking over from" });
                    stop_server(pid, force);
                }
            }
        }
        std::thread::sleep(tick);
        if let Some(l) = try_lock(config, "app") {
            return Some(l);
        }
        tries += 1;
        if tries >= TAKEOVER_GIVE_UP_AFTER {
            log::error!("engine: the background Transfer Server did not release the engine; starting anyway");
            return None;
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
    fn app_takeover_wait_is_bounded() {
        // Someone else (here: this process, mode "app" so nobody is signalled)
        // holds the engine forever — the app must still give up and start.
        let dir = std::env::temp_dir().join(format!("dropbeam-lock-{}", uuid::Uuid::new_v4()));
        let _held = try_lock(&dir, "app").expect("first lock");
        let t = std::time::Instant::now();
        assert!(lock_for_app_with(&dir, Duration::from_millis(1)).is_none());
        assert!(t.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

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
