//! Native macOS "drag a file over the menu-bar icon → the menu pops down"
//! (Blip-style spring-loading).
//!
//! Tauri builds the tray with the `tray-icon` crate, which keeps its
//! `NSStatusItem` private and whose mouse-tracking explicitly does NOT fire
//! while a drag is in progress — so there's no Tauri/safe-Rust way to know a
//! file is being dragged over the icon. We solve it natively: we locate the
//! status-item button (it lives inside `tray-icon`'s `TaoTrayTarget` view),
//! lay a transparent overlay view over it that is registered as a file-drag
//! destination, and on `draggingEntered:` we spring the popover open.
//!
//! Crucially the overlay FORWARDS every mouse event to the real tray view, so
//! clicking / right-clicking the icon behaves exactly as before. If we can't
//! find the button (OS change, timing), we simply do nothing and the tray keeps
//! working — the feature degrades, it never breaks the icon.
#![cfg(target_os = "macos")]

use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSBezierPath, NSColor, NSDragOperation, NSDraggingInfo, NSEvent, NSPasteboard,
    NSView, NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};
use tauri_nspanel::{CollectionBehavior, ManagerExt, PanelLevel, StyleMask, WebviewWindowExt};

// Declare our non-activating panel type (so the menu can float over full-screen
// apps without activating the app or hiding the Dock icon). It lives in its own
// module because the macro pulls in objc2 imports that would otherwise collide
// with this file's imports.
mod db_panel {
    use tauri::Manager;
    tauri_nspanel::tauri_panel! {
        panel!(DropBeamPanel {
            config: {
                can_become_key_window: true,
                is_floating_panel: true
            }
        })
    }
}
use db_panel::DropBeamPanel;

/// Convert the popover window into a non-activating NSPanel — the only kind of
/// window macOS lets float over another app's full-screen Space (since Big Sur).
/// Safe + idempotent: skips if already converted. Preserves the content view and
/// our drag overlay (it's an in-place class swizzle, not a window rebuild).
pub fn convert_popover_to_panel(app: &AppHandle) {
    if app.get_webview_panel("popover").is_ok() {
        return;
    }
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    let Ok(panel) = window.to_panel::<DropBeamPanel>() else {
        log::info!("[traydrag] popover -> panel conversion failed");
        return;
    };
    // Float above normal windows; show on every Space + over full-screen apps.
    panel.set_level(PanelLevel::Floating.value());
    panel.set_style_mask(StyleMask::empty().nonactivating_panel().into());
    panel.set_collection_behavior(
        CollectionBehavior::new()
            .full_screen_auxiliary()
            .can_join_all_spaces()
            .into(),
    );
    log::info!("[traydrag] popover converted to non-activating panel");
}

/// Show the popover — non-activating (orderFrontRegardless) so it appears on the
/// current Space (incl. full-screen) without switching Spaces or stealing focus.
/// Falls back to a normal show if the panel conversion didn't happen.
pub fn show_popover(app: &AppHandle) {
    if let Ok(panel) = app.get_webview_panel("popover") {
        panel.show();
    } else if let Some(w) = app.get_webview_window("popover") {
        let _ = w.show();
    }
}

/// Show the popover AND make it key (for click-to-open, so the search field
/// works and it dismisses on blur). Still non-activating — won't switch Spaces.
pub fn show_popover_key(app: &AppHandle) {
    if let Ok(panel) = app.get_webview_panel("popover") {
        panel.show_and_make_key();
    } else if let Some(w) = app.get_webview_window("popover") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Hide the popover (orderOut).
pub fn hide_popover_window(app: &AppHandle) {
    if let Ok(panel) = app.get_webview_panel("popover") {
        panel.hide();
    } else if let Some(w) = app.get_webview_window("popover") {
        let _ = w.hide();
    }
}

/// App handle for the drag overlays (so their objc methods can open/close the
/// popover without threading closures through everything).
static POPOVER_APP: OnceLock<AppHandle> = OnceLock::new();

/// Bumped on every drag-enter (icon or popover). A scheduled close re-checks it
/// and cancels itself if the drag came back — giving Blip's "drag off → close,
/// drag back → stay open" behavior.
static DRAG_GEN: AtomicU64 = AtomicU64::new(0);

fn bump_drag_gen() {
    DRAG_GEN.fetch_add(1, Ordering::SeqCst);
}

/// Schedule the popover to close shortly, unless the drag re-enters first.
fn schedule_popover_close() {
    let Some(app) = POPOVER_APP.get().cloned() else {
        return;
    };
    let gen = DRAG_GEN.load(Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(260)).await;
        if DRAG_GEN.load(Ordering::SeqCst) == gen {
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || close_popover(&app2));
        }
    });
}

