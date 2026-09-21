//! UIKit navigation, owned and mutated exclusively on the main thread.
#![cfg(target_os = "ios")]

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{class, define_class, msg_send, AllocAnyThread, DefinedClass, Encode, Encoding};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use objc2_ui_kit::UIEdgeInsets;
use std::{cell::RefCell, sync::Mutex};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const TAB_HEIGHT: f64 = 49.0;

// CoreGraphics' by-value ABI, without enabling the typed UIView bindings.
#[repr(C)]
#[derive(Clone, Copy)]
struct Transform { a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64 }
unsafe impl Encode for Transform {
    const ENCODING: Encoding = Encoding::Struct("CGAffineTransform", &[f64::ENCODING; 6]);
}

struct TabBarIvars { app: Mutex<AppHandle> }
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DropBeamTabBarDelegate"]
    #[ivars = TabBarIvars]
    struct DropBeamTabBarDelegate;
    impl DropBeamTabBarDelegate {
        #[unsafe(method(tabBar:didSelectItem:))]
        fn selected(&self, _bar: &AnyObject, item: &AnyObject) {
            let tag: isize = unsafe { msg_send![item, tag] };
            let _ = self.ivars().app.lock().unwrap().emit("native-tab", tag as i32);
        }
    }
);

struct NativeTabBar {
    bar: Retained<AnyObject>,
    root: Retained<AnyObject>,
    items: Retained<NSArray<AnyObject>>,
    // UITabBar.delegate is weak.
    _delegate: Retained<DropBeamTabBarDelegate>,
}
thread_local! {
    static TAB_BAR: RefCell<Option<NativeTabBar>> = const { RefCell::new(None) };
}

// Same key-window/root lookup as ios_media::presenter; a presented sheet does
// not prevent installing navigation underneath it.
unsafe fn presenter() -> Result<(Retained<AnyObject>, Retained<AnyObject>), String> {
    let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
    let windows: Retained<NSArray<AnyObject>> = msg_send![&*app, windows];
    for window in windows.iter() {
        let key: bool = msg_send![&*window, isKeyWindow];
        if !key { continue; }
        let root: Option<Retained<AnyObject>> = msg_send![&*window, rootViewController];
        if let Some(root) = root { return Ok((window.clone(), root)); }
    }
    Err("No active iOS window.".into())
}

fn insets(bottom: f64) -> UIEdgeInsets {
    UIEdgeInsets { top: 0.0, left: 0.0, bottom, right: 0.0 }
}

async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = oneshot::channel();
    app.run_on_main_thread(move || { let _ = tx.send(work()); }).map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn native_tabbar_install(app: AppHandle) -> Result<f64, String> {
    let handle = app.clone();
    on_main(&app, move || TAB_BAR.with(|slot| unsafe {
        let mut slot = slot.borrow_mut();
        if slot.is_some() { return Ok(TAB_HEIGHT); }
        let (window, root) = presenter()?;
        // The bar lives on the window, not the root view: Tauri's webview is a
        // sibling added later than any root-view subview would be, so it would
        // otherwise cover the bar.
        let view: Retained<AnyObject> = window.clone();
        let bounds: NSRect = msg_send![&*view, bounds];
        let safe: UIEdgeInsets = msg_send![&*view, safeAreaInsets];
        log::info!("native tab bar: window bounds {:?}x{:?} safe bottom {}", bounds.size.width, bounds.size.height, safe.bottom);
        let height = TAB_HEIGHT + safe.bottom;
        let frame = NSRect::new(
            NSPoint::new(bounds.origin.x, bounds.origin.y + bounds.size.height - height),
            NSSize::new(bounds.size.width, height),
        );
        let bar: Allocated<AnyObject> = msg_send![class!(UITabBar), alloc];
        let bar: Retained<AnyObject> = msg_send![bar, initWithFrame: frame];
        let _: () = msg_send![&*bar, setAutoresizingMask: (2usize | 8usize)];
        let _: () = msg_send![&*bar, setItemPositioning: 1isize]; // UITabBarItemPositioningFill
        let mut items = Vec::new();
        for (tag, (title, symbol)) in [
            ("Send", "paperplane.fill"), ("Friends", "person.2.fill"),
            ("Chat", "bubble.left.and.bubble.right.fill"),
            ("History", "clock.fill"), ("Settings", "gearshape.fill"),
        ].into_iter().enumerate() {
            let image: Option<Retained<AnyObject>> = msg_send![class!(UIImage), systemImageNamed: &*NSString::from_str(symbol)];
            let item: Allocated<AnyObject> = msg_send![class!(UITabBarItem), alloc];
            let item: Retained<AnyObject> = msg_send![item, initWithTitle: &*NSString::from_str(title), image: image.as_deref(), tag: tag as isize];
            items.push(item);
        }
        let items = NSArray::from_retained_slice(&items);
        let _: () = msg_send![&*bar, setItems: &*items];
        let _: () = msg_send![&*bar, setSelectedItem: &*items.objectAtIndex(0)];
        let provider = RcBlock::new(|traits: *mut AnyObject| -> *mut AnyObject {
            let style: isize = msg_send![traits, userInterfaceStyle];
            let (r, g, b) = if style == 2 { (124.0, 124.0, 255.0) } else { (91.0, 91.0, 240.0) };
            // The provider returns an autoreleased UIColor (+0).
            msg_send![class!(UIColor), colorWithRed: r / 255.0, green: g / 255.0, blue: b / 255.0, alpha: 1.0f64]
        });
        let tint: Retained<AnyObject> = msg_send![class!(UIColor), colorWithDynamicProvider: &*provider];
        let _: () = msg_send![&*bar, setTintColor: &*tint];
        let delegate = DropBeamTabBarDelegate::alloc().set_ivars(TabBarIvars { app: Mutex::new(handle) });
        let delegate: Retained<DropBeamTabBarDelegate> = msg_send![super(delegate), init];
        let _: () = msg_send![&*bar, setDelegate: &*delegate];
        let _: () = msg_send![&*view, addSubview: &*bar];
        let _: () = msg_send![&*view, bringSubviewToFront: &*bar];
        let _: () = msg_send![&*root, setAdditionalSafeAreaInsets: insets(TAB_HEIGHT)];
        *slot = Some(NativeTabBar { bar, root, items, _delegate: delegate });
        Ok(TAB_HEIGHT)
    })).await
}

