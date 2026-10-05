# Everyday flows — "grandma walkthrough" audit (2026-10-05)

Branch `ux-everyday` (from `ios` @ 74aaaf1). Goal: a non-technical 75-year-old can install
DropBeam, send and receive files, and share a folder without getting stuck or losing anything.
Accounts, linking devices, friends, friend requests and chat are covered by the separate
`ux-accounts` pass and are out of scope here.

How it was done: walked each journey in the desktop Vite mock (macOS/Windows/Linux chrome,
light/dark, wide/narrow, empty and populated) and read the code behind every step; iOS was
walked in the native SwiftUI code and the simulator. Edge cases are from code reading unless
marked *live*. Before/after screenshots: `/tmp/dropbeam-audit/ux-everyday/{before,after,ios}/`.

**Ranking.** **P1** she'd get stuck or lose something · **P2** confused but recovers · **P3** polish.
**Status.** ✅ fixed on this branch (commit) · ⏸ deferred (why) · ❓ owner decision.

---

## Desktop (macOS · Windows · Linux)

### 1. Install and first launch

| # | P | Problem | Status |
|---|---|---|---|
| 1.1 | P1 | Unsigned builds: macOS "Apple could not verify…" and Windows SmartScreen block the first launch. The README explains the workaround, but the download page is the only place she'd see it. | ⏸ Real fix is signing + notarizing (AUDIT-2026-10-04 P1/P2). README steps reworded step-by-step ✅. |
| 1.2 | P2 | The welcome dialog asks for a name but never says what DropBeam is for. "Link an Existing Device…" sits next to Continue and competes with it. | ✅ One-sentence purpose under the title; the link button is quieter and says when to use it. |
| 1.3 | P2 | Nothing tells her what to do after naming herself. The empty Send page says "Drop files here to send" but she has no friends yet and doesn't know a code works for anyone. | ✅ Empty Send page: three plain steps (pick files → pick who → they get it) and a visible "Send to someone without DropBeam?" note. |
| 1.4 | P1 | macOS **Local Network** prompt appears with no context; if she clicks *Don't Allow*, transfers to the next room crawl over the internet. | Detection already exists (banner when a LAN peer is only reachable by relay). ✅ Banner copy now says what to click, step by step. |
| 1.5 | P1 | macOS **Files & Folders** (Desktop/Documents/Downloads) refusal: the error says "DropBeam doesn't have permission…" and stops there — dead end. | ✅ Permission errors now carry an **Open Settings** button (new `open_privacy_settings` command) and the exact switch to turn on. |
| 1.6 | P1 | **Notifications** turned off (or never allowed): with the window closed she never learns a file arrived. Nothing in the app notices. | ✅ Settings → Notifications checks the OS permission and shows a warning row with **Open Settings** (macOS/Windows). |
| 1.7 | P2 | Two copies of the app | Already handled (single-instance plugin focuses the running copy). |
| 1.8 | P3 | Linux without a tray (GNOME): closing minimizes instead of hiding — documented in README. | No change. |

### 2. Sending

| # | P | Problem | Status |
|---|---|---|---|
| 2.1 | P1 | **Quick Send to someone without DropBeam**: the code/QR only works inside DropBeam. Scanned with a phone camera it's just text; there's no hint that the other person needs the app or where to get it. | ✅ The waiting card says "They need DropBeam (free)" with a **Copy Download Link** button. ❓ A real web receiver (open the link in any browser and download) — see Owner decisions. |
| 2.2 | P2 | Send-to sheet: "Offline people: DropBeam keeps trying for about 2 minutes." — then what? | ✅ Says what happens after ("…then it stops; try again when they're online") and that her computer has to stay on. |
| 2.3 | P2 | **Sleep mid-transfer**: a long send/receive stops when the computer idles to sleep. Big single files resume, folders restart their current file. | ✅ macOS/Linux keep the computer awake while a transfer is moving (`caffeinate -i` / `systemd-inhibit`), released when idle. ⏸ Windows needs `SetThreadExecutionState` (a `windows` crate feature) — left for the Windows/CI owner. Lid-close sleep can't be prevented (by design). |
| 2.4 | P2 | Speed and time-left toggles (click to swap live/average) are invisible features; labels fine. | No change (tooltips exist). |
| 2.5 | P2 | Cancel vs Pause: icon-only buttons. | Tooltips exist; P3. |
| 2.6 | P2 | Failed send shows "Couldn't send — <plain reason>" + Retry. | Good already. |
| 2.7 | P3 | Big files / many files: engine handles (T1/T2 fixed on `ios`). Disk full on the receiver is only discovered when the disk fills. | ⏸ A pre-flight free-space check needs macOS "available for important usage" (statvfs ignores purgeable space and would refuse sends that would fit). Owner decision. |