fn close_popover(app: &AppHandle) {
    set_popover_drop_armed(app, false);
    hide_popover_window(app);
}

/// Make the popover appear over full-screen apps and on every Space (otherwise
/// it only drops down when you're not in a full-screen app).
fn configure_popover_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("popover") else {
        return;
    };
    let Ok(ptr) = win.ns_window() else {
        return;
    };
    let ns: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary,
    );
    // Kill the native window shadow — for a transparent window it renders as an
    // ugly square. The rounded shadow comes from the panel's CSS box-shadow,
    // which now has room to render (see the popover-root padding).
    ns.setHasShadow(false);
}

/// A friend row's on-screen rectangle in the popover's CSS pixels, reported by
/// the menu's JS (webview → Rust, which works even while the window is inactive).
/// The native drop handler maps the drop point to a friend using these, so the
/// send happens entirely in Rust — no dependency on the inactive webview.
#[derive(Clone)]
pub struct RowRect {
    pub id: String,
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

static POPOVER_ROWS: Mutex<Vec<RowRect>> = Mutex::new(Vec::new());

/// Called by the `set_popover_rows` command whenever the menu (re)lays out.
pub fn set_rows(rows: Vec<RowRect>) {
    if let Ok(mut g) = POPOVER_ROWS.lock() {
        *g = rows;
    }
}

/// Which friend id is at this drop point (CSS px), with a small nearest-row
/// fallback so an imperfect drop still lands on the right person.
fn friend_at(x: f64, y: f64) -> Option<String> {
    let rows = POPOVER_ROWS.lock().ok()?;
    for r in rows.iter() {
        if x >= r.left && x <= r.right && y >= r.top && y <= r.bottom {
            return Some(r.id.clone());
        }
    }
    // Nearest row whose horizontal band contains x.
    let mut best: Option<(&str, f64)> = None;
    for r in rows.iter() {
        if x < r.left - 24.0 || x > r.right + 24.0 {
            continue;
        }
        let dist = (y - (r.top + r.bottom) / 2.0).abs();
        if best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((&r.id, dist));
        }
    }
    best.filter(|(_, d)| *d < 80.0).map(|(id, _)| id.to_string())
}

/// Send the dropped files to a friend by id — the same logic as the
/// `send_to_friend` command, but callable from the native drop handler so the
/// send never depends on the (inactive) webview.
fn send_drop_to_friend(app: &AppHandle, friend_id: &str, paths: Vec<String>) {
    let paths: Vec<String> = paths
        .into_iter()
        .filter(|p| !p.trim().is_empty() && std::path::Path::new(p).exists())
        .collect();
    if paths.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() else {
        return;
    };
    let Some(iroh) = app.try_state::<std::sync::Arc<crate::iroh_net::IrohState>>() else {
        return;
    };
    let Some(friend) = crate::friends::get(&state.config_dir, friend_id) else {
        return;
    };
    let direct = state.settings.lock().map(|s| s.direct_mode).unwrap_or(true);
    log::info!("[traydrag] sending {} file(s) to friend '{}'", paths.len(), friend.name);

    // Immediate, reliable feedback — fired from Rust, so it shows even though the
    // menu webview was inactive during the drag. (Research: native apps like Blip
    // activate + show a "Sending to X" affordance on drop rather than relying on
    // the inactive web UI.)
    {
        let fname = paths
            .first()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let body = if paths.len() > 1 {
            format!("Sending {} files to {}", paths.len(), friend.name)
        } else {
            format!("Sending {fname} to {}", friend.name)
        };
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("DropBeam").body(body).show();
    }

    if direct && iroh.get().is_some() {
        if let Some(eid) = friend.endpoint_id.clone() {
            let _ = crate::iroh_net::send_to_friend(
                app.clone(),
                iroh.inner().clone(),
                friend.name,
                eid,
                paths,
            );
            return;
        }
    }
    let code = crate::friends::friend_inbox_code(&friend);
    let _ = crate::croc::start_send(
        app.clone(),
        state.inner().clone(),
        paths,
        Some(code),
        Some(friend.name),
    );
}