#[tauri::command]
pub async fn native_tabbar_select(app: AppHandle, index: i32) -> Result<(), String> {
    on_main(&app, move || TAB_BAR.with(|slot| unsafe {
        let slot = slot.borrow();
        let tab = slot.as_ref().ok_or("Native tab bar is not installed.")?;
        if !(0..5).contains(&index) { return Err("Invalid tab index.".into()); }
        let _: () = msg_send![&*tab.bar, setSelectedItem: &*tab.items.objectAtIndex(index as usize)];
        Ok(())
    })).await
}

#[tauri::command]
pub async fn native_tabbar_badge(app: AppHandle, index: i32, count: i32) -> Result<(), String> {
    on_main(&app, move || TAB_BAR.with(|slot| unsafe {
        let slot = slot.borrow();
        let tab = slot.as_ref().ok_or("Native tab bar is not installed.")?;
        if !(0..5).contains(&index) { return Err("Invalid tab index.".into()); }
        let badge = if count <= 0 { None } else {
            Some(NSString::from_str(&if count > 99 { "99+".into() } else { count.to_string() }))
        };
        let _: () = msg_send![&*tab.items.objectAtIndex(index as usize), setBadgeValue: badge.as_deref()];
        Ok(())
    })).await
}

#[tauri::command]
pub async fn native_tabbar_hidden(app: AppHandle, hidden: bool) -> Result<(), String> {
    on_main(&app, move || TAB_BAR.with(|slot| unsafe {
        let slot = slot.borrow();
        let tab = slot.as_ref().ok_or("Native tab bar is not installed.")?;
        let bar = tab.bar.clone();
        let root = tab.root.clone();
        let bounds: NSRect = msg_send![&*bar, bounds];
        let _: () = msg_send![&*bar, setUserInteractionEnabled: !hidden];
        let animation = RcBlock::new(move || {
            let _: () = msg_send![&*bar, setAlpha: if hidden { 0.0f64 } else { 1.0f64 }];
            let _: () = msg_send![&*bar, setTransform: Transform {
                a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0,
                ty: if hidden { bounds.size.height } else { 0.0 },
            }];
            let _: () = msg_send![&*root, setAdditionalSafeAreaInsets: insets(if hidden { 0.0 } else { TAB_HEIGHT })];
        });
        let _: () = msg_send![class!(UIView), animateWithDuration: 0.2f64, delay: 0.0f64,
            options: 4usize, animations: &*animation, completion: std::ptr::null::<AnyObject>()]; // BeginFromCurrentState
        Ok(())
    })).await
}