### 3. Receiving — "where did it go?"

| # | P | Problem | Status |
|---|---|---|---|
| 3.1 | P1 | After a file arrives, the only way to find it is a small folder **icon** on the card. The floating card disappears the moment the transfer ends, the notification said only "Saved — click to open DropBeam", and the menu-bar list has no open button. | ✅ Received cards show a labelled **Show in Finder / File Explorer / Folder** button and "Saved in Downloads". The floating card now stays with **Received ✓ · Saved in Downloads** and **Open** / **Show in Finder** buttons. The notification says "Saved in Downloads". Menu-bar rows get a show button. |
| 3.2 | P2 | Auto-accept: files from friends land without asking (default on), and nothing explains that. | ✅ Downloads settings row explains it ("Friends' files save here automatically; turn off per friend under Friends"). |
| 3.3 | P2 | Changing the download folder: "Save files to [Downloads]" button is clear. | No change. |
| 3.4 | P3 | Duplicate names land as "name (1)". | No change (standard). |

### 4. Shared Folders

| # | P | Problem | Status |
|---|---|---|---|
| 4.1 | **P1** | **Joining a folder into Documents/Desktop mixed the shared files into it** — and in a two-way or total-sync folder, uploaded everything already in Documents to the other person. The invite prompt says "Choose where to keep it", which invites exactly that choice. | ✅ Engine (`557e24e`): a non-empty chosen folder gets a new subfolder named after the shared folder ("Documents/Family Photos"); tested. Dialog copy says so. |
| 4.2 | P1 | The incoming invite prompt doesn't say what joining allows ("Alex wants to share this folder with you"). In a total-sync folder, deleting a file deletes it for everyone. | ✅ The prompt decodes the invite and says it in plain words ("Everyone can add, change and delete files…"). |
| 4.3 | P1 | Folder settings: flipping **Total sync** on (deletes start syncing both ways) or **Delete after delivery** on (your own copies get deleted) takes effect instantly with no warning. | ✅ Mode is a 3-option choice with plain descriptions; switching to "Fully synced" or turning on delete-after-delivery asks first and says what will happen. |
| 4.4 | P2 | Jargon: "Total sync", "Two-way", "Accept Invite…", "Folder History", "Verify". | ✅ "Fully synced" / "Add and edit only" / "Only I can change it"; **Join a Folder…**; **Recoverable Files**; **Check Everything Matches**. GLOSSARY updated. |
| 4.5 | P2 | Empty state ("No shared folders · Keep a folder in sync with friends.") doesn't say how it differs from sending. | ✅ Illustrated empty state: "A shared folder is the same folder on everyone's computer. Put a file in, and it shows up for everyone. To send something once, use Send & Receive." |
| 4.6 | P2 | Roles: Editor/Viewer segmented control has tooltips only. | ✅ One-line explanation under People ("Editors can add, change and delete files. Viewers can only open and copy them."). |
| 4.7 | P2 | Deleted a file by mistake → where is it? Recoverable Files lives under History with no explanation, and the folder menu only links to it for total-sync folders. | ✅ History → Recoverable Files has an intro line; the folder menu links to it for every folder that has copies; folder settings mention it. |
| 4.8 | P2 | Leaving/stop sharing: confirm already says "Files already here stay on this computer." | Good already. Removing a person also says what happens to *their* copy now. ✅ |
| 4.9 | P2 | Status language is good ("Up to date", "Waiting for Sam", "Paused", red for problems). "Syncing 9 of 12" fine. | No change. |
| 4.10 | P3 | Sharing a very broad folder (home folder, whole Documents) when creating. | ✅ Create dialog warns when the chosen folder is your home folder or a top-level one (Desktop, Documents, Downloads, Pictures) and suggests a subfolder. |
| 4.11 | P3 | Conflicts (both edit the same file): engine keeps newest; older copy goes to Recoverable Files. | Explained in the "Fully synced" description. |

### 5. Locations and Transfer Server (advanced)

