Vendored from tauri-plugin-notification 2.3.3 (MIT OR Apache-2.0).
Rust, desktop, and Android implementation are unchanged. Local changes:

- iOS ActiveNotification includes the persisted `__EXTRA__` payload.
- Reading a delivered notification no longer force-unwraps the process-local
  notification cache, which is empty after restarting the app.
- iOS `watch_actions` installs an app-lifetime channel and drains taps queued
  before JS initialization. The existing actionPerformed event remains available.
- build.rs declares the watch_actions permission; only the app's iOS capability
  grants it. The frontend ignores dismissals and non-chat notifications.

Remove this patch when upstream provides equivalent metadata and startup delivery.
