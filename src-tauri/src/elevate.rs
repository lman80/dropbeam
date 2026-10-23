//! Changes that need administrator rights on Windows — the "Keep running when
//! no one is signed in" startup task and the drive's WebDAV fix — are made by
//! DropBeam.exe ITSELF through native APIs (Task Scheduler COM, the registry,
//! the Service Control Manager), never by a script host: antivirus behaviour
//! shields (AVG flagged `IDP.HELU.PSE95` and deleted DropBeam.exe) treat an
//! unsigned app that launches elevated/encoded PowerShell as malware.
//!
//! When the app isn't elevated it re-launches its own exe with the "runas"
//! verb (one UAC prompt) and ONE internal argument set, waits, and reads the
//! exit code; when it already is (an admin's SSH session, CI) it does the job
//! in-process:
//!
//! - `DropBeam.exe --background-task register --config <dir> --user <DOMAIN\name>`
//! - `DropBeam.exe --background-task remove`
//! - `DropBeam.exe --fix-webdav`
//!
//! main.rs routes these before any window or CLI code; the elevated copy
//! checks them strictly, does only that job and exits: 0 = done, 2 = arguments
//! refused, anything else = the HRESULT of what Windows refused.

use std::path::PathBuf;

pub const TASK_FLAG: &str = "--background-task";
pub const WEBDAV_FLAG: &str = "--fix-webdav";
pub const EXIT_BAD_ARGS: i32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// (Re)create the startup task for this exe with `config`, running as `user`, and start it.
    RegisterTask { config: PathBuf, user: String },
    /// Stop and delete the startup task (fine if it's gone).
    RemoveTask,
    /// WebClient starts automatically, file size limit 4 GB, restarted.
    FixWebdav,
}

/// Is this launch one of the internal elevated jobs?
pub fn is_job(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == TASK_FLAG || a == WEBDAV_FLAG)
}

/// The job in `args` (argv, program first) — exactly one of the shapes above,
/// nothing more, every value checked.
pub fn parse(args: &[String]) -> Result<Job, String> {
    let a: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match a.as_slice() {
        [WEBDAV_FLAG] => Ok(Job::FixWebdav),
        [TASK_FLAG, "remove"] => Ok(Job::RemoveTask),
        [TASK_FLAG, "register", "--config", config, "--user", user] => {
            Ok(Job::RegisterTask { config: PathBuf::from(valid_dir(config)?), user: valid_account(user)?.to_owned() })
        }
        _ => Err("DropBeam was started with arguments it doesn't accept here.".into()),
    }
}

/// The arguments (without the program) that `parse` turns back into `job`.
pub fn args_of(job: &Job) -> Vec<String> {
    match job {
        Job::RegisterTask { config, user } => vec![TASK_FLAG.into(), "register".into(), "--config".into(), config.to_string_lossy().into_owned(), "--user".into(), user.clone()],
        Job::RemoveTask => vec![TASK_FLAG.into(), "remove".into()],
        Job::FixWebdav => vec![WEBDAV_FLAG.into()],
    }
}

/// `args_of(job)` as one Windows command line (ShellExecuteExW's parameters).
pub fn command_line(job: &Job) -> String {
    args_of(job).iter().map(|a| crate::background_host::win_arg(a)).collect::<Vec<_>>().join(" ")
}

/// An absolute Windows folder path (`X:\…` or `\\server\share\…`, no device
/// paths, no `..`, no quotes, wildcards, pipes, stream colons or control characters).
pub fn valid_dir(s: &str) -> Result<&str, String> {
    let b = s.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    let unc = s.len() > 2 && s.starts_with(r"\\") && !s.starts_with(r"\\?\") && !s.starts_with(r"\\.\");
    let rest = if drive { &s[2..] } else { s };
    let bad = !(drive || unc)
        || s.len() > 1024
        || rest.chars().any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '?' | '*' | ':'))
        || s.split(['\\', '/']).any(|p| p == "..");
    if bad { Err("DropBeam can't set this up: its settings folder has characters Windows doesn't allow here.".into()) } else { Ok(s) }
}

