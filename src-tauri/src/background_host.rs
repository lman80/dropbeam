//! "Keep running when no one is signed in": start `DropBeam --server` when the
//! computer starts, before anyone signs in, so hosted Locations and the
//! Transfer Server stay reachable on an always-on PC (headless.rs).
//!
//! - Windows: a Scheduled Task `DropBeam Background` (at startup, runs as the
//!   current user with logon type S4U — no password stored — limited rights,
//!   no time limit, restarts if it fails). Turning it on or off asks Windows
//!   for permission once (UAC); reading it doesn't.
//! - Linux: the .deb's systemd unit; the app only shows the command.
//! - macOS: not offered.
//!
//! The task passes `--config <the app's own config folder>` explicitly, so the
//! background copy uses the same identity and Locations as the window even if
//! the S4U session's environment differs from a signed-in one.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const TASK_NAME: &str = "DropBeam Background";
pub const LINUX_COMMAND: &str = "sudo systemctl enable --now dropbeam-server@$USER";

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

/// A PowerShell single-quoted string literal. PowerShell also ends a
/// single-quoted string at the typographic quotes ‘ ’ ‚ ‛, so every one of
/// them is doubled too — nothing in the value can close the literal.
pub fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
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
/// control characters) before they go anywhere near a script.
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

/// The elevated script that (re)creates the task and starts it right away
/// (it waits while the app holds the engine, then takes over when it closes).
pub fn register_script(exe: &str, config: &Path, user: &str, err_file: &Path) -> Result<String, String> {
    let exe = checked("program path", exe)?;
    let config_s = config.to_string_lossy();
    checked("settings folder", &config_s)?;
    let user = checked("account name", user)?;
    let name = ps_quote(TASK_NAME);
    Ok(format!(
        "$ErrorActionPreference = 'Stop'\n\
         try {{\n\
         $old = Get-ScheduledTask -TaskPath '\\' -TaskName {name} -ErrorAction SilentlyContinue\n\
         if ($old) {{ Stop-ScheduledTask -InputObject $old -ErrorAction SilentlyContinue }}\n\
         $action = New-ScheduledTaskAction -Execute {exe} -Argument {args}\n\
         $trigger = New-ScheduledTaskTrigger -AtStartup\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -DontStopOnIdleEnd -StartWhenAvailable \
         -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -Priority 4\n\
         $principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType S4U -RunLevel Limited\n\
         Register-ScheduledTask -TaskPath '\\' -TaskName {name} -Action $action -Trigger $trigger -Settings $settings -Principal $principal \
         -Description 'Keeps DropBeam sharing your Locations and holding messages when no one is signed in. Turn it off in DropBeam settings.' -Force | Out-Null\n\
         Start-ScheduledTask -TaskPath '\\' -TaskName {name}\n\
         exit 0\n\
         }} catch {{ [IO.File]::WriteAllText({err}, $_.Exception.Message); exit 1 }}\n",
        exe = ps_quote(exe),
        args = ps_quote(&task_arguments(config)),
        user = ps_quote(user),
        err = ps_quote(&err_file.to_string_lossy()),
    ))
}

/// The elevated script that stops and removes the task (fine if it's gone).
pub fn unregister_script(err_file: &Path) -> String {
    let name = ps_quote(TASK_NAME);
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         try {{\n\
         $t = Get-ScheduledTask -TaskPath '\\' -TaskName {name} -ErrorAction SilentlyContinue\n\
         if ($t) {{ Stop-ScheduledTask -InputObject $t -ErrorAction SilentlyContinue; Unregister-ScheduledTask -TaskPath '\\' -TaskName {name} -Confirm:$false }}\n\
         exit 0\n\
         }} catch {{ [IO.File]::WriteAllText({err}, $_.Exception.Message); exit 1 }}\n",
        err = ps_quote(&err_file.to_string_lossy()),
    )
}

/// Read-only (no elevation): the task as one JSON line on stdout, in UTF-8.
pub fn status_script() -> String {
    let name = ps_quote(TASK_NAME);
    format!(
        "$ErrorActionPreference = 'SilentlyContinue'\n\
         [Console]::OutputEncoding = [Text.Encoding]::UTF8\n\
         $t = Get-ScheduledTask -TaskPath '\\' -TaskName {name}\n\
         if ($t) {{ [pscustomobject]@{{ installed = $true; enabled = [bool]$t.Settings.Enabled; state = [string]$t.State; \
         execute = [string]$t.Actions[0].Execute; arguments = [string]$t.Actions[0].Arguments }} | ConvertTo-Json -Compress }} \
         else {{ '{{\"installed\":false}}' }}\n"
    )
}

