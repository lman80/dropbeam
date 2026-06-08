#![cfg(target_os = "macos")]
//! macOS Finder "Share with DropBeam" via the system Services menu.
//!
//! The service is declared statically in `src-tauri/Info.plist` (the `NSServices`
//! array, which Tauri v2 merges into the bundle's Info.plist at build time). When
//! the user right-clicks a file → Services → "Share with DropBeam", macOS hands
//! our already-running app an `NSPasteboard` of the selected file URLs. We forward
//! the first file into the SAME send flow the Windows right-click + cold-start use
//! (`LAUNCH_FILE` + the `open-file-send` event), so there's no new UI to build.
//!
//! Notes: the app must live in /Applications for `pbs` to index the service, it
//! appears under right-click → Services (not top-level), and may need a login or
//! a one-time toggle in System Settings ▸ Keyboard ▸ Services on first install.
//! Mirrors the objc2 patterns in `tray_drag.rs` so signatures match our pinned
//! crate versions.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_app_kit::{NSApplication, NSPasteboard};
use objc2_foundation::{MainThreadMarker, NSArray, NSString};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager};

/// The running app handle, so the service callback (an AppKit method) can reach
/// Tauri — the same trick the drag overlays use with `POPOVER_APP`.
static SERVICE_APP: OnceLock<AppHandle> = OnceLock::new();

struct ServiceIvars;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DropBeamServiceProvider"]
    #[ivars = ServiceIvars]
    struct ServiceProvider;

    impl ServiceProvider {
        // The selector MUST equal the Info.plist NSMessage ("shareWithDropBeam")
        // plus the fixed Services suffix ":userData:error:".
        #[unsafe(method(shareWithDropBeam:userData:error:))]
        fn share_with_dropbeam(
            &self,
            pboard: &NSPasteboard,
            _user_data: *mut NSString,
            _error: *mut *mut NSString,
        ) {
            let paths = read_paths(pboard);
            log::info!("[service] Share with DropBeam: {} path(s)", paths.len());
            if let Some(app) = SERVICE_APP.get() {
                deliver(app, paths);
            }
        }
    }
);

impl ServiceProvider {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(ServiceIvars);
        unsafe { msg_send![super(this), init] }
    }
}

/// Read file paths off the service pasteboard — the same `NSFilenamesPboardType`
/// → `NSArray<NSString>` read that `tray_drag::drag_paths` uses for drops.
fn read_paths(pb: &NSPasteboard) -> Vec<String> {
    let ty = NSString::from_str("NSFilenamesPboardType");
    let Some(plist) = pb.propertyListForType(&ty) else {
        return Vec::new();
    };
    let Ok(arr) = plist.downcast::<NSArray<AnyObject>>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..arr.count() {
        if let Ok(s) = arr.objectAtIndex(i).downcast::<NSString>() {
            out.push(s.to_string());
        }
    }
    out
}

fn deliver(app: &AppHandle, paths: Vec<String>) {
    // Services may not bring us forward — do it ourselves.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(first) = paths.into_iter().find(|p| std::path::Path::new(p).is_file()) {
        crate::set_launch_file(first.clone());
        let _ = app.emit("open-file-send", first);
    }
}

/// Register the Services provider with AppKit. Call once, on the main thread.
pub fn install(app: &AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("[service] not on main thread; skipping service registration");
        return;
    };
    let _ = SERVICE_APP.set(app.clone());
    let provider = ServiceProvider::new();
    let ns_app = NSApplication::sharedApplication(mtm);
    unsafe {
        ns_app.setServicesProvider(Some(&provider));
    }
    // Keep the provider alive for the app's lifetime (same as the forgotten
    // overlay views in tray_drag.rs).
    std::mem::forget(provider);
    log::info!("[service] Services provider registered");
}