/// What the overlay needs to do its job: forward mouse events to the real tray
/// view, and (on a drag) open the popover.
pub struct OverlayIvars {
    /// The underlying `TaoTrayTarget` we sit on top of — every mouse event is
    /// handed straight to it so clicks/menus are unchanged.
    target: Retained<NSView>,
    /// Springs the popover open under the icon. Boxed so this objc class stays
    /// non-generic over the Tauri runtime.
    on_drag: Box<dyn Fn()>,
    /// Re-entrancy guard so a drag that lingers over the icon only opens once.
    open_armed: RefCell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "DropBeamTrayDragOverlay"]
    #[ivars = OverlayIvars]
    struct DragOverlay;

    /// File-drag destination: this is the whole point — fire when a file is
    /// dragged over the menu-bar icon.
    impl DragOverlay {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            // A drag is over the icon — cancel any pending close.
            bump_drag_gen();
            if !*self.ivars().open_armed.borrow() {
                *self.ivars().open_armed.borrow_mut() = true;
                (self.ivars().on_drag)();
            }
            // "Copy" gives the user the green “+” drop badge, like Blip.
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            *self.ivars().open_armed.borrow_mut() = false;
            // Drag left the icon. If it doesn't continue into the popover shortly,
            // close the menu (Blip: drag off the icon → it closes). Moving down into
            // the popover bumps the gen and cancels this.
            schedule_popover_close();
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            true
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            // The real drop happens inside the popover (on a person's row); a drop
            // on the bare icon is a no-op we just accept.
            true
        }
    }

    /// Mouse events: forward EVERYTHING to the real tray view so click-to-open
    /// and the right-click menu behave exactly as they did before the overlay.
    impl DragOverlay {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, mouseDown: event]; };
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, mouseUp: event]; };
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, rightMouseDown: event]; };
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, rightMouseUp: event]; };
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, otherMouseDown: event]; };
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            let target = self.ivars().target.clone();
            unsafe { let _: () = msg_send![&*target, otherMouseUp: event]; };
        }
    }
);

impl DragOverlay {
    fn new(mtm: MainThreadMarker, target: Retained<NSView>, on_drag: Box<dyn Fn()>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(OverlayIvars {
            target,
            on_drag,
            open_armed: RefCell::new(false),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// Install the drag-to-open overlay. Best-effort + idempotent: if the tray
/// button isn't found yet (it appears a beat after launch), this just returns
/// and the caller can retry. Safe to call when the feature can't be set up — the
/// tray keeps working untouched.
pub fn install(app: &AppHandle) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(target) = find_tray_target(mtm) else {
        return false;
    };
    // The button is the tray target's superview — overlay the whole button.
    let Some(button) = (unsafe { target.superview() }) else {
        return false;
    };

    // Remember the app handle for the overlays + configure the popover window so
    // it can appear over full-screen apps. Attach the popover's native drop view.
    let _ = POPOVER_APP.set(app.clone());
    configure_popover_window(app);
    attach_popover_drop(app);

    let app = app.clone();
    let on_drag: Box<dyn Fn()> = Box::new(move || {
        spring_popover(&app);
        // Arm the popover's native drop view so a drop on a person lands + sends.
        // It's disarmed by the close-on-drag-off logic, not a timer.
        set_popover_drop_armed(&app, true);
    });
    let overlay = DragOverlay::new(mtm, target.clone(), on_drag);

    // Match the button's bounds and track its size.
    overlay.setFrame(button.bounds());
    use objc2_app_kit::NSAutoresizingMaskOptions;
    overlay.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // Accept file drags (both modern URL + legacy filenames pasteboard types).
    let types = NSArray::from_retained_slice(&[
        NSString::from_str("public.file-url"),
        NSString::from_str("NSFilenamesPboardType"),
    ]);
    overlay.registerForDraggedTypes(&types);
    button.addSubview(&overlay);
    // Keep the overlay alive for the app's lifetime.
    std::mem::forget(overlay);
    true
}

/// Depth-first search of every app window's view tree for `tray-icon`'s
/// `TaoTrayTarget` (the view that backs the menu-bar button).
fn find_tray_target(mtm: MainThreadMarker) -> Option<Retained<NSView>> {
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for window in windows.iter() {
        if let Some(content) = window.contentView() {
            if let Some(found) = search_view(&content) {
                return Some(found);
            }
        }
    }
    None
}

fn search_view(view: &NSView) -> Option<Retained<NSView>> {
    if view.class().name() == c"TaoTrayTarget" {
        return Some(view.retain());
    }
    let subviews = view.subviews();
    for sub in subviews.iter() {
        if let Some(found) = search_view(&sub) {
            return Some(found);
        }
    }
    None
}

/// Open the popover anchored under the menu-bar icon WITHOUT stealing focus, so
/// the in-flight drag survives and can continue down into the popover. (Showing
/// it un-focused also means it won't blur-hide the instant the user moves the
/// drag onto it.)
fn spring_popover(app: &AppHandle) {
    let Some(win) = app.get_webview_window("popover") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        return;
    }
    // Anchor X under the icon: the status-bar button's window frame origin sits
    // at the icon. Y just under the ~24px menu bar.
    let mut x = 200.0_f64;
    if let Some(mtm) = MainThreadMarker::new() {
        if let Some(target) = find_tray_target(mtm) {
            if let Some(w) = target.window() {
                let f = w.frame();
                let scale = win.scale_factor().unwrap_or(1.0);
                // macOS screen points already match Tauri logical points on the
                // primary display; center the popover under the icon.
                x = f.origin.x + f.size.width / 2.0 - 300.0 / 2.0;
                let _ = scale;
            }
        }
    }
    let _ = win.set_position(tauri::LogicalPosition::new(x.max(8.0), 28.0));
    show_popover(app);
    // NOT set_focus(): macOS won't activate the app mid-drag anyway (that's the
    // "grayed out" look). The drop is caught by the popover's native drop view
    // (PopoverDropView), which receives drags while the app is inactive — the
    // webview can't. See attach_popover_drop / set_popover_drop_armed below.
}

// ── Native drop destination over the popover ─────────────────────────────────
//
// The webview can't receive a drop while its window is inactive (a hard
// Tauri/macOS limit), and the app can't activate mid-drag. So we lay a
// transparent NSView drop destination over the popover that receives the drop
// natively (it works inactive — same as the menu-bar overlay), reads the file
// paths, and reports the drop point to the popover JS so it sends to the person
// under the cursor. It's hidden except while a drag is in flight, so it never
// blocks normal clicks in the menu.

pub struct PopoverDropIvars {
    /// Called on drop with (file paths, cssX, cssY) — triggers the send + close.
    on_drop: Box<dyn Fn(Vec<String>, f64, f64)>,
    /// The hovered row's rect in THIS view's coords (bottom-left origin). Drawn
    /// natively as the highlight so it appears live during the drag even while
    /// the menu's web UI is inactive (the whole point — Blip does this natively).
    hover: RefCell<Option<NSRect>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "DropBeamPopoverDrop"]
    #[ivars = PopoverDropIvars]
    struct PopoverDropView;

    impl PopoverDropView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            bump_drag_gen();
            let (x, y) = self.css_point(sender);
            self.update_hover(x, y);
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let (x, y) = self.css_point(sender);
            self.update_hover(x, y);
            NSDragOperation::Copy
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            true
        }

