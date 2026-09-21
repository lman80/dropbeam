// Set before activation so StrictMode cannot race the legacy UIKit tab bar.
export let nativeShellActive = false
export function setNativeShellActive(active: boolean) { nativeShellActive = active }
