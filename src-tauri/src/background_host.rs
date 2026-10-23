//! "Keep running when no one is signed in": start `DropBeam --server` when the
//! computer starts, before anyone signs in, so hosted Locations and the
//! Transfer Server stay reachable on an always-on PC (headless.rs).
//!
//! - Windows: a Scheduled Task `DropBeam Background` (at startup, runs as the
//!   current user with logon type S4U — no password stored — limited rights,
//!   no time limit, restarts if it fails), made through the Task Scheduler COM
//!   API. Turning it on or off asks Windows for permission once (UAC): DropBeam
//!   re-launches itself elevated for that one step (elevate.rs) — never a
//!   script host, which antivirus treats as malware. Reading it doesn't.
//! - Linux: the .deb's systemd unit; the app only shows the command.
//! - macOS: not offered.
//!
//! The task passes `--config <the app's own config folder>` explicitly, so the
//! background copy uses the same identity and Locations as the window even if
//! the S4U session's environment differs from a signed-in one.

// The task helpers are pure (tested everywhere) but only called on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;
use std::path::Path;

pub const TASK_NAME: &str = "DropBeam Background";
pub const LINUX_COMMAND: &str = "sudo systemctl enable --now dropbeam-server@$USER";
const DESCRIPTION: &str = "Keeps DropBeam sharing your Locations and holding messages when no one is signed in. Turn it off in DropBeam settings.";

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundHostStatus {
    /// The switch works on this platform (Windows).
    pub supported: bool,
    /// The startup task exists and is enabled.
    pub enabled: bool,
    /// The background copy is running right now (it waits while the app is open).
    pub running: bool,
    /// The task starts THIS DropBeam with THIS config folder (false after a move).
    pub current: bool,
    /// Linux: the command that turns it on instead (no switch there).
    pub command: Option<String>,
}

/// One argument quoted for a Windows command line (CommandLineToArgvW /
/// MSVC rules: backslashes only escape when they precede a quote).
pub fn win_arg(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| matches!(c, ' ' | '\t' | '"')) {
        return s.to_owned();
    }
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// Refuse values that have no business in a path or account name (quotes,
/// control characters — XML can't even carry most of those).
fn checked<'a>(what: &str, s: &'a str) -> Result<&'a str, String> {
    if s.trim().is_empty() || s.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("DropBeam can't set this up: its {what} has characters Windows doesn't allow here."));
    }
    Ok(s)
}

/// The task action's arguments: run the background engine on the app's own config.
pub fn task_arguments(config: &Path) -> String {
    format!("--server --config {}", win_arg(&config.to_string_lossy()))
}

/// Text for an XML element or attribute value.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// The task definition (Task Scheduler 1.2 schema): at startup, as `user`
/// with S4U and limited rights, no time limit, on batteries too, restart
/// every minute if it fails, one copy at a time, normal-ish priority.
pub fn task_xml(exe: &str, config: &Path, user: &str) -> Result<String, String> {
    let exe = checked("program path", exe)?;
    let config_s = config.to_string_lossy();
    checked("settings folder", &config_s)?;
    let user = checked("account name", user)?;
    Ok(format!(
        "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\
<RegistrationInfo><Description>{desc}</Description></RegistrationInfo>\
<Triggers><BootTrigger><Enabled>true</Enabled></BootTrigger></Triggers>\
<Principals><Principal id=\"Author\"><UserId>{user}</UserId><LogonType>S4U</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\
<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable>\
<IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings><AllowStartOnDemand>true</AllowStartOnDemand>\
<Enabled>true</Enabled><Hidden>false</Hidden><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>4</Priority>\
<RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure></Settings>\
<Actions Context=\"Author\"><Exec><Command>{exe}</Command><Arguments>{args}</Arguments></Exec></Actions></Task>",
        desc = xml_escape(DESCRIPTION),
        user = xml_escape(user),
        exe = xml_escape(exe),
        args = xml_escape(&task_arguments(config)),
    ))
}

/// What Task Scheduler has under TASK_NAME.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskInfo {
    pub installed: bool,
    pub enabled: bool,
    pub running: bool,
    pub execute: String,
    pub arguments: String,
}

/// `t` judged against this app's exe and config.
pub fn judge(t: &TaskInfo, exe: &str, config: &Path) -> BackgroundHostStatus {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    // The program path may be stored quoted (Task Scheduler accepts both).
    let exe_same = |a: &str, b: &str| same(a.trim().trim_matches('"'), b);
    BackgroundHostStatus {
        supported: true,
        enabled: t.installed && t.enabled,
        running: t.installed && t.running,
        current: t.installed && exe_same(&t.execute, exe) && same(&t.arguments, &task_arguments(config)),
        command: None,
    }
}