        #[unsafe(method(performDragOperation:))]
        fn perform(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let (x, y) = self.css_point(sender);
            let paths = drag_paths(sender);
            log::info!("[traydrag] popover performDrag ({x:.0},{y:.0}) paths={}", paths.len());
            (self.ivars().on_drop)(paths, x, y);
            self.clear_hover();
            self.setHidden(true);
            true
        }

        #[unsafe(method(draggingExited:))]
        fn exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            // Drag left the menu without dropping — close it shortly unless it
            // comes back (Blip behavior).
            self.clear_hover();
            schedule_popover_close();
        }

        #[unsafe(method(draggingEnded:))]
        fn ended(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) {
            self.clear_hover();
            self.setHidden(true);
        }

        /// Native highlight: paint a soft rounded rectangle over the hovered row.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let Some(rect) = *self.ivars().hover.borrow() else {
                return;
            };
            // Match the web ".drop" style: accent (#5b5bf0) 14% fill + 2px border.
            let fill = NSColor::colorWithSRGBRed_green_blue_alpha(0.357, 0.357, 0.941, 0.14);
            let stroke = NSColor::colorWithSRGBRed_green_blue_alpha(0.357, 0.357, 0.941, 0.9);
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 9.0, 9.0);
            fill.setFill();
            path.fill();
            stroke.setStroke();
            path.setLineWidth(2.0);
            path.stroke();
        }
    }
);