| # | P | Problem | Status |
|---|---|---|---|
| 5.1 | P2 | **Locations** sits in the middle of the everyday sidebar items. Its empty state ("No locations yet · Folders friends share with you appear here.") doesn't say what it is or that it's optional. | ✅ Moved below History in the sidebar (shortcuts follow the order). Empty state explains it ("A folder on an always-on computer or network drive (NAS) that friends let you open… Optional — most people never need it.") ❓ Hide the sidebar item until something is shared — see Owner decisions. |
| 5.2 | P2 | Settings tabs mix everyday and advanced: Transfers holds "Only send over direct connections", "Wait for a direct connection", "Parallel streams"; Privacy holds "Custom collector"; the Server tab says "Network port … UDP". | ✅ Connection tuning and the custom diagnostics collector moved to **Advanced**; tab renamed **Transfer Server**; Transfers keeps only Save to, unfinished transfers, Test connection, Local network access, speed. |
| 5.3 | P2 | Transfer Server has a good animated explainer. | No change. |

### 6. Settings overall

| # | P | Problem | Status |
|---|---|---|---|
| 6.1 | P2 | "Transfer leftovers · Clear Now" is jargon. | ✅ "Unfinished transfers — partly received files kept so an interrupted transfer can pick up where it stopped." |
| 6.2 | P2 | **Updates** never surface: a new version only shows in Settings → General → Updates, and installing restarts even mid-transfer. | ✅ A banner appears when an update is ready (**Restart to Update**, dismissable); Settings gets a dot. Installing while something is transferring asks first. |
| 6.3 | P3 | Dangerous buttons (Free Up Space, Delete Everything, Turn Off) already confirm. | Checked. |

### 7. Edge cases

| Case | P | What happens | Status |
|---|---|---|---|
| No internet | P2 | Desktop showed nothing; friends just go "offline". | ✅ Banner: "You're offline. Devices on the same Wi-Fi can still send to each other." |
| Captive-portal Wi-Fi / VPN / firewall | P2 | Looks like "offline friends"; relay fallback usually works. Windows firewall advice is in the Local Network banner. | ⏸ Detecting a captive portal needs an engine probe — Owner decision. |
| Two copies of the app | — | Single-instance; second launch focuses the first. | OK |
| App updated mid-transfer | P2 | Restart dropped transfers. | ✅ Confirm before installing while transfers run (big files resume after). |
| External drive unplugged | — | Shared folder shows "Folder not found — reconnect the drive" (D6). | OK |
| Read-only folder / no permission | P1 | Plain error, now with **Open Settings** when it's a macOS privacy refusal. | ✅ |
| Emoji / very long names, 0-byte files | — | Engine sanitizes names; 0-byte files transfer. | OK (engine tests) |
| Folders with 100k files | P2 | Manifest cap fixed (T1); first scan can take minutes with "Syncing…". | OK |
| Laptop lid closed / idle sleep | P2 | See 2.3. | ✅ macOS/Linux idle sleep; ⏸ Windows |
| Windows long paths (>260 chars) | P2 | Deep folder trees from a Mac can fail to land on Windows. | ⏸ Engine (`\\?\` prefixes) — Windows/CI owner. |
| Linux without tray | P3 | Window minimizes instead of hiding (README). | OK |
| Disk full | P2 | Plain "There isn't enough free space on the disk." after the disk fills. | ⏸ see 2.7 |

---

## iOS

_(iOS section from the iOS pass — see below.)_

---

## Owner decisions

1. **Web receiver for Quick Send.** Today the other person must install DropBeam. A hosted page (e.g. `dropbeam.app/r#<code>`) that downloads in any browser would make "send to anyone" true. Needs a relay/web endpoint (fits the self-hosted relay / Transfer Server plans). Recommend: yes, after signing.
2. **Hide Locations until used.** The sidebar item is now lower down with an explanation. Fully hiding it until a friend shares one (or you host one) is cleaner for most people but makes the feature harder to discover. Recommend: hide behind Settings → Advanced → "Show Locations" for new installs.
3. **Free-space check before receiving.** Needs the macOS "available for important usage" figure (not statvfs) to avoid refusing sends that would fit. Recommend: yes, warn-only on the accept prompt first.
4. **Captive-portal / no-route detection** in the engine (probe a known URL when every friend goes offline at once) to say "This Wi-Fi needs you to sign in" instead of "offline".
5. **Windows keep-awake** during transfers (`SetThreadExecutionState`) and **long-path** support — Windows owner.

## Deferred

- Signing/notarization (1.1) — release owner.
- Windows keep-awake and long paths — Windows owner.
- Pre-flight disk space (2.7), captive portal (edge cases) — owner decisions above.
