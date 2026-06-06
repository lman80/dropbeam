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

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{NSApplication, NSDragOperation, NSDraggingInfo, NSEvent, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSString};
use tauri::{AppHandle, Manager, Runtime};

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
            // Re-arm so the next drag opens again. We do NOT hide the popover here:
            // exiting the icon is exactly when the user is moving the drag down into
            // the popover to drop on a person.
            *self.ivars().open_armed.borrow_mut() = false;
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
pub fn install<R: Runtime>(app: &AppHandle<R>) -> bool {
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

    let app = app.clone();
    let on_drag: Box<dyn Fn()> = Box::new(move || spring_popover(&app));
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
fn spring_popover<R: Runtime>(app: &AppHandle<R>) {
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
    let _ = win.show();
    // Deliberately NOT set_focus(): keep the drag session alive.
}
