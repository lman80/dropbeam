//! Desktop shell glue that differs per OS: where the tray popover goes, whether a
//! tray exists at all (Linux), keeping the login item pointed at this binary, and
//! `dropbeam:` deep links. Kept out of lib.rs so the platform branches live in one
//! place and the pure parts are unit-testable.

/// A rectangle in physical pixels (a monitor's work area: the screen minus the
/// taskbar / panels / menu bar).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Where to put a `pop_w`×`pop_h` popover for a tray click at (`cx`, `cy`),
/// everything in physical pixels. The popover is centred on the click, kept
/// `margin` inside the work area, and opens ABOVE the click when the click is in
/// the lower half (a bottom taskbar, the Windows default) — below it otherwise
/// (top panels, the macOS menu bar). Left/right taskbars are handled by the clamp.
pub fn popover_origin(cx: f64, cy: f64, pop_w: f64, pop_h: f64, margin: f64, wa: Rect) -> (f64, f64) {
    let clamp = |v: f64, lo: f64, hi: f64| if hi < lo { lo } else { v.max(lo).min(hi) };
    let x = clamp(cx - pop_w / 2.0, wa.x + margin, wa.x + wa.w - pop_w - margin);
    let below = cy < wa.y + wa.h / 2.0;
    let y = if below { cy.max(wa.y) + margin } else { cy.min(wa.y + wa.h) - pop_h - margin };
    let y = clamp(y, wa.y + margin, wa.y + wa.h - pop_h - margin);
    (x, y)
}

