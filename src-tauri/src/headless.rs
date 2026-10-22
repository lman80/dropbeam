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
//!
//! Windows ("Keep running when no one is signed in", background_host.rs): a
//! Scheduled Task starts `--server` at boot in its own non-interactive (S4U)
//! session. Signals don't cross sessions, so there `--server` is a small
//! supervisor that keeps one engine child alive (`--server-child`, in a
//! kill-on-close job so ending the task ends both). The app asks the engine to
//! step aside by writing its pid to `engine.stop` in the shared config folder;
//! the engine polls for it, exits, and the supervisor starts a fresh engine that
//! waits for the lock again — so it takes back over as soon as the app quits.

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

/// The app's "please step aside" note for a Windows background engine, which
/// may run in another session where no signal or window message reaches it.
fn stop_path(config: &Path) -> PathBuf {
    config.join("engine.stop")
}

/// Whether the app asked THIS process (by pid) to hand the engine over.
#[cfg_attr(unix, allow(dead_code))]
fn stop_requested(config: &Path) -> bool {
    std::fs::read_to_string(stop_path(config)).ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .is_some_and(|pid| pid == std::process::id())
}

/// Ask (then make) a background `--server` holding the engine stop.
fn stop_server(config: &Path, pid: i32, force: bool) {
    #[cfg(unix)]
    {
        let _ = config;
        unsafe { libc::kill(pid, if force { libc::SIGKILL } else { libc::SIGTERM }) };
    }
    #[cfg(windows)]
    {
        // Polite: the engine polls for this file (works across sessions,
        // where taskkill without /F has no window to close).
        let _ = std::fs::write(stop_path(config), pid.to_string());
        if force {
            // Last resort; may be refused across sessions — the app then
            // starts anyway after TAKEOVER_GIVE_UP_AFTER.
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000).output();
        }
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
        if tries.is_multiple_of(20) || tries == TAKEOVER_FORCE_AFTER {
            if let Some((pid, mode)) = holder(config) {
                if mode == "server" && pid > 1 && pid as u32 != std::process::id() {
                    let force = tries >= TAKEOVER_FORCE_AFTER;
                    log::info!("engine: the background Transfer Server is running; {} it for the app",
                        if force { "force-stopping" } else { "taking over from" });
                    stop_server(config, pid, force);
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

/// `--config DIR` if given, else the app's own folder.
fn config_arg(args: &[String]) -> PathBuf {
    args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)).map(PathBuf::from)
        .unwrap_or_else(default_config_dir)
}

/// Windows: the supervisor runs the engine as this child (see the module doc).
#[cfg(windows)]
const CHILD_FLAG: &str = "--server-child";

/// Entry point for `DropBeam --server [--config DIR]`.
pub fn run(args: &[String]) {
    let _ = log::set_boxed_logger(Box::new(StderrLog)).map(|()| log::set_max_level(log::LevelFilter::Info));
    let config = config_arg(args);
    #[cfg(windows)]
    if !args.iter().any(|a| a == CHILD_FLAG) {
        supervise(&config);
    }
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
    // A note left for an earlier engine (or a crashed one) isn't for us.
    let _ = std::fs::remove_file(stop_path(&config));
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
        wait_for_stop(&config).await;
        log::info!("DropBeam --server: stopping");
        accept.abort();
        ep.close().await;
    });
}

async fn wait_for_stop(config: &Path) {
    #[cfg(unix)]
    {
        let _ = config;
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let asked = async {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if stop_requested(config) {
                    let _ = std::fs::remove_file(stop_path(config));
                    log::info!("DropBeam --server: the app is taking over");
                    return;
                }
            }
        };
        // No console (a Scheduled Task): if Ctrl+C can't be hooked, it never fires.
        let ctrl_c = async {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            _ = asked => {},
            _ = ctrl_c => {},
        }
    }
}

/// Where the Windows background engine and its supervisor write their log
/// (there's no console). Kept to ~1 MB, the previous one beside it.
#[cfg(windows)]
fn log_file(config: &Path) -> Option<std::fs::File> {
    let path = config.join("background-server.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = std::fs::rename(&path, config.join("background-server.old.log"));
    }
    let _ = std::fs::create_dir_all(config);
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

#[cfg(windows)]
fn note(config: &Path, msg: &str) {
    if let Some(mut f) = log_file(config) {
        use std::io::Write as _;
        let _ = writeln!(f, "{} supervisor: {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// A job that kills the engine child if the supervisor goes away (the task
/// was ended, or it crashed): never two engines, never an orphan.
#[cfg(windows)]
fn kill_on_close_job() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32).ok()?;
        Some(job)
    }
}

/// Windows `--server`: keep one engine child running for as long as the
/// Scheduled Task runs. A child that handed over to the app (exit 0) is
/// replaced at once (the new one waits for the lock); a crash is retried
/// after a pause. Never returns.
#[cfg(windows)]
fn supervise(config: &Path) -> ! {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("DropBeam.exe"));
    let job = kill_on_close_job();
    note(config, &format!("started (pid {})", std::process::id()));
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["--server", CHILD_FLAG, "--config"]).arg(config)
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null());
        if let Some(f) = log_file(config) {
            if let Ok(out) = f.try_clone() {
                cmd.stdout(out);
            }
            cmd.stderr(f);
        }
        let started = std::time::Instant::now();
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(job) = job {
                    let h = windows::Win32::Foundation::HANDLE(child.as_raw_handle());
                    let _ = unsafe { windows::Win32::System::JobObjects::AssignProcessToJobObject(job, h) };
                }
                let status = child.wait();
                if !status.as_ref().is_ok_and(|s| s.success()) {
                    note(config, &format!("the engine stopped unexpectedly ({status:?}); restarting it"));
                    std::thread::sleep(Duration::from_secs(10));
                } else if started.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            Err(e) => {
                note(config, &format!("couldn't start the engine: {e}"));
                std::thread::sleep(Duration::from_secs(30));
            }
        }
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

#[cfg(test)]
mod stop_tests {
    use super::*;

    #[test]
    fn stop_note_is_only_for_the_named_engine() {
        let dir = std::env::temp_dir().join(format!("dropbeam-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!stop_requested(&dir));
        std::fs::write(stop_path(&dir), (std::process::id() + 1).to_string()).unwrap();
        assert!(!stop_requested(&dir), "another pid's note");
        std::fs::write(stop_path(&dir), format!("{}\n", std::process::id())).unwrap();
        assert!(stop_requested(&dir));
        let args: Vec<String> = ["DropBeam.exe", "--server", "--config", r"C:\a b"].map(String::from).to_vec();
        assert_eq!(config_arg(&args), PathBuf::from(r"C:\a b"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Windows: the app's polite stop is a note the engine (another session) polls.
    #[cfg(windows)]
    #[test]
    fn app_leaves_a_stop_note_on_windows() {
        let dir = std::env::temp_dir().join(format!("dropbeam-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        stop_server(&dir, std::process::id() as i32, false);
        assert!(stop_requested(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