/// `powershell -EncodedCommand` takes base64 of the UTF-16LE script: no
/// command-line quoting is involved at all.
pub fn encode_command(script: &str) -> String {
    use base64::Engine as _;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TaskInfo {
    installed: bool,
    enabled: bool,
    state: String,
    execute: String,
    arguments: String,
}

/// What `status_script` printed, judged against this app's exe and config.
pub fn parse_status(json: &str, exe: &str, config: &Path) -> BackgroundHostStatus {
    let t: TaskInfo = serde_json::from_str(json.trim().trim_start_matches('\u{feff}')).unwrap_or_default();
    let same = |a: &str, b: &str| a.trim().trim_matches('"').eq_ignore_ascii_case(b.trim());
    BackgroundHostStatus {
        supported: true,
        enabled: t.installed && t.enabled,
        running: t.installed && t.state.eq_ignore_ascii_case("Running"),
        current: t.installed && same(&t.execute, exe) && same(&t.arguments, &task_arguments(config)),
        command: None,
    }
}

// ── Windows ─────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Windows PowerShell by full path (never whatever `powershell` is on PATH).
    fn powershell() -> String {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe")
    }

    pub fn exe() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // `\\?\C:\…` would still work, but compare and display as Windows does.
        let s = exe.to_string_lossy().into_owned();
        Ok(s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s))
    }

    fn user() -> Result<String, String> {
        let name = std::env::var("USERNAME").map_err(|_| "Couldn't tell which Windows account this is.".to_string())?;
        Ok(match std::env::var("USERDOMAIN") {
            Ok(d) if !d.is_empty() => format!(r"{d}\{name}"),
            _ => name,
        })
    }

    pub fn status(config: &Path) -> Result<BackgroundHostStatus, String> {
        let out = std::process::Command::new(powershell())
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode_command(&status_script())])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("Couldn't check Windows Task Scheduler: {e}"))?;
        Ok(parse_status(&String::from_utf8_lossy(&out.stdout), &exe()?, config))
    }

    /// Run `script` in an elevated, hidden Windows PowerShell — one UAC prompt —
    /// and wait for it. The script reports failures through `err_file`.
    fn run_elevated(script: &str, err_file: &Path) -> Result<(), String> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let file = wide(&powershell());
        let params = wide(&format!(
            "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand {}",
            encode_command(script)
        ));
        let _ = std::fs::remove_file(err_file);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: 0, // SW_HIDE
            ..Default::default()
        };
        if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "Windows didn't get permission, so nothing changed.".into()
            } else {
                format!("Windows couldn't ask for permission: {}", e.message())
            });
        }
        if info.hProcess.is_invalid() {
            return Err("Windows didn't start the change.".into());
        }
        let mut code = 1u32;
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
        }
        let detail = std::fs::read_to_string(err_file).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let _ = std::fs::remove_file(err_file);
        if code == 0 {
            return Ok(());
        }
        Err(match detail {
            Some(d) => format!("Windows Task Scheduler said: {d}"),
            None => "Windows Task Scheduler didn't make the change.".into(),
        })
    }

    pub fn set(enabled: bool, config: &Path) -> Result<(), String> {
        let err_file = std::env::temp_dir().join(format!("dropbeam-bg-{}.txt", uuid::Uuid::new_v4()));
        let script = if enabled {
            register_script(&exe()?, config, &user()?, &err_file)?
        } else {
            unregister_script(&err_file)
        };
        run_elevated(&script, &err_file)
    }

    /// At app start: if the task is on but not running (e.g. an update ended
    /// it), start it again so it's there when the window closes. No elevation:
    /// the task's own user may run it.
    pub fn ensure_started(config: &Path) {
        if let Ok(s) = status(config) {
            if s.enabled && s.current && !s.running {
                let _ = std::process::Command::new("schtasks")
                    .args(["/Run", "/TN", TASK_NAME])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
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
    fn parse_win_args(cmd: &str) -> Vec<String> {
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
    }

    #[test]
    fn powershell_literals_cannot_be_closed() {
        assert_eq!(ps_quote(r"C:\Program Files\DropBeam\DropBeam.exe"), r"'C:\Program Files\DropBeam\DropBeam.exe'");
        assert_eq!(ps_quote("O'Brien"), "'O''Brien'");
        assert_eq!(ps_quote("a\u{2019}; calc; \u{2018}"), "'a\u{2019}\u{2019}; calc; \u{2018}\u{2018}'");
        assert_eq!(ps_quote("$(calc) `n"), "'$(calc) `n'");
        // Every quote char inside is doubled, so the literal only ends at the last char.
        for s in ["'", "''", "x'; Remove-Item C:\\ -Recurse; '", "\u{201A}\u{201B}"] {
            let q = ps_quote(s);
            let inner: Vec<char> = q.chars().skip(1).collect::<Vec<_>>()[..q.chars().count() - 2].to_vec();
            let mut i = 0;
            while i < inner.len() {
                if matches!(inner[i], '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    assert!(i + 1 < inner.len() && inner[i + 1] == inner[i], "unescaped quote in {q}");
                    i += 2;
                } else { i += 1; }
            }
        }
    }

    #[test]
    fn register_script_quotes_everything_and_refuses_bad_values() {
        let config = PathBuf::from(r"C:\Users\O'Neil\AppData\Roaming\com.dropbeam.app");
        let s = register_script(r"C:\Users\O'Neil\AppData\Local\DropBeam\DropBeam.exe", &config, r"PC\O'Neil", &PathBuf::from(r"C:\t\e.txt")).unwrap();
        assert!(s.contains(r"-Execute 'C:\Users\O''Neil\AppData\Local\DropBeam\DropBeam.exe'"), "{s}");
        assert!(s.contains(r#"-Argument '--server --config C:\Users\O''Neil\AppData\Roaming\com.dropbeam.app'"#), "{s}");
        assert!(s.contains(r"-UserId 'PC\O''Neil' -LogonType S4U -RunLevel Limited"), "{s}");
        assert!(s.contains("-TaskName 'DropBeam Background'") && s.contains("-AtStartup") && s.contains("[TimeSpan]::Zero"));
        assert!(s.contains("-AllowStartIfOnBatteries -DontStopIfGoingOnBatteries") && s.contains("-RestartCount"));
        for bad in ["C:\\a\"b.exe", "C:\\a\nb.exe", "", "  "] {
            assert!(register_script(bad, &config, "PC\\u", &PathBuf::from("e")).is_err(), "{bad:?}");
        }
        assert!(register_script("C:\\d.exe", &config, "PC\\u\r\nx", &PathBuf::from("e")).is_err());
        assert!(unregister_script(&PathBuf::from("e")).contains("Unregister-ScheduledTask -TaskPath '\\' -TaskName 'DropBeam Background' -Confirm:$false"));
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD.decode(encode_command("Ä'x")).unwrap();
        assert_eq!(raw, vec![0xC4, 0, b'\'', 0, b'x', 0]);
    }

    #[test]
    fn status_compares_exe_and_config() {
        let config = PathBuf::from(r"C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app");
        let exe = r"C:\Users\Ann Lee\AppData\Local\DropBeam\DropBeam.exe";
        let json = serde_json::json!({"installed": true, "enabled": true, "state": "Running",
            "execute": exe.to_uppercase(), "arguments": task_arguments(&config)}).to_string();
        let s = parse_status(&format!("\u{feff}{json}\r\n"), exe, &config);
        assert_eq!(s, BackgroundHostStatus { supported: true, enabled: true, running: true, current: true, command: None });
        let moved = parse_status(&json, r"D:\Apps\DropBeam.exe", &config);
        assert!(moved.enabled && !moved.current);
        let ready = parse_status(&json.replace("Running", "Ready").replace("\"enabled\":true", "\"enabled\":false"), exe, &config);
        assert!(!ready.enabled && !ready.running && ready.current);
        assert_eq!(parse_status(r#"{"installed":false}"#, exe, &config), BackgroundHostStatus { supported: true, ..Default::default() });
        assert_eq!(parse_status("garbage", exe, &config), BackgroundHostStatus { supported: true, ..Default::default() });
    }
}