// ── Windows ─────────────────────────────────────────────────────────────────

#[cfg(windows)]
pub(crate) mod win {
    use super::*;
    use crate::elevate::win::Com;
    use windows::core::{Interface, BSTR, HRESULT};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::TaskScheduler::{IExecAction, IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler, TASK_CREATE_OR_UPDATE, TASK_LOGON_S4U, TASK_STATE_RUNNING};
    use windows::Win32::System::Variant::VARIANT;

    /// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND): no such task.
    const NOT_FOUND: HRESULT = HRESULT(0x8007_0002_u32 as i32);

    pub fn exe() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // `\\?\C:\…` would still work, but compare and display as Windows does.
        let s = exe.to_string_lossy().into_owned();
        Ok(s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s))
    }

    /// The root task folder on this computer (caller holds COM).
    fn root() -> windows::core::Result<ITaskFolder> {
        unsafe {
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            svc.Connect(&none, &none, &none, &none)?;
            svc.GetFolder(&BSTR::from(r"\"))
        }
    }

    fn task(root: &ITaskFolder) -> windows::core::Result<Option<IRegisteredTask>> {
        match unsafe { root.GetTask(&BSTR::from(TASK_NAME)) } {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Read-only (no elevation).
    pub fn info() -> windows::core::Result<TaskInfo> {
        let _com = Com::init();
        let Some(t) = task(&root()?)? else { return Ok(TaskInfo::default()) };
        unsafe {
            let (mut execute, mut arguments) = (BSTR::new(), BSTR::new());
            let actions = t.Definition()?.Actions()?;
            let mut count = 0i32;
            actions.Count(&mut count)?;
            if count >= 1 {
                if let Ok(exec) = actions.get_Item(1)?.cast::<IExecAction>() {
                    exec.Path(&mut execute)?;
                    exec.Arguments(&mut arguments)?;
                }
            }
            Ok(TaskInfo {
                installed: true,
                enabled: t.Enabled()?.as_bool(),
                running: t.State()? == TASK_STATE_RUNNING,
                execute: execute.to_string(),
                arguments: arguments.to_string(),
            })
        }
    }

    /// (Re)create the task for this exe and start it right away (it waits
    /// while the app holds the engine, then takes over when it closes).
    /// Needs administrator rights (elevate.rs).
    pub fn register(config: &Path, user: &str) -> windows::core::Result<()> {
        let invalid = |m: String| windows::core::Error::new(windows::Win32::Foundation::E_INVALIDARG, m);
        let xml = task_xml(&exe().map_err(invalid)?, config, user).map_err(invalid)?;
        let _com = Com::init();
        let root = root()?;
        if let Some(old) = task(&root)? {
            let _ = unsafe { old.Stop(0) };
        }
        let none = VARIANT::default();
        let t = unsafe { root.RegisterTask(&BSTR::from(TASK_NAME), &BSTR::from(xml), TASK_CREATE_OR_UPDATE.0, &VARIANT::from(user), &none, TASK_LOGON_S4U, &none) }?;
        unsafe { t.Run(&none) }?;
        Ok(())
    }

    /// Stop and delete the task (fine if it's gone). Needs administrator rights.
    pub fn remove() -> windows::core::Result<()> {
        let _com = Com::init();
        let root = root()?;
        if let Some(t) = task(&root)? {
            let _ = unsafe { t.Stop(0) };
            match unsafe { root.DeleteTask(&BSTR::from(TASK_NAME), 0) } {
                Err(e) if e.code() != NOT_FOUND => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    /// Start the task now (no elevation: the task's own user may run it).
    pub fn run_now() -> windows::core::Result<()> {
        let _com = Com::init();
        let Some(t) = task(&root()?)? else { return Ok(()) };
        unsafe { t.Run(&VARIANT::default()) }.map(|_| ())
    }

    pub fn status(config: &Path) -> Result<BackgroundHostStatus, String> {
        let t = info().map_err(|e| format!("Couldn't check Windows Task Scheduler: {}", e.message()))?;
        Ok(judge(&t, &exe()?, config))
    }

    pub fn set(enabled: bool, config: &Path) -> Result<(), String> {
        use crate::elevate::Job;
        let job = if enabled {
            Job::RegisterTask { config: config.to_path_buf(), user: crate::elevate::win::current_account()? }
        } else {
            Job::RemoveTask
        };
        crate::elevate::win::run(&job, "Windows Task Scheduler")
    }

    /// At app start: if the task is on but not running (e.g. an update ended
    /// it), start it again so it's there when the window closes.
    pub fn ensure_started(config: &Path) {
        if let Ok(s) = status(config) {
            if s.enabled && s.current && !s.running {
                let _ = run_now();
            }
        }
    }
}

#[cfg(windows)]
pub fn status(config: &Path) -> Result<BackgroundHostStatus, String> {
    win::status(config)
}
#[cfg(target_os = "linux")]
pub fn status(_config: &Path) -> Result<BackgroundHostStatus, String> {
    Ok(BackgroundHostStatus { command: Some(LINUX_COMMAND.into()), ..Default::default() })
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn status(_config: &Path) -> Result<BackgroundHostStatus, String> {
    Ok(BackgroundHostStatus::default())
}

#[cfg(windows)]
pub fn set(enabled: bool, config: &Path) -> Result<(), String> {
    win::set(enabled, config)
}
#[cfg(target_os = "linux")]
pub fn set(_enabled: bool, _config: &Path) -> Result<(), String> {
    Err(format!("On Linux, run this in a terminal instead: {LINUX_COMMAND}"))
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn set(_enabled: bool, _config: &Path) -> Result<(), String> {
    Err("Keeping DropBeam running when no one is signed in isn't available on this computer.".into())
}

#[cfg(windows)]
pub fn ensure_started(config: &Path) {
    win::ensure_started(config)
}
#[cfg(not(windows))]
pub fn ensure_started(_config: &Path) {}

// ── Commands ────────────────────────────────────────────────────────────────

/// Whether DropBeam starts in the background when the computer starts.
#[tauri::command]
pub async fn background_host_status(state: tauri::State<'_, std::sync::Arc<crate::AppState>>) -> Result<BackgroundHostStatus, String> {
    let config = state.config_dir.clone();
    tokio::task::spawn_blocking(move || status(&config)).await.map_err(|e| e.to_string())?
}

/// Turn "Keep running when no one is signed in" on or off. Windows asks for
/// permission (UAC) on this computer's screen.
#[tauri::command]
pub async fn background_host_set(state: tauri::State<'_, std::sync::Arc<crate::AppState>>, enabled: bool) -> Result<BackgroundHostStatus, String> {
    let config = state.config_dir.clone();
    tokio::task::spawn_blocking(move || {
        set(enabled, &config)?;
        status(&config)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// CommandLineToArgvW, enough of it to check `win_arg` round-trips.
    pub(crate) fn parse_win_args(cmd: &str) -> Vec<String> {
        let (mut args, mut cur, mut quoted, mut any) = (vec![], String::new(), false, false);
        let chars: Vec<char> = cmd.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                let start = i;
                while i < chars.len() && chars[i] == '\\' { i += 1; }
                let n = i - start;
                if i < chars.len() && chars[i] == '"' {
                    cur.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 { cur.push('"'); i += 1; }
                } else {
                    cur.extend(std::iter::repeat_n('\\', n));
                }
                any = true;
                continue;
            }
            match c {
                '"' => { quoted = !quoted; any = true; }
                ' ' | '\t' if !quoted => { if any { args.push(std::mem::take(&mut cur)); any = false; } }
                _ => { cur.push(c); any = true; }
            }
            i += 1;
        }
        if any { args.push(cur); }
        args
    }

    #[test]
    fn windows_arguments_round_trip() {
        for s in [r"C:\Users\Noreen\AppData\Roaming\com.dropbeam.app", r"C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app",
            r"C:\a b\", r"C:\x\\", "plain", "", r#"we"ird"#, r#"a\"b"#, "tab\there"] {
            let line = format!("--config {}", win_arg(s));
            assert_eq!(parse_win_args(&line), vec!["--config".to_owned(), s.to_owned()], "{s:?} -> {line}");
        }
        let a = task_arguments(&PathBuf::from(r"C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app"));
        assert_eq!(a, r#"--server --config "C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app""#);
        // The elevated helper's command line comes back as exactly its arguments.
        let job = crate::elevate::Job::RegisterTask { config: PathBuf::from(r"C:\Users\O'Neil Ä\AppData\Roaming\com.dropbeam.app\"), user: r"PC\O'Neil Ä".into() };
        let mut argv = vec!["DropBeam.exe".to_owned()];
        argv.extend(parse_win_args(&crate::elevate::command_line(&job)));
        assert_eq!(crate::elevate::parse(&argv), Ok(job));
    }

    #[test]
    fn xml_values_cannot_break_out() {
        assert_eq!(xml_escape(r#"<a href="x">&'</a>"#), "&lt;a href=&quot;x&quot;&gt;&amp;&apos;&lt;/a&gt;");
        let config = PathBuf::from(r"C:\Users\Ann & <Lee>\AppData\Roaming\com.dropbeam.app");
        let x = task_xml(r"C:\Users\Ann & <Lee>\AppData\Local\DropBeam\DropBeam.exe", &config, r"PC\O'Neil</UserId><x>").unwrap();
        assert!(x.contains(r"<Command>C:\Users\Ann &amp; &lt;Lee&gt;\AppData\Local\DropBeam\DropBeam.exe</Command>"), "{x}");
        assert!(x.contains(r"<Arguments>--server --config &quot;C:\Users\Ann &amp; &lt;Lee&gt;\AppData\Roaming\com.dropbeam.app&quot;</Arguments>"), "{x}");
        assert!(x.contains(r"<UserId>PC\O&apos;Neil&lt;/UserId&gt;&lt;x&gt;</UserId><LogonType>S4U</LogonType><RunLevel>LeastPrivilege</RunLevel>"), "{x}");
        assert!(x.contains("<BootTrigger>") && x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>") && x.contains("<Priority>4</Priority>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</") && x.contains("<StopIfGoingOnBatteries>false</") && x.contains("<Count>999</Count>"));
        assert!(x.contains("<MultipleInstancesPolicy>IgnoreNew</"));
        // Every element opened is closed in order: nothing in a value made a tag.
        let tags: Vec<&str> = x.split('<').skip(1).map(|t| t.split(['>', ' ']).next().unwrap()).collect();
        let mut stack = vec![];
        for t in tags {
            if let Some(close) = t.strip_prefix('/') { assert_eq!(stack.pop(), Some(close), "{x}"); } else { stack.push(t); }
        }
        assert!(stack.is_empty());
        for bad in ["C:\\a\"b.exe", "C:\\a\nb.exe", "", "  ", "C:\\a\u{1}b.exe"] {
            assert!(task_xml(bad, &config, "PC\\u").is_err(), "{bad:?}");
        }
        assert!(task_xml("C:\\d.exe", &config, "PC\\u\r\nx").is_err());
        assert!(task_xml("C:\\d.exe", &PathBuf::from("C:\\a\"b"), "PC\\u").is_err());
    }

    #[test]
    fn status_compares_exe_and_config() {
        let config = PathBuf::from(r"C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app");
        let exe = r"C:\Users\Ann Lee\AppData\Local\DropBeam\DropBeam.exe";
        let t = TaskInfo { installed: true, enabled: true, running: true, execute: format!("\"{}\"", exe.to_uppercase()), arguments: task_arguments(&config) };
        assert_eq!(judge(&t, exe, &config), BackgroundHostStatus { supported: true, enabled: true, running: true, current: true, command: None });
        let moved = judge(&t, r"D:\Apps\DropBeam.exe", &config);
        assert!(moved.enabled && !moved.current);
        let ready = judge(&TaskInfo { enabled: false, running: false, ..t.clone() }, exe, &config);
        assert!(!ready.enabled && !ready.running && ready.current);
        assert_eq!(judge(&TaskInfo::default(), exe, &config), BackgroundHostStatus { supported: true, ..Default::default() });
        assert_eq!(judge(&TaskInfo { installed: false, ..t }, exe, &config), BackgroundHostStatus { supported: true, ..Default::default() });
    }

    /// CI only (windows-latest runs as an administrator): really register the
    /// task through the native path, read it back, run it, remove it. The
    /// action is this test binary, which just exits on `--server`.
    #[cfg(windows)]
    #[test]
    fn ci_task_scheduler_round_trip() {
        if std::env::var("DROPBEAM_CI_TASK_TEST").as_deref() != Ok("1") {
            return;
        }
        assert!(crate::elevate::win::is_elevated(), "the CI runner should be elevated");
        let config = std::env::temp_dir().join(format!("dropbeam ci task {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config).unwrap();
        let user = crate::elevate::win::current_account().unwrap();
        assert!(crate::elevate::valid_account(&user).is_ok(), "{user}");
        let _ = win::remove();
        // In-process (already elevated), exactly as the app does it.
        set(true, &config).unwrap();
        let t = win::info().unwrap();
        assert!(t.installed && t.enabled, "{t:?}");
        let s = status(&config).unwrap();
        assert!(s.supported && s.enabled && s.current, "{s:?} {t:?}");
        win::run_now().unwrap();
        // Another config folder isn't "current".
        assert!(!status(&config.join("other")).unwrap().current);
        set(false, &config).unwrap();
        assert_eq!(win::info().unwrap(), TaskInfo::default());
        assert_eq!(status(&config).unwrap(), BackgroundHostStatus { supported: true, ..Default::default() });
        set(false, &config).unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(&config);
    }
}
