# DropBeam Privacy Policy

_Last updated: October 4, 2026_

DropBeam is a peer-to-peer file transfer and chat app for Mac, Windows, Linux and iPhone. Your files and messages travel directly between your devices and the people you choose, end-to-end encrypted. DropBeam has no account server and no file storage of ours. This page lists **every** way data leaves your device, including the few services involved.

Short version:

- Files, folders and chat messages go device to device, end-to-end encrypted. We never receive them.
- **Diagnostics are ON by default.** They send a redacted error/performance summary to us. You can turn them off in **Settings → Privacy → Share diagnostics**.
- **Feedback you send becomes a public GitHub issue**, including any screenshot you attach.
- Relay and discovery servers can see your IP address and your device's public key, but not your data.

## 1. Files, folders and chat

- **Device to device.** Transfers, shared-folder sync and chat use encrypted QUIC connections ([iroh](https://iroh.computer)) between devices. Only the two ends hold the keys.
- **Your identity is a key.** Each install creates a cryptographic key pair on the device. Its public half (the "device id") is how friends reach you. There is no sign-up, email or phone number.
- **Stored on your devices only.** Your friends list, chat history, shared-folder settings, transfer history and settings stay on your devices. Linking your own devices (Settings → Devices) copies them directly between those devices over the same encrypted connection. Nothing goes through us.
- **Shared folders** keep a recovery copy of files that were changed or deleted ("Recoverable files" in History) on your device. Retention follows your settings, and you can clear it at any time.

## 2. Relays and discovery (what network services can see)

To connect two devices, DropBeam uses:

- **Local network discovery (mDNS).** DropBeam announces itself on your local network so your devices and nearby friends can connect directly. Other devices on the same network can see that a DropBeam device is present, along with its device id.
- **Relay servers.** When a direct connection is not possible, encrypted traffic passes through a relay. By default these are the public relays run by number0 (n0), the company that makes iroh. You can set your own relay in Settings ([RELAY-SETUP.md](RELAY-SETUP.md)). A relay sees your **IP address**, your **device id**, the device id of the peer, and how much encrypted data passes between them. It cannot read the data.
- **Address lookup (DNS / pkarr).** Your device publishes a small signed record so friends can find it. The record holds your device id and the URL of your home relay, but not your IP address. It goes to number0's discovery service (DNS + pkarr relay), and anyone who knows your device id can look it up. Looking up a friend sends their device id to the same service.
- **Recovery code.** Your recovery words never leave your device. After you restore an account with them, the new device publishes, for 30 days, a small signed record on the same pkarr service under your account's public key, listing the device ids friends should greet. Every device also looks up its friends' account records every few hours, which sends those account keys to the same service. Only someone who already knows your account key (your friends and your own devices) can look yours up. See [docs/RECOVERY-CODE.md](docs/RECOVERY-CODE.md).

## 3. Transfer Servers (optional)

A Transfer Server is an always-on DropBeam device that you or a friend set up (for example a home computer or NAS). It holds items for devices that are offline.

- Items are sealed to the recipient device's key before upload. The server stores ciphertext and can't read files or messages.
- The server does see metadata: which device ids sent and receive each item, item sizes, times, and whether an item is a file or a chat message.
- **Retention:** by default a held file is kept **14 days** and a held chat message **30 days**, then deleted (the server owner can choose 1–90 days). Delivery receipts (item id + final state) are kept 30 days. An upload that was never finished is abandoned after 7 days.
- The server owner runs it. We do not operate Transfer Servers.

## 4. iPhone push notifications

When a Transfer Server holds something for a sleeping iPhone, it asks our push relay (a Cloudflare Worker, `dropbeam-push`) to send an Apple Push Notification.

- The message preview is sealed to the phone's own key, and the phone's push token is sealed to the Worker's key. The Worker can't read the preview, and the server can't read the token.
- The Worker and **Apple** do see that a notification was sent to that phone, when it was sent, and a short tag that groups notifications from the same person. Who sent it is sealed to the phone's key along with the preview, except when a Transfer Server doesn't know your phone's notification key yet (older app versions), in which case the sender's device id is visible to them. The phone decrypts the real preview on the device, and you can turn previews off.
- Your own Transfer Servers don't send pushes about people you blocked. If one arrives through a friend's server, your phone shows it without the name, text or sound.
- The Worker keeps only rate-limit counters and logs only Apple's status codes.

## 5. Diagnostics (on by default)

So we can find bugs and slow transfers we'd never hear about otherwise, DropBeam sends a diagnostics digest about 30 seconds after launch, then every 12 hours, and a couple of minutes after a transfer fails.

- **What's in it:** the errors, warnings and performance lines from DropBeam's own log since the last upload, grouped and counted. It also includes average transfer speeds, how many connections were direct versus relayed, a random per-install id (not linked to your identity), the app version, the OS and the CPU type.
- **What is removed first:** file and folder names, every quoted string, file-system paths, device ids and other long ids/keys, IP and MAC addresses, host names and URLs, and email addresses. File contents are never read.
- **Where it goes:** a Cloudflare Worker we operate (`dropbeam-diag`), stored in Cloudflare KV. The digests are kept for review and periodically pruned.
- **Crash reports:** while diagnostics are on, an unhandled error in the app window is also reported on the next launch through the feedback service (§6), so it becomes a **public** GitHub issue. It contains the error message and stack trace, the app version and the OS.
- **Turn it off:** Settings → Privacy → **Share diagnostics**. This stops both the digests and the automatic crash reports. Settings → "Detailed logging" only changes what is written to the log on your own device.

## 6. Feedback (public)

"Send Feedback" sends your message, an optional screenshot of the DropBeam window that you review before sending, and device information (app version, OS version, device model) to our feedback service, a Cloudflare Worker (`superfeedback`). It turns your report into a **public issue** at <https://github.com/lman80/dropbeam/issues>, so anyone can read it. Screenshots are stored in the same public repository. Don't include anything you wouldn't post publicly. To have a report removed, open an issue or email us.

The feedback widget also sends a once-a-day "check-in" with the app name, app version, OS and the widget's settings, so we can see which versions are in use. It contains no identifiers. Taking a screenshot downloads the html-to-image library from esm.sh.

## 7. Reporting people or messages

**Report** (on a person or a message) opens a pre-filled email in your own mail app, addressed to us. It contains only what you choose: the reason, your notes and, if you tick the box, the text of the message. Files are never attached.

## 8. Other services DropBeam contacts

- **Updates (desktop):** the app checks GitHub (`github.com/lman80/dropbeam/releases`) for new versions and downloads updates from there. GitHub sees your IP address.
- **GIFs (optional):** if you add a Giphy API key in Settings, GIF search terms and your IP address go to Giphy (api.giphy.com / media.giphy.com) when you use the GIF picker. The GIF you pick is downloaded by your device and sent to your friend as a normal file. Friends don't contact Giphy.
- **Link previews (optional, Settings):** when you send a link, **your** device fetches that web page to build the preview, so the website sees your IP address. The preview travels with the message, so the receiver never contacts the site.
- **Your own relay / Transfer Server / diagnostics endpoint:** if you configure one in Settings, the corresponding traffic goes there instead.

## 9. Permissions

- **Local network** (macOS, iPhone): used to find and connect to devices on the same network.
- **Camera:** used only while you scan a QR code. No images are stored.
- **Photos / files (iPhone):** DropBeam receives only the items you pick in the system picker.
- **Notifications:** used for incoming files and messages.
- **Launch at login:** on by default on macOS and Windows so the app can receive in the background. It's off by default on Linux. Change it in Settings → Launch at login.

## 10. What we don't do

We don't sell or share your data. We don't use advertising or tracking identifiers. We don't store your files, messages, contacts or photos on servers of ours.

## 11. Your choices and deleting data

- **Stop diagnostics and crash reports:** Settings → Privacy → Share diagnostics (off).
- **Delete local data:** remove friends, clear History and Recoverable files, or uninstall the app. Uninstalling doesn't remove the app's data folder on every OS. To remove it, delete the folder:
  - macOS: `~/Library/Application Support/com.dropbeam.app` and `~/Library/Logs/com.dropbeam.app`
  - Windows: `%APPDATA%\com.dropbeam.app` and `%LOCALAPPDATA%\com.dropbeam.app`
  - Linux: `~/.config/com.dropbeam.app`, `~/.local/share/com.dropbeam.app`
- **Diagnostics we hold:** they are tied only to a random install id. To have them deleted, send us that id (the `diag-id` file in the app's data folder) and we'll remove them.
- **Feedback issues:** ask us to delete an issue (and its screenshot) by commenting on it or emailing us.
- **Transfer Server items** expire on their own (§3), or the server owner can wipe them.

## Contact

Email: imamiller64@gmail.com, or open an issue at <https://github.com/lman80/dropbeam/issues> (public).
