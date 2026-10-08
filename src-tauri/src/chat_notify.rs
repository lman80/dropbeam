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
//!     a thread that ends when the banner is clicked or closed. At most
//!     `MAX_LIVE_BANNERS` banners stay clickable: a newer one closes the oldest
//!     (which ends its thread), and a hard cap on waiter threads backs that up,
//!     so a long chat burst can't pile up blocked threads.
//! A click brings the main window forward and emits `chat-notification-open`
//! `{ peerId }`; the frontend opens that conversation.
//!
//! `init` must run once at startup (from `setup`): on macOS the click delegate
//! has to be installed BEFORE the app finishes launching, or a click on a banner
//! delivered by a previous run (or the click that relaunched the app) is dropped.

use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const OPEN_EVENT: &str = "chat-notification-open";

/// After a cold launch the webview isn't listening yet, so a click that arrives
/// in this window after `init` is re-announced a few times (opening the same
/// chat again is harmless).
const LAUNCH_GRACE: Duration = Duration::from_secs(20);
const LAUNCH_REPLAY_MS: &[u64] = &[1_500, 4_000, 8_000];

fn started() -> &'static std::sync::OnceLock<Instant> {
    static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    &STARTED
}

/// Register the platform click handler. Call once from `setup` (main thread).
pub fn init(app: &AppHandle) {
    let _ = started().set(Instant::now());
    imp::init(app);
}

/// Re-emit delays for a click `since_init` after startup (empty once warm).
fn replay_delays(since_init: Option<Duration>) -> &'static [u64] {
    match since_init {
        Some(d) if d < LAUNCH_GRACE => LAUNCH_REPLAY_MS,
        _ => &[],
    }
}

/// A click on a chat notification: show + focus the main window, open the chat.
pub fn open_chat(app: &AppHandle, peer_id: &str) {
    log::info!("chat notification clicked: opening {peer_id}");
    crate::show_main_window(app);
    let payload = serde_json::json!({ "peerId": peer_id });
    let _ = app.emit(OPEN_EVENT, payload.clone());
    let delays = replay_delays(started().get().map(Instant::elapsed));
    if !delays.is_empty() {
        let app = app.clone();
        let _ = std::thread::Builder::new().name("chat-notify-replay".into()).spawn(move || {
            let mut waited = 0;
            for &ms in delays {
                std::thread::sleep(Duration::from_millis(ms - waited));
                waited = ms;
                let _ = app.emit(OPEN_EVENT, payload.clone());
            }
        });
    }
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

    /// Install the delegate at startup so clicks on banners from a previous run,
    /// or the click that launched us, reach `did_receive`.
    pub fn init(app: &AppHandle) {
        let _ = APP.set(app.clone());
        let _ = center();
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

    pub fn init(_app: &AppHandle) {}

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
    use notify_rust::{ActionResponse, NotificationHandle};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Banners whose click we still wait on, oldest first.
    static LIVE: Mutex<LiveBanners<NotificationHandle>> = Mutex::new(LiveBanners::new());
    /// Waiter threads alive right now (a backstop if a server never closes).
    static WAITERS: AtomicUsize = AtomicUsize::new(0);

    pub fn init(_app: &AppHandle) {}

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
        let id = handle.id();
        if !try_acquire(&WAITERS, MAX_WAITERS) {
            // Banner is up; it just won't open the chat on click.
            log::warn!("chat notification: {MAX_WAITERS} banners already awaiting a click; not tracking this one");
            return true;
        }
        let (app, peer) = (app.clone(), peer_id.to_string());
        let spawned = std::thread::Builder::new().name("chat-notify".into()).spawn(move || {
            // Returns when the banner is clicked, dismissed, expires or is closed
            // by us for being the oldest past MAX_LIVE_BANNERS.
            let _ = notify_rust::handle_action(id, |resp: &ActionResponse| {
                if matches!(resp, ActionResponse::Custom("default")) {
                    let (a, p) = (app.clone(), peer.clone());
                    let _ = app.run_on_main_thread(move || open_chat(&a, &p));
                }
            });
            LIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(|h| h.id() == id);
            WAITERS.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            WAITERS.fetch_sub(1, Ordering::SeqCst);
            return true;
        }
        let evicted = LIVE.lock().unwrap_or_else(|e| e.into_inner()).push(handle);
        // Closing ends the oldest banner's waiter (NotificationClosed).
        for old in evicted {
            old.close();
        }
        true
    }
}

/// Linux: banners kept clickable at once; older ones are closed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_LIVE_BANNERS: usize = 5;
/// Linux: hard cap on threads waiting for a banner click.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_WAITERS: usize = 8;

/// Take a slot if fewer than `cap` are in use.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn try_acquire(n: &std::sync::atomic::AtomicUsize, cap: usize) -> bool {
    use std::sync::atomic::Ordering;
    n.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| (v < cap).then_some(v + 1)).is_ok()
}

/// Oldest-first list of live banners, bounded at `MAX_LIVE_BANNERS`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct LiveBanners<T> {
    items: std::collections::VecDeque<T>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl<T> LiveBanners<T> {
    const fn new() -> Self {
        Self { items: std::collections::VecDeque::new() }
    }
    /// Add the newest; returns the oldest ones that must now be closed.
    fn push(&mut self, item: T) -> Vec<T> {
        self.items.push_back(item);
        let over = self.items.len().saturating_sub(MAX_LIVE_BANNERS);
        self.items.drain(..over).collect()
    }
    fn remove(&mut self, pred: impl Fn(&T) -> bool) {
        self.items.retain(|x| !pred(x));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::*;
    pub fn init(_app: &AppHandle) {}
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

    #[test]
    fn cold_launch_clicks_are_replayed_warm_ones_are_not() {
        assert_eq!(replay_delays(Some(Duration::from_secs(1))), LAUNCH_REPLAY_MS);
        assert!(replay_delays(Some(LAUNCH_GRACE)).is_empty());
        assert!(replay_delays(Some(Duration::from_secs(3600))).is_empty());
        // `init` never ran (shouldn't happen) → no replay rather than a guess.
        assert!(replay_delays(None).is_empty());
        assert!(LAUNCH_REPLAY_MS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn live_banners_stay_bounded_and_evict_oldest() {
        let mut live = LiveBanners::new();
        let mut evicted = Vec::new();
        for id in 0..(MAX_LIVE_BANNERS as u32 + 3) {
            evicted.extend(live.push(id));
        }
        assert_eq!(evicted, vec![0, 1, 2]);
        assert_eq!(live.items.len(), MAX_LIVE_BANNERS);
        live.remove(|&id| id == 5);
        assert_eq!(live.items.iter().copied().collect::<Vec<_>>(), vec![3, 4, 6, 7]);
        assert!(live.push(8).is_empty());
    }

    #[test]
    fn waiter_slots_are_capped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let n = AtomicUsize::new(0);
        for _ in 0..MAX_WAITERS {
            assert!(try_acquire(&n, MAX_WAITERS));
        }
        assert!(!try_acquire(&n, MAX_WAITERS));
        n.fetch_sub(1, Ordering::SeqCst);
        assert!(try_acquire(&n, MAX_WAITERS));
        assert_eq!(n.load(Ordering::SeqCst), MAX_WAITERS);
    }
}