impl PopoverDropView {
    fn new(
        mtm: MainThreadMarker,
        on_drop: Box<dyn Fn(Vec<String>, f64, f64)>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PopoverDropIvars {
            on_drop,
            hover: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// The current drag location in the popover's CSS pixels (top-left origin),
    /// matching what `getBoundingClientRect()` uses in the webview.
    fn css_point(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> (f64, f64) {
        let win_pt = sender.draggingLocation();
        let local = self.convertPoint_fromView(win_pt, None);
        let h = self.bounds().size.height;
        (local.x, h - local.y)
    }

    /// Recompute the hovered-row highlight rect from the reported rows and ask for
    /// a redraw. Inset a touch so it reads like a row pill (Blip-style).
    fn update_hover(&self, x: f64, y: f64) {
        let h = self.bounds().size.height;
        let next = POPOVER_ROWS.lock().ok().and_then(|rows| {
            rows.iter()
                .find(|r| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
                .map(|r| {
                    let width = (r.right - r.left - 12.0).max(0.0);
                    let height = (r.bottom - r.top - 2.0).max(0.0);
                    // CSS top-left → view bottom-left.
                    NSRect::new(NSPoint::new(r.left + 6.0, h - r.bottom + 1.0), NSSize::new(width, height))
                })
        });
        let mut cur = self.ivars().hover.borrow_mut();
        *cur = next;
        drop(cur);
        self.setNeedsDisplay(true);
    }

    fn clear_hover(&self) {
        *self.ivars().hover.borrow_mut() = None;
        self.setNeedsDisplay(true);
    }
}

/// Read dropped file paths from the drag pasteboard (legacy filenames type —
/// simplest and still supported).
fn drag_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<String> {
    let pb: Retained<NSPasteboard> = sender.draggingPasteboard();
    let ty = NSString::from_str("NSFilenamesPboardType");
    let Some(plist) = pb.propertyListForType(&ty) else {
        return Vec::new();
    };
    // The plist for NSFilenamesPboardType is an NSArray of path strings.
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

/// Attach the (hidden) native drop view over the popover's content view. Safe to
/// call repeatedly — it won't double-attach.
fn attach_popover_drop(app: &AppHandle) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(content) = popover_content_view(app) else {
        log::info!("[traydrag] attach_popover_drop: no popover content view");
        return false;
    };
    log::info!(
        "[traydrag] attach: content class={:?} subviews={}",
        content.class().name(),
        content.subviews().len()
    );
    if search_named(&content, "DropBeamPopoverDrop").is_some() {
        log::info!("[traydrag] attach: already attached");
        return true;
    }
    let app_drop = app.clone();
    let view = PopoverDropView::new(
        mtm,
        // Highlight is drawn natively (so it shows on the first pass while the menu
        // is inactive); the send happens entirely in Rust.
        Box::new(move |paths, x, y| {
            match friend_at(x, y) {
                Some(id) => send_drop_to_friend(&app_drop, &id, paths),
                None => log::info!("[traydrag] drop ({x:.0},{y:.0}) hit no friend row"),
            }
            // Blip behavior: close the menu after the drop. The native "Sending to
            // X" notification is the confirmation.
            close_popover(&app_drop);
        }),
    );
    view.setHidden(true);
    view.setFrame(content.bounds());
    use objc2_app_kit::NSAutoresizingMaskOptions;
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let types = NSArray::from_retained_slice(&[
        NSString::from_str("public.file-url"),
        NSString::from_str("NSFilenamesPboardType"),
    ]);
    view.registerForDraggedTypes(&types);
    content.addSubview(&view);
    log::info!(
        "[traydrag] attach: added drop view; content now {} subviews",
        content.subviews().len()
    );
    std::mem::forget(view);
    true
}

/// Show/hide the popover's native drop view. Armed only during a drag so it
/// never intercepts normal clicks in the menu.
fn set_popover_drop_armed(app: &AppHandle, armed: bool) {
    // (Re)attach in case it wasn't ready at install time.
    if armed {
        attach_popover_drop(app);
    }
    if let Some(content) = popover_content_view(app) {
        if let Some(view) = search_named(&content, "DropBeamPopoverDrop") {
            if armed {
                // Re-cover the whole menu now (the window may have just been shown
                // / resized) so the very FIRST drag is tracked across the full
                // height — fixes the "have to do it twice" symptom.
                view.setFrame(content.bounds());
            }
            view.setHidden(!armed);
            log::info!("[traydrag] set_popover_drop_armed: armed={armed} (view found)");
            return;
        }
    }
    log::info!("[traydrag] set_popover_drop_armed: armed={armed} but view NOT found");
}

/// The popover window's content view, via Tauri's raw `ns_window` handle.
fn popover_content_view(app: &AppHandle) -> Option<Retained<NSView>> {
    let win = app.get_webview_window("popover")?;
    let ptr = win.ns_window().ok()?;
    let ns_window: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    ns_window.contentView()
}

/// Depth-first search for a subview of a given class name.
fn search_named(view: &NSView, name: &str) -> Option<Retained<NSView>> {
    if view.class().name().to_str().ok() == Some(name) {
        return Some(view.retain());
    }
    for sub in view.subviews().iter() {
        if let Some(found) = search_named(&sub, name) {
            return Some(found);
        }
    }
    None
}
