//! Desktop chat notifications you can click (GitHub #67).
//!
//! The notification plugin's desktop path is fire-and-forget (notify-rust), so a
//! click never reached the app. Chat banners go through a native path that
//! reports the click instead:
//!   • macOS: UNUserNotificationCenter with our own delegate. Only inside a real
//!     .app bundle — the framework throws for a bare binary (`tauri dev`), so
//!     there we fall back to the plugin (banner still shows, click does nothing).
//!   • Windows: a WinRT toast with an `Activated` handler.
//!   • Linux: a freedesktop notification with a "default" action, waited on from
//!     a short-lived thread (it ends when the banner is clicked or closed).
//! A click brings the main window forward and emits `chat-notification-open`
//! `{ peerId }`; the frontend opens that conversation.

use tauri::{AppHandle, Emitter};

pub const OPEN_EVENT: &str = "chat-notification-open";

/// A click on a chat notification: show + focus the main window, open the chat.
pub fn open_chat(app: &AppHandle, peer_id: &str) {
    log::info!("chat notification clicked: opening {peer_id}");
    crate::show_main_window(app);
    let _ = app.emit(OPEN_EVENT, serde_json::json!({ "peerId": peer_id }));
}

/// Show a clickable chat notification. `false` = this platform path isn't
/// available right now; the caller shows the plain plugin banner instead.
pub fn show(app: &AppHandle, title: &str, body: &str, peer_id: &str) -> bool {
    imp::show(app, title, body, peer_id)
}

/// Request-identifier / tag round trip: `chat:<peer>:<unique>` → `<peer>`.
fn peer_from_id(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("chat:")?;
    let (peer, _) = rest.rsplit_once(':')?;
    (!peer.is_empty()).then_some(peer)
}

fn new_id(peer_id: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("chat:{peer_id}:{n}")
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use std::sync::OnceLock;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "DropBeamChatNotificationDelegate"]
        struct ChatNoteDelegate;

        unsafe impl NSObjectProtocol for ChatNoteDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for ChatNoteDelegate {
            /// Show the banner even while DropBeam is frontmost — the caller
            /// already skipped the one case that should stay quiet (you're
            /// reading that very thread).
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let action = response.actionIdentifier().to_string();
                let id = response.notification().request().identifier().to_string();
                if action != "com.apple.UNNotificationDismissActionIdentifier" {
                    if let (Some(peer), Some(app)) = (peer_from_id(&id), APP.get()) {
                        let (app, peer) = (app.clone(), peer.to_string());
                        let main = app.clone();
                        let _ = main.run_on_main_thread(move || open_chat(&app, &peer));
                    }
                }
                handler.call(());
            }
        }
    );

    impl ChatNoteDelegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    /// UNUserNotificationCenter raises for a process that isn't a bundled app.
    fn bundled() -> bool {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
    }

    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        static READY: OnceLock<bool> = OnceLock::new();
        let ok = *READY.get_or_init(|| {
            if !bundled() {
                log::info!("chat notifications: not a bundled app, clicks can't be routed");
                return false;
            }
            let center = UNUserNotificationCenter::currentNotificationCenter();
            // The center keeps a WEAK delegate reference: keep ours alive forever.
            let delegate: &'static ChatNoteDelegate = Box::leak(Box::new(ChatNoteDelegate::new()));
            center.setDelegate(Some(ProtocolObject::from_ref(delegate)));
            true
        });
        ok.then(UNUserNotificationCenter::currentNotificationCenter)
    }

    pub fn show(app: &AppHandle, title: &str, body: &str, peer_id: &str) -> bool {
        let _ = APP.set(app.clone());
        let Some(center) = center() else { return false };
        let (title, body, peer) = (title.to_string(), body.to_string(), peer_id.to_string());
        let fallback_app = app.clone();
        // Asking again is free once decided (no prompt), and it sequences the
        // post after the very first grant instead of racing it.
        let post = RcBlock::new(move |granted: Bool, _err: *mut NSError| {
            if !granted.as_bool() {
                log::info!("chat notifications: not authorized, using the plain banner");
                plain(&fallback_app, &title, &body);
                return;
            }
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(&title));
            content.setBody(&NSString::from_str(&body));
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            content.setThreadIdentifier(&NSString::from_str(&peer));
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                &NSString::from_str(&new_id(&peer)),
                &content,
                None,
            );
            let peer_log = peer.clone();
            let done = RcBlock::new(move |err: *mut NSError| {
                if !err.is_null() {
                    log::warn!("chat notification for {peer_log} was refused by the system");
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .addNotificationRequest_withCompletionHandler(&request, Some(&done));
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
            &post,
        );
        true
    }

    fn plain(app: &AppHandle, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title(title).body(body).sound("default").show();
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use tauri_winrt_notification::{Sound, Toast};

    pub fn show(app: &AppHandle, title: &str, body: &str, peer_id: &str) -> bool {
        // Only the installed app has a Start-menu AppUserModelID to post as.
        let installed = tauri::utils::platform::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.display().to_string()))
            .map(|dir| !(dir.ends_with("target\\debug") || dir.ends_with("target\\release")))
            .unwrap_or(false);
        if !installed {
            return false;
        }
        let (app, peer) = (app.clone(), peer_id.to_string());
        let result = Toast::new(&app.config().identifier.clone())
            .title(title)
            .text1(body)
            .sound(Some(Sound::Default))
            .on_activated(move |_action| {
                let (a, p) = (app.clone(), peer.clone());
                let _ = app.run_on_main_thread(move || open_chat(&a, &p));
                Ok(())
            })
            .show();
        match result {
            Ok(()) => true,
            Err(err) => {
                log::warn!("chat toast failed ({err:?}); using the plain banner");
                false
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn show(app: &AppHandle, title: &str, body: &str, peer_id: &str) -> bool {
        let mut n = notify_rust::Notification::new();
        n.appname("DropBeam")
            .summary(title)
            .body(body)
            .sound_name("message-new-instant")
            .auto_icon()
            // "default" = a click on the notification body itself.
            .action("default", "Open");
        let handle = match n.show() {
            Ok(h) => h,
            Err(err) => {
                log::warn!("chat notification failed ({err}); using the plain banner");
                return false;
            }
        };
        let (app, peer) = (app.clone(), peer_id.to_string());
        // Returns when the banner is clicked, dismissed or expires.
        let _ = std::thread::Builder::new().name("chat-notify".into()).spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    let (a, p) = (app.clone(), peer.clone());
                    let _ = app.run_on_main_thread(move || open_chat(&a, &p));
                }
            });
        });
        true
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::*;
    pub fn show(_app: &AppHandle, _title: &str, _body: &str, _peer_id: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_round_trips_through_the_notification_id() {
        let id = new_id("41d75e10-1d21-456e-9cc5-2cc2459f9d63");
        assert_eq!(peer_from_id(&id), Some("41d75e10-1d21-456e-9cc5-2cc2459f9d63"));
        assert_eq!(peer_from_id("chat::1"), None);
        assert_eq!(peer_from_id("transfer:x:1"), None);
        assert_eq!(peer_from_id("chat:abc"), None);
    }
}