/// `name` or `DOMAIN\name`, each part a valid Windows account name.
pub fn valid_account(s: &str) -> Result<&str, String> {
    let part = |p: &str| !p.is_empty() && p.len() <= 256 && p.trim() == p && !p.chars().any(|c| c.is_control() || r#""/\[]:;|=,+*?<>"#.contains(c));
    let ok = match s.split_once('\\') { Some((d, u)) => part(d) && part(u), None => part(s) };
    if ok { Ok(s) } else { Err("DropBeam can't set this up: its account name has characters Windows doesn't allow here.".into()) }
}

/// main.rs: run the internal job in `args` (if it is one) and return the exit code.
pub fn main_job(args: &[String]) -> Option<i32> {
    if !is_job(args) {
        return None;
    }
    let job = match parse(args) {
        Ok(Job::RegisterTask { config, .. }) if !config.is_dir() => return Some(EXIT_BAD_ARGS),
        Ok(j) => j,
        Err(_) => return Some(EXIT_BAD_ARGS),
    };
    #[cfg(windows)]
    return Some(match win::perform_here(&job) { Ok(()) => 0, Err(e) => e.code().0 });
    #[cfg(not(windows))]
    {
        let _ = job;
        Some(EXIT_BAD_ARGS)
    }
}

/// Words for a failed job's exit code.
pub fn describe_exit(code: u32, what: &str) -> String {
    match code {
        2 => "DropBeam couldn't make the change: its settings folder or account name was refused.".into(),
        c if c < 0x8000_0000 => format!("DropBeam couldn't make the change (it stopped with code {c})."),
        #[cfg(windows)]
        c => format!("{what} said: {}", windows::core::Error::from_hresult(windows::core::HRESULT(c as i32)).message()),
        #[cfg(not(windows))]
        c => format!("{what} said: error {c:#x}"),
    }
}

#[cfg(windows)]
pub mod win {
    use super::*;
    use windows::core::{w, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Security::{GetTokenInformation, LookupAccountSidW, TokenElevation, TokenUser, SID_NAME_USE, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Com::{CoInitializeEx, CoInitializeSecurity, CoUninitialize, COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    /// COM on this thread for the guard's lifetime (multithreaded; a thread that
    /// already chose single-threaded keeps it and isn't uninitialized here).
    pub struct Com(bool);
    impl Com {
        pub fn init() -> Com {
            Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn token() -> windows::core::Result<Handle> {
        let mut h = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut h) }?;
        Ok(Handle(h))
    }

    /// This process runs with full administrator rights (no UAC prompt needed).
    pub fn is_elevated() -> bool {
        let Ok(t) = token() else { return false };
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = unsafe { GetTokenInformation(t.0, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len) };
        ok.is_ok() && e.TokenIsElevated != 0
    }

    /// `DOMAIN\name` of the account this process runs as (from its token, not
    /// the environment).
    pub fn current_account() -> Result<String, String> {
        let fail = |e: windows::core::Error| format!("Couldn't tell which Windows account this is: {}", e.message());
        let t = token().map_err(fail)?;
        let mut len = 0u32;
        let _ = unsafe { GetTokenInformation(t.0, TokenUser, None, 0, &mut len) };
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        unsafe { GetTokenInformation(t.0, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len) }.map_err(fail)?;
        let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
        let (mut n, mut d, mut kind) = (0u32, 0u32, SID_NAME_USE::default());
        let _ = unsafe { LookupAccountSidW(PCWSTR::null(), sid, None, &mut n, None, &mut d, &mut kind) };
        let (mut name, mut domain) = (vec![0u16; n.max(1) as usize], vec![0u16; d.max(1) as usize]);
        unsafe { LookupAccountSidW(PCWSTR::null(), sid, Some(PWSTR(name.as_mut_ptr())), &mut n, Some(PWSTR(domain.as_mut_ptr())), &mut d, &mut kind) }.map_err(fail)?;
        let (name, domain) = (String::from_utf16_lossy(&name[..n as usize]), String::from_utf16_lossy(&domain[..d as usize]));
        Ok(if domain.is_empty() { name } else { format!(r"{domain}\{name}") })
    }

    /// The elevated copy (main.rs): our own short-lived process, so set the
    /// COM security Task Scheduler's samples use, then do the job.
    pub fn perform_here(job: &Job) -> windows::core::Result<()> {
        let _com = Com::init();
        let _ = unsafe { CoInitializeSecurity(None, -1, None, None, RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE, None) };
        perform(job)
    }

    /// Do `job` here and now (this process must already be elevated).
    pub fn perform(job: &Job) -> windows::core::Result<()> {
        let _com = Com::init();
        match job {
            Job::RegisterTask { config, user } => crate::background_host::win::register(config, user),
            Job::RemoveTask => crate::background_host::win::remove(),
            Job::FixWebdav => crate::drive::webdav_fix_native(),
        }
    }

    /// Do `job` with administrator rights: in-process when this process
    /// already has them, otherwise DropBeam.exe re-launched elevated (one UAC
    /// prompt). `what` names the part of Windows in error messages.
    pub fn run(job: &Job, what: &str) -> Result<(), String> {
        // The elevated copy will refuse anything `parse` refuses: check first.
        let mut argv = vec![String::new()];
        argv.extend(args_of(job));
        if parse(&argv).as_ref() != Ok(job) {
            return Err(parse(&argv).err().unwrap_or_else(|| "DropBeam can't set this up.".into()));
        }
        if is_elevated() {
            return std::thread::scope(|s| s.spawn(|| perform(job)).join())
                .map_err(|_| "DropBeam couldn't make the change.".to_string())?
                .map_err(|e| format!("{what} said: {}", e.message()));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let file: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt;
            exe.as_os_str().encode_wide().chain([0]).collect()
        };
        let params = wide(&command_line(job));
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
        let process = Handle(info.hProcess);
        if unsafe { WaitForSingleObject(process.0, 120_000) } != WAIT_OBJECT_0 {
            return Err("The change took too long; check again in a moment.".into());
        }
        let mut code = 1u32;
        let _ = unsafe { GetExitCodeProcess(process.0, &mut code) };
        if code == 0 { Ok(()) } else { Err(describe_exit(code, what)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        std::iter::once("DropBeam.exe").chain(a.iter().copied()).map(str::to_owned).collect()
    }

    #[test]
    fn jobs_parse_strictly_and_round_trip() {
        let reg = Job::RegisterTask { config: PathBuf::from(r"C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app"), user: r"PC-1\Ann Lee".into() };
        for job in [reg.clone(), Job::RemoveTask, Job::FixWebdav] {
            let mut a = vec!["DropBeam.exe".to_owned()];
            a.extend(args_of(&job));
            assert!(is_job(&a));
            assert_eq!(parse(&a), Ok(job.clone()));
        }
        assert_eq!(command_line(&reg), r#"--background-task register --config "C:\Users\Ann Lee\AppData\Roaming\com.dropbeam.app" --user "PC-1\Ann Lee""#);
        assert_eq!(command_line(&Job::FixWebdav), "--fix-webdav");
        assert!(!is_job(&argv(&["--server"])) && !is_job(&argv(&[])) && !is_job(&argv(&["cli", "--fix-webdav"])));
        for bad in [
            &["--fix-webdav", "x"][..], &["--background-task"], &["--background-task", "remove", "now"], &["--background-task", "install"],
            &["--background-task", "register", "--config", r"C:\x"], &["--background-task", "register", "--user", "u", "--config", r"C:\x"],
            &["--background-task", "register", "--config", r"C:\x", "--user", "u", "--extra"],
            &["--background-task", "register", "--config", r"relative\dir", "--user", "u"],
            &["--background-task", "register", "--config", r"C:\x", "--user", "a\"b"],
        ] {
            assert!(parse(&argv(bad)).is_err(), "{bad:?}");
        }
        assert_eq!(main_job(&argv(&["--fix-webdav", "x"])), Some(EXIT_BAD_ARGS));
        assert_eq!(main_job(&argv(&["--background-task", "register", "--config", r"C:\does\not\exist-dropbeam", "--user", "u"])), Some(EXIT_BAD_ARGS));
        assert_eq!(main_job(&argv(&["--server"])), None);
    }

    #[test]
    fn folder_and_account_checks() {
        for ok in [r"C:\Users\O'Neil\AppData\Roaming\com.dropbeam.app", r"D:\x", r"\\server\share\dropbeam", r"C:\Ünïcode dir\a b"] {
            assert_eq!(valid_dir(ok), Ok(ok));
        }
        for bad in ["", "C:", "C:x", r"relative\x", r"\x", r"\\?\C:\x", r"\\.\pipe\x", r"C:\a\..\b", r#"C:\a"b"#, "C:\\a\nb", r"C:\a|b",
            r"C:\a*", r"C:\a:stream", "/home/a", r"\\"] {
            assert!(valid_dir(bad).is_err(), "{bad:?}");
        }
        for ok in ["ann", r"PC-1\Ann Lee", r"CORP.EXAMPLE\o'neil", r"WORKGROUP\Noreen_2"] {
            assert_eq!(valid_account(ok), Ok(ok));
        }
        for bad in ["", r"\ann", r"PC\", r"a\b\c", "a\"b", "a\r\nb", "a;b", " ann", "ann ", "a<b", "a/b", r"PC\a*"] {
            assert!(valid_account(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn exit_codes_have_words() {
        assert!(describe_exit(2, "x").contains("refused"));
        assert!(describe_exit(101, "x").contains("code 101"));
        assert!(describe_exit(0x8007_0005, "Windows Task Scheduler").starts_with("Windows Task Scheduler said: "));
    }

    /// Guard: Windows code paths never start a script host or a system tool
    /// to make changes (antivirus deletes apps that do — see the module doc).
    /// Comments may mention them; code (tests included) may not. agent_exec.rs
    /// (the user's own remote commands, in the shell THEY choose) is the one
    /// deliberate exception.
    #[test]
    fn no_script_hosts_in_windows_code() {
        let files = [
            ("elevate.rs", include_str!("elevate.rs")),
            ("background_host.rs", include_str!("background_host.rs")),
            ("drive.rs", include_str!("drive.rs")),
            ("agent_ctl.rs", include_str!("agent_ctl.rs")),
            ("headless.rs", include_str!("headless.rs")),
            ("desktop_shell.rs", include_str!("desktop_shell.rs")),
            ("folder_history.rs", include_str!("folder_history.rs")),
            ("lib.rs", include_str!("lib.rs")),
            ("main.rs", include_str!("main.rs")),
        ];
        // Spelled in pieces so this list doesn't trip the check itself.
        let q = '"';
        let mut needles: Vec<String> = [["power", "shell"], ["pw", "sh"], ["encoded", "command"], ["sch", "tasks"], ["task", "kill"], ["w", "script"],
            ["c", "script"], ["ms", "hta"], ["rund", "ll32"], ["regs", "vr32"]].iter().map(|p| p.concat()).collect();
        needles.extend("net reg sc cmd attrib wmic icacls".split(' ').map(|t| format!("{q}{t}{q}")));
        let mut hits = vec![];
        for (name, src) in files {
            for (i, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("").to_ascii_lowercase();
                for n in &needles {
                    if code.contains(n.as_str()) {
                        hits.push(format!("{name}:{}: {}", i + 1, line.trim()));
                    }
                }
            }
            if matches!(name, "elevate.rs" | "background_host.rs") {
                assert!(!src.contains(&["process::", "Command"].concat()), "{name} must not start child processes");
            }
        }
        assert!(hits.is_empty(), "script host / system tool in Windows code:\n{}", hits.join("\n"));
    }
}