/// Is there something on screen that shows tray icons? Always true on macOS and
/// Windows. On Linux the tray icon is a StatusNotifierItem, which only appears if a
/// StatusNotifierWatcher owns its D-Bus name — stock GNOME has none (the
/// AppIndicator extension adds one). Without it, hiding the window on close or
/// starting hidden at login leaves an app the user can't see or reach.
/// `DROPBEAM_ASSUME_TRAY=1` / `=0` overrides the probe.
#[cfg(desktop)]
pub fn tray_host_available() -> bool {
    match std::env::var("DROPBEAM_ASSUME_TRAY").ok().as_deref() {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    #[cfg(target_os = "linux")]
    {
        use gtk::gio;
        use gtk::glib::{self, ToVariant};
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return false;
        };
        let reply = bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
            Some(glib::VariantTy::new("(b)").expect("valid variant type")),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        );
        match reply.ok().and_then(|v| v.get::<(bool,)>()) {
            Some((owned,)) => owned,
            None => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// The path the login item should launch: the same value
/// tauri-plugin-autostart writes (canonical exe on macOS, the AppImage file on
/// Linux when running from one, the exe otherwise).
#[cfg(desktop)]
fn expected_autostart_path() -> Option<String> {
    #[cfg(target_os = "linux")]
    if let Some(p) = std::env::var_os("APPIMAGE") {
        return Some(p.to_string_lossy().into_owned());
    }
    let exe = std::env::current_exe().ok()?;
    #[cfg(target_os = "macos")]
    let exe = exe.canonicalize().ok()?;
    Some(exe.display().to_string())
}

/// The command line currently registered as DropBeam's login item, if any.
#[cfg(desktop)]
fn registered_autostart_command(app_name: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        let file = std::path::Path::new(&home).join("Library/LaunchAgents").join(format!("{app_name}.plist"));
        std::fs::read_to_string(file).ok()
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME")?;
        let file = std::path::Path::new(&home).join(".config/autostart").join(format!("{app_name}.desktop"));
        std::fs::read_to_string(file).ok()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("reg")
            .args(["query", r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "/v", app_name])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// True when a login item exists but launches a DIFFERENT binary than this one
/// (the app was moved, or an AppImage was replaced by a newer file name). Never
/// true for development builds, so running from `target/` can't steal the
/// installed app's login item.
#[cfg(desktop)]
pub fn autostart_entry_stale(app_name: &str) -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    let Some(expected) = expected_autostart_path() else { return false };
    if expected.contains("/target/") || expected.contains("\\target\\") {
        return false;
    }
    match registered_autostart_command(app_name) {
        Some(cmd) => !cmd.contains(&expected),
        None => false,
    }
}

/// `dropbeam:` links the desktop app was asked to open (browser link, another
/// app, a second launch with the URL in argv). Only the scheme is checked here;
/// the UI parses the code and ASKS before acting on it, exactly like a pasted or
/// scanned code — nothing is executed from a link on its own.
pub fn dropbeam_links<'a>(urls: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    urls.into_iter()
        .map(str::trim)
        .filter(|u| u.len() <= 8192 && u.get(..9).is_some_and(|p| p.eq_ignore_ascii_case("dropbeam:")))
        .map(str::to_string)
        .collect()
}

/// A local path from a launch argument: a plain absolute path, or a `file://`
/// URI (Linux file managers pass those through `Exec=… %U`). Files and folders
/// both count — a folder is sent like a dropped folder.
pub fn local_path_from_arg(arg: &str) -> Option<String> {
    let path = if let Some(rest) = arg.strip_prefix("file://") {
        // file:///abs/path or file://localhost/abs/path
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        percent_encoding::percent_decode_str(rest).decode_utf8().ok()?.into_owned()
    } else {
        arg.to_string()
    };
    let p = std::path::Path::new(&path);
    (p.is_absolute() && p.exists()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WA: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 }; // 40 px taskbar at the bottom

    #[test]
    fn bottom_taskbar_opens_above_and_stays_on_screen() {
        // Tray icon near the bottom-right corner (Windows default).
        let (x, y) = popover_origin(1880.0, 1060.0, 300.0, 390.0, 8.0, WA);
        assert_eq!(x, 1920.0 - 300.0 - 8.0);
        assert_eq!(y, 1040.0 - 390.0 - 8.0);
    }

    #[test]
    fn top_panel_opens_below() {
        let wa = Rect { x: 0.0, y: 28.0, w: 1920.0, h: 1052.0 };
        let (x, y) = popover_origin(960.0, 10.0, 300.0, 390.0, 8.0, wa);
        assert_eq!(x, 810.0);
        assert_eq!(y, 28.0 + 8.0);
    }

    #[test]
    fn left_taskbar_and_second_monitor_clamp() {
        let wa = Rect { x: 1920.0 + 48.0, y: 0.0, w: 2560.0 - 48.0, h: 1440.0 };
        let (x, y) = popover_origin(1920.0 + 20.0, 1400.0, 300.0, 390.0, 8.0, wa);
        assert_eq!(x, 1920.0 + 48.0 + 8.0);
        // Lower half → opens above the icon, not pinned to the bottom edge.
        assert_eq!(y, 1400.0 - 390.0 - 8.0);
    }

    #[test]
    fn tiny_work_area_never_panics() {
        let wa = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let (x, y) = popover_origin(100.0, 190.0, 300.0, 390.0, 8.0, wa);
        assert_eq!((x, y), (8.0, 8.0));
    }

    #[test]
    fn only_dropbeam_links_pass() {
        let got = dropbeam_links(["dropbeam:ABC", "DropBeam://add?code=x", "https://evil", "dropbea", " dropbeam:x "]);
        assert_eq!(got, vec!["dropbeam:ABC", "DropBeam://add?code=x", "dropbeam:x"]);
    }

    #[test]
    fn file_uri_args_decode() {
        let dir = std::env::temp_dir().join("db shell test");
        std::fs::create_dir_all(&dir).unwrap();
        let s = dir.display().to_string();
        assert_eq!(local_path_from_arg(&s).as_deref(), Some(s.as_str()));
        #[cfg(unix)]
        assert_eq!(local_path_from_arg(&format!("file://{}", s.replace(' ', "%20"))).as_deref(), Some(s.as_str()));
        assert_eq!(local_path_from_arg("--minimized"), None);
        assert_eq!(local_path_from_arg("relative/thing"), None);
        let _ = std::fs::remove_dir(&dir);
    }
}
