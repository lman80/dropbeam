# DropBeam — the complete feature guide

DropBeam sends files, photos and messages **directly between your devices and your friends' devices** — across the room or across the world — with end-to-end encryption and no cloud account. This guide covers everything the app can do, how to use it on each platform, and the edges worth knowing.

**Platforms.** DropBeam ships for **macOS** (Apple Silicon + Intel, one `.dmg`), **Windows** (`.exe`/`.msi`), **Linux** (`.deb` and `.AppImage`) and **iOS** (iPhone/iPad, via TestFlight). Every feature below ends with an availability line:

> **Available:** macOS · Windows · Linux · iOS

"✗" next to a platform means the feature isn't there; notes explain partial support. A side-by-side matrix lives in [PARITY.md](PARITY.md).

---

## Contents

1. [Getting started, your profile and your devices](#1-getting-started-your-profile-and-your-devices)
2. [Sending files](#2-sending-files)
3. [Receiving files](#3-receiving-files)
4. [Transfers: progress, pause, retry, verify](#4-transfers-progress-pause-retry-verify)
5. [Friends](#5-friends)
6. [Chat](#6-chat)
7. [Shared Folders](#7-shared-folders)
8. [Locations (share a NAS or folder with friends)](#8-locations-share-a-nas-or-folder-with-friends)
9. [History and Recoverable files](#9-history-and-recoverable-files)
10. [Menu bar / tray, pop-ups and system integration](#10-menu-bar--tray-pop-ups-and-system-integration)
11. [Settings reference](#11-settings-reference)
12. [Diagnostics and feedback](#12-diagnostics-and-feedback)
13. [Updates](#13-updates)
14. [Privacy and security](#14-privacy-and-security)
15. [Troubleshooting](#15-troubleshooting)
16. [Every DropBeam code at a glance](#16-every-dropbeam-code-at-a-glance)

---

## 1. Getting started, your profile and your devices

### First launch: choose your name
The first time you open DropBeam it asks **"What should people call you?"** This is the name friends see when you send them something, chat, or share a folder. You can change it any time.

- **Desktop:** a welcome dialog; it also offers *"Already use DropBeam on another device? Link it"*.
- **iOS:** a welcome sheet with a name field and an *"Already use DropBeam? → Scan Code"* card to join your existing account.

> **Available:** macOS · Windows · Linux · iOS

### Your profile picture and name
Friends see your picture next to your messages and on their friend list. It's copied into DropBeam's own storage, so moving the original doesn't break it.

- **Desktop:** Friends → **You** card → click your picture to change it, click the pencil to rename; *Remove picture* is there too. Your name is also editable in Settings → Profile.
- **iOS:** Settings → tap your picture to pick a photo; tap your name row → **Profile** to rename or long-press the picture → *Remove Picture*.

Name changes are pushed to every friend automatically (immediately to friends who are online, and to the rest when they next connect).

> **Available:** macOS · Windows · Linux · iOS

### My Devices — one account across your phone and computers
Link your own devices together and they **share your friends list and every conversation**, kept in step directly between the devices (no server). Your own devices show up as *"Your Mac"*, *"Your iPhone"*, *"Your Windows PC"*… in your friend list and send menus, and a friend who uses several devices appears as one person.

**Link a new device (the device that already has your account shows a code):**
- **Desktop:** Settings → **Devices** → **Add a device** (or Friends → My devices → *Add a device*). A QR code appears; the new device scans it. You can instead choose *"Scan the other device instead"*.
- **iOS:** Settings → **My Devices** → **Add a Device**. Same QR; or *"Scan the Other Device Instead"*.

**Join an account from a new device:**
- **Desktop:** first-run dialog → *"…Link it"*, or Settings → Devices → **Join my other device's account** → scan the code (camera, screenshot, or paste). *"Show a code instead"* makes this device display a code for the other one to scan.
- **iOS:** welcome sheet → **Scan Code**, or Settings → My Devices → **Scan Code** / **Show a Code Instead**.

**Manage:** the Devices list shows each device, whether it's online, and when it last synced. **Sync now** forces a round; **Remove from account** (per device) or **Remove this device from account** (leave) stops syncing — friends and chats stay on the device, they just stop updating.

**Limits:** accounts sync *friends, chats and the device list*. They do not sync shared folders, Locations, settings, or transfer history. Devices must be online at the same time to sync (they catch up automatically when they next meet).

> **Available:** macOS · Windows · Linux · iOS

---

## 2. Sending files

There are several ways to start a send. They all end in the same place: a **"Send to"** chooser listing **My Devices**, **Friends** (with online status), and **Quick Send (code)**.

### Drag and drop (desktop)
Drag files or folders onto the DropBeam window on the **Send & Receive** page (the drop zone reads *"Drop to send"*). Pick who gets them.
- On the **Chat** page with a conversation open, a dropped file is *staged in the message box* instead (see [Chat](#6-chat)).
- On the **Locations** page with a friend's folder open, a drop *uploads into that folder*.

> **Available:** macOS · Windows · Linux · iOS ✗ (use the pickers)

### Pick files, photos or a folder
- **Desktop:** Send & Receive → **Photos** or **Files**. On macOS the Files picker selects folders too; on Windows and Linux use the separate **Choose a folder** button to send a whole folder.
- **iOS:** Send tab → **Photos** (your library; iCloud photos download first — a *"Preparing photo…"* overlay with Cancel appears) or **Files** (the Files app). iOS can't send a whole folder to a friend.

> **Available:** macOS · Windows · Linux · iOS (no folder sends)

### Send to a friend by name
Choose a friend (or one of your own devices) in the Send-to chooser. No code needed — it goes straight to them. If they have **auto-accept** on (the default) it just arrives; otherwise they get Accept/Decline.
- **Desktop:** also the **Send** button on any friend card (Friends page), and the menu-bar/tray menu (see [section 10](#10-menu-bar--tray-pop-ups-and-system-integration)).
- **iOS:** also Friends → a friend → **Send Files** → Photos or Files.

If the friend is offline, the transfer card says it's waiting and keeps trying.

> **Available:** macOS · Windows · Linux · iOS

### Quick Send — a one-time code, link or QR (no friendship needed)
For sending to anyone once. Choose **Quick Send (code)** in the Send-to chooser. The transfer card shows a **QR code** and a long `direct…` code with **Copy**. The other person opens DropBeam → **Have a code?** and scans the QR or pastes the code, and the files flow directly to them.
- Keep DropBeam open on your side until they've received it; the card says *"Waiting for the other device to connect…"*.
- A Quick Send code is meant for one receiver and one set of files.

> **Available:** macOS · Windows · Linux · iOS

### Send from outside the app
- **macOS — menu bar:** drag a file onto the DropBeam icon in the menu bar; the menu springs open and you drop it on a friend's name. Or click the icon → click a friend → pick files, or **Send a file** for a Quick Send.
- **macOS — Finder Services:** right-click a file → **Services → Share with DropBeam** (the app must be in /Applications; you may need to enable it once in System Settings → Keyboard → Keyboard Shortcuts → Services).
- **Windows — right-click:** right-click any file → **Send with DropBeam**. The app opens the Send-to chooser for it (starting DropBeam if it wasn't running).
- **Windows — tray:** click the tray icon to open the same quick menu as macOS (friend list, *Send a file*, receive-with-code). Drag-onto-the-icon is macOS only.
- **Linux:** the tray icon offers **Open DropBeam** and **Quit**. (The quick menu doesn't open from the tray on Linux — see [section 10](#10-menu-bar--tray-pop-ups-and-system-integration).)
- **iOS:** sending starts inside DropBeam (Send tab, a friend's page, or a chat). DropBeam doesn't appear in other apps' share sheets.

> **Available:** macOS (menu bar + Services) · Windows (right-click + tray) · Linux (tray menu only) · iOS ✗

### Sending in a chat
Attach files or photos to a conversation and they're delivered to that friend inside the chat — see [Chat → files and photos](#files-photos-and-gifs-in-chat).

> **Available:** macOS · Windows · Linux · iOS

---

## 3. Receiving files

### From friends: auto-accept or approve
Each friend has an **Accept files automatically** switch (on by default).
- **On:** their files land in your download folder without a prompt.
- **Off:** you get an offer — *"Alex wants to send you photo.jpg · 4 MB"* — with **Accept** / **Decline**.

Where you'll see the offer:
- **Desktop:** a small floating card in the bottom-right corner (with a **Save to…** menu), and the card on Send & Receive.
- **iOS:** the Send tab's transfer list (**Accept Files** / **Decline**).

Change it per friend: desktop Friends → the switch under a friend (*"Auto-accept files"* / *"Approve files first"*); iOS Friends → a friend → *Accept files automatically*.

> **Available:** macOS · Windows · Linux · iOS

### Save to… (choose a folder for one delivery)
On the desktop floating receive card, the caret beside **Accept** opens **Save to**: your default folder, a few quick folders, or **Choose…** for any folder — just for this delivery.

> **Available:** macOS · Windows · Linux · iOS ✗ (files always go to DropBeam's folder)

### Receiving with a Quick Send code
- **Desktop:** Send & Receive → **Have a code? Receive files** → paste the code, or **Scan a QR code** (camera, a dropped screenshot, or a pasted image). This field is smart: paste *any* DropBeam code and it does the right thing (a friend code adds the friend, a folder invite opens *Accept invite*, a device code links devices).
- **Menu bar / tray quick menu (macOS, Windows):** the ↓ button → paste a code. Scanning hands off to the main window.
- **iOS:** Send tab → **Have a code?** → paste, or tap the QR icon to scan. This field accepts Quick Send codes only; add friends from the Friends tab.

> **Available:** macOS · Windows · Linux · iOS

### Where received files land
| Platform | Default location | Change it |
|---|---|---|
| macOS | `~/Downloads` | Settings → Downloads → *Save received files to* |
| Windows | your Downloads folder | same |
| Linux | `~/Downloads` | same |
| iOS | **Files → On My iPhone → DropBeam** | fixed |

A file never overwrites one that's already there — a second `photo.jpg` is saved as `photo (1).jpg`. When a delivery completes on desktop, the card offers **Show in folder** (one file) or **Open folder**; on iOS it offers **Share** (save to Photos, AirDrop, open in another app…).

> **Available:** macOS · Windows · Linux · iOS

---

## 4. Transfers: progress, pause, retry, verify

Every send and receive gets a **transfer card** (Send & Receive on desktop; Send tab on iOS).

### Live progress
Percent, bytes done / total, **speed** and **time left**. On desktop, click the speed to switch between the live rate and the whole-transfer average, and click the time to base it on either. When done: *"Delivered · 1.2 GB · 42 s · 28 MB/s avg"*. Progress reflects what actually landed on the other side, not just what left your device.

> **Available:** macOS · Windows · Linux · iOS (live rate only)

### How it's connected: Local, Direct or Relay
Each card shows the path your files are taking:
- **Local network** — same Wi-Fi/LAN, fastest.
- **Direct** — a peer-to-peer link across the internet.
- **Relay** — an encrypted relay carries the data when a direct link can't be made (the relay can't read it, but it's slower).
- *"upgrading to direct…"* appears while a faster path is being negotiated.

Desktop cards show the path, round-trip time and upgrade status. iOS shows the path when you tap **Check** on a friend.

> **Available:** macOS · Windows · Linux · iOS (on the friend page, not on transfer cards)

### Pause and resume
**Desktop:** the ⏸ button on any send you're driving. Everything already delivered is kept; **Resume** carries on from there (even later, from the same card). Interrupted *receives* also keep their progress on disk so a retry resumes rather than restarting.

> **Available:** macOS · Windows · Linux · iOS (resume after a failure only — no pause button)

### Cancel, retry, dismiss
- **Cancel** (✕) stops an active transfer.
- **Retry** appears on a failed send or Location upload — one tap replays the same files; **Resume** on a paused one. On iOS a failed Quick Send *receive* can be retried too (it re-uses the code).
- **Dismiss** (✕ on a finished card, desktop) clears it from the list.

> **Available:** macOS · Windows · Linux · iOS (no dismiss for finished cards)

### Integrity: every file is checked
Every file is fingerprinted with **SHA-256** on both ends. The card and History show **Verified** when the receiver's copy matches, *Saved, unverified* if the other side is too old to confirm, and *Verification failed — retry* on a mismatch. On desktop, click the label to see each file's fingerprint on both sides.

> **Available:** macOS · Windows · Linux · iOS (checks run; the labels aren't shown)

### Verify copy
On a finished **send**, desktop offers **Verify copy**: DropBeam re-reads every file on the other device and compares it to yours — *"All 1,204 files identical"*, or a list of files that differ or are missing. Big folders show progress and can be cancelled.

> **Available:** macOS · Windows · Linux · iOS ✗

### Wait for a direct connection
With this setting on (Settings → Connection), a send that could only reach the relay **waits** instead, while DropBeam keeps trying for a fast direct path. The card says *"Waiting for a direct connection"* with a **Send over relay anyway** button. After a bounded wait it falls back to the relay so files still arrive.

> **Available:** macOS · Windows · Linux · iOS (setting only — no "Send over relay anyway" button)

### Taskbar / Dock progress
- **Windows, Linux:** the taskbar/launcher button fills while a transfer runs.
- **macOS:** the Dock icon shows progress when the window is minimized.

> **Available:** macOS · Windows · Linux · iOS ✗

---

## 5. Friends

A friend is permanent: add someone once and you can send and chat by name forever — it survives updates and reinstalls of the app on either side.

### Your DropBeam code
Your personal code (`dropbeam:…`) plus its QR. Share it once; anyone who adds it can reach you.
- **Desktop:** Friends → **You** → *Your DropBeam code* (QR, copy).
- **iOS:** Settings → your name → **Profile** → QR, **Copy Code**, **Share Code**.

> **Available:** macOS · Windows · Linux · iOS

### Add a friend
- **Desktop:** Friends → **Add friend** → paste their code, or scan their QR (camera, a dropped screenshot, or a pasted screenshot image). The code can be buried in a sentence or link — DropBeam finds it.
- **iOS:** Friends → **+** → **Add Friend** → scan their QR, or *Paste code instead*.

Once one side adds the other, both see each other.

> **Available:** macOS · Windows · Linux · iOS

### Rename, remove
- **Rename** changes the name *you* see for them (pencil on desktop; *Rename* on iOS).
- **Remove friend** removes them from your list; your chat history with them stays on the device, and re-adding the same device restores the conversation.

> **Available:** macOS · Windows · Linux · iOS

### Re-send an invite (desktop)
Each friend card has **Invite**, which shows a one-time invite QR/code for that person — handy if they reinstalled and need to add you back.

> **Available:** macOS · Windows · Linux · iOS ✗ (share your permanent code instead)

### Online status and "Check"
A green dot / *"Online now"* means DropBeam can reach them right now. **Check** pings them immediately (opening Friends or Locations also re-checks anyone who looks offline).
- **Desktop:** the friend card also shows the live **connection inspector** — Local / Direct / Relay, round-trip time, and *upgrading to direct…* — with a refresh button.
- **iOS:** a friend's page → **Check** → e.g. *"Direct · 42 ms"* or *"Offline · Try again later"*.

> **Available:** macOS · Windows · Linux · iOS

### Friend page actions (iOS)
Friends → a friend: **Send Files**, **Message**, **Check**, *Accept files automatically*, **Rename**, **Browse Locations** (their shared folders), **Remove Friend** (or *Remove from Account* for your own devices). Friends is searchable.

> **Available:** iOS (desktop has the same actions on the friend card)

---

## 6. Chat

A private, end-to-end encrypted messenger with each friend, iMessage-style.

- **Desktop:** the **Chat** page — conversation list on the left, the thread on the right. The sidebar and (on macOS) the Dock icon show your unread count.
- **iOS:** the **Chat** tab (with an unread badge) — a Messages-style list; tap **New message** (✎) to start one.

Messages you send while your friend is offline are kept and **delivered automatically** when they come back ("Sending — delivers when they're online"). Edits, unsends and reactions made offline are also delivered later.

> **Available:** macOS · Windows · Linux · iOS

### Messages, links and emoji
Type and press Enter (Shift+Enter for a new line on desktop). Links are clickable. Desktop has an **emoji picker** (☺); on iOS use the keyboard's emoji. A message of only 1–3 emoji shows large on iOS.

> **Available:** macOS · Windows · Linux · iOS

### Reactions
- **Desktop:** hover a message → ☺ → pick 👍 ❤️ 😂 🔥 😮 😢 🙏. Click your reaction chip to remove it.
- **iOS:** long-press a message → tap a tapback (❤️ 👍 👎 😂 ‼️ ❓) or **+** for more emoji.

> **Available:** macOS · Windows · Linux · iOS

### Reply, edit, unsend, copy
- **Reply** quotes a message (the quote updates if the original is edited or unsent).
- **Edit** your own text messages — shows *"(edited)"* / *"Edited"*.
- **Unsend** (desktop) / **Undo Send** (iOS) removes your message for both of you — *"You unsent a message."*
- **Copy** a message's text.

Desktop: hover → ↩ Reply, or ⋯ → Copy / Edit / Unsend. iOS: long-press → Reply / Copy / Edit / Save or Share / Undo Send.

> **Available:** macOS · Windows · Linux · iOS

### Typing indicator and read receipts
You'll see *typing…* (desktop) or the three-dot bubble (iOS) while your friend types. Under your latest message: **Delivered**, then **Read** when they've seen it. Turn off *Send read receipts* in Settings to stop sending yours.

> **Available:** macOS · Windows · Linux · iOS

### Files, photos and GIFs in chat
- **Attach:** desktop 📎 (or drag files onto the conversation); iOS **+** → Photos / Files. Attachments are **staged** in the message box first so you can add a caption and send them together; remove any with ✕.
- **Paste a screenshot (desktop):** ⌘V / Ctrl+V an image into the message box to stage it (up to 25 MB).
- **Viewing:** images open in a viewer (desktop lightbox; iOS swipeable full-screen viewer); other files open in their app or Share sheet. Chat files show live progress and a **Retry** if delivery failed.
- **GIFs:** add a free Giphy key in Settings (*GIFs (Giphy key)*); then desktop ✨ / iOS **+ → GIFs** to search and send. No key → the GIF button is hidden.

> **Available:** macOS · Windows · Linux · iOS (no paste-image on iOS)

### Search a conversation
Desktop 🔍 in the conversation header (*"Search messages and files…"*, ↑/↓ between matches, Esc to close). iOS 🔍 in the conversation, with *"n of m"* and ↑/↓. iOS can also search the whole chat list by name or last message.

> **Available:** macOS · Windows · Linux · iOS

### Notifications
A system notification arrives for new messages when you're not looking at that conversation (and a soft sound if *Play sounds* is on). On iOS, tapping a notification opens the chat. Turn off with *Chat message notifications*.

> **Available:** macOS · Windows · Linux · iOS (only while DropBeam is running — see [Troubleshooting](#15-troubleshooting))

### Chat extras
- **iOS:** **pin** conversations (swipe right → Pin, shown as big avatars at the top), swipe → **Read** to mark read, tap the header avatar for the friend's page.
- **Desktop:** a **Shared folder** button in the conversation header opens the folder you share with that friend, and shared-folder activity (*"You added the folder Vases (3 items)"*, *"Alex moved X → Y"*) appears in the conversation.

> **Available:** pins — iOS only · shared-folder button/activity — macOS · Windows · Linux

---

## 7. Shared Folders

Pair a folder on your computer with a friend's (or your own other computer's). Anything you drop in is beamed across automatically — even over the internet — and optionally everything stays identical both ways, like a private shared drive with no cloud.

**Where:** desktop sidebar → **Shared Folders**.

> **Available:** macOS · Windows · Linux · iOS (join only — see the note below)

### Create a shared folder
Shared Folders → **New folder**:
1. **Folder to share** — pick a folder.
2. **Their name** (optional) — adding it links you as friends automatically.
3. **Who can do what:**
   - **Full access · total sync** — everyone adds, edits and deletes; both folders stay identical. Deleted/replaced files are kept in Recoverable files.
   - **Full access · no deletes** — both sides add and change files, but deletes stay local (a safer shared drop).
   - **View only (read-only for them)** — your files flow to everyone you invite; their changes never come back to you.
4. **Invite friends** (optional) — tick friends and they get an in-app prompt to accept. Or skip this and share the invite code/QR with anyone.

### Join a shared folder
- **Desktop:** accept the in-app prompt (*"Shared folder invite"*) and choose where to keep it — or Shared Folders → **Accept invite** → paste/scan the `dropbeam1:…` invite → pick a folder. Pasting an invite into *Have a code?* also opens this.
- **iOS:** accept the **Folder Invite** sheet, or Friends → **+** → **Join Shared Folder** → scan it. You then pick a folder from Files; DropBeam imports a copy into **Files → DropBeam → Imported Folders** and keeps that copy in sync while the app is open.
  - iOS has no Shared Folders screen yet: once joined, you can't see the folder's status, pause it, change its mode or leave it from the phone. See [PARITY.md](PARITY.md).

### The folder card (desktop)
Each folder shows its members and their online state, its mode (*Total sync*, *Two-way*, *View only*, *Auto-delete*), and a live status line — *"Up to date · synced 2 min ago"*, *"Sending report.pdf"* with progress (and a **Stop this transfer** button; it will retry), *"Waiting for someone to accept the invite"*, *"Sync paused"*. After a batch it shows a summary: *"Sent 12 files · 340 MB · 18 s · 19 MB/s avg"*. Buttons: **Open folder**, **Pause/Resume**, and ⚙ **Folder settings**.

### Folder settings (⚙)
- **Play sound on sync** — a soft cue on each file (off by default).
- **Total sync (source of truth)** — switch between total sync and the lighter modes.
- **Two-way sync** — receive their files too, not just send (when total sync is off).
- **Delete after delivery** — a self-emptying outbox: your copy is removed once they confirm receipt, to the **Trash** (recoverable) or **Permanent**.
- **History** — jump to this folder's Recoverable files.
- **Verify** — re-checks that both folders are identical and fixes any difference (*"Folders match — 1,204 files"*). Needs the other device online.
- **Show invite** — re-show the invite (the folder's creator).
- **Unpair / Leave** — stop syncing. Your files stay where they are.

### People and roles
- **Add person** — creates a new invite for someone else to join the same folder.
- **Editor / Viewer** — the folder's owner can switch each member between ✎ **Editor** (can change the folder) and 👁 **Viewer** (read-only; their changes aren't sent). Viewers see *"View only: changes you make here are not sent"*.
- **Remove a member**, or cancel an invite nobody has used yet.

### Pause sync
**Pause** freezes the folder in both directions (the switch is shared — pausing on one device pauses it for everyone). **Resume** merges everything that changed on both sides meanwhile.

### Moves and renames
Moving or renaming a file *inside* a synced folder moves it on the other side too, instead of re-uploading it. Detected on macOS, Linux and iOS; on Windows a move is sent as a new copy plus a delete (the result is the same, just slower).

### Safety nets
- **Deletes are careful:** a file that appeared in the last couple of minutes is never removed by a sync, which protects files you're still dropping in.
- **Recoverable files:** in total-sync folders, anything deleted or overwritten is kept in a hidden history area inside the folder (never synced) — restore it from [History → Recoverable files](#recoverable-files).
- **Not synced:** hidden files (names starting with `.`) and DropBeam's own housekeeping files.
- **Sync popup:** a small floating *"Syncing folder…"* card appears during big folder transfers (turn off in Settings → Behavior).

> **Available:** macOS · Windows · Linux · iOS (join only; runs only while the app is open)

---

## 8. Locations (share a NAS or folder with friends)

A **Location** is a folder you open up to chosen friends *without* mirroring it: they browse it live, download what they want, and — if you allow — upload, rename or delete. Perfect for a home NAS, an external drive or a big media folder. Nothing is copied until someone asks for it.

### Share a Location (host)
**Desktop:** Settings → **Locations** → **Add a location** (or Locations page → **Share a folder**). A 3-step wizard:
1. **Where is the folder?** — one-click choices for mounted **NAS / network drives** and **external disks** (with free space), or **Choose another folder…**.
2. **Name it** — the name friends see (the folder itself isn't renamed).
3. **Who can use it?** — tick friends (each of your own devices counts separately), then choose:
   - **Can look and download**
   - **Can add files** — nothing already there is ever overwritten
   - **Can also rename & delete** — deleted items go to the folder's own `.dropbeam-trash`, so they're recoverable

   *Advanced:* **Largest single transfer** (default 500 GB).

On save DropBeam writes a tiny marker file so it can tell that exact drive from an empty mount point later. **Edit** or **Stop sharing** any time (stopping only removes access; nothing is deleted). A **Recent activity** list shows what friends did this session.

The Locations page's **Shared from this device** section shows each hosted folder as a *Gateway* card: reachable or not, free space, who has access, the last thing a friend did, and a live *"Receiving from Alex · 12 files"* line.

- The host device must be **awake with DropBeam running** for friends to reach it.
- Hosting is available on **macOS and Linux**. Windows can browse and use friends' Locations but can't host one yet. iPhone is a client only.

> **Available (hosting):** macOS · Linux · Windows ✗ · iOS ✗

### Browse a friend's Location
- **Desktop:** **Locations** → a friend's folder card (shows free space and your rights) → a file browser with breadcrumbs, filter, sort (name / size / date), pages of 500.
- **iOS:** Friends → **Locations** (or a friend → **Browse Locations**) → folder → pull to refresh, search, **Select**, and **•••** for actions. Friends who share nothing are listed with the reason (offline, nothing shared with this iPhone, error + Retry).

**What you can do (depending on your rights):**
- **Download** selected items — they arrive like any transfer, into your download folder.
- **Upload files** / **Upload folder** — desktop: buttons or just drag files onto the open folder; iOS: •••→ **Upload Photos / Files / Folder**. Uploads include hidden files. A file that already exists with different content lands beside it as *"… (2)"*.
- **New folder**, **Rename**, **Move to trash**.

**Limits:** at most 2 simultaneous transfers per friend per host, a per-transfer size cap set by the host, and the host must be online.

> **Available (browsing):** macOS · Windows · Linux · iOS

### Synced to a location — keep a folder copied to a friend's Location
A one-way "my computer feeds the NAS" sync. Locations page → **Sync a folder here**:
1. **Choose the folder** on this device.
2. **Choose where it goes** — any friend Location that lets you upload, plus a sub-folder (defaults to the folder's name).
3. **Check it over** → **Start syncing**. *Advanced:* **Also remove the copy when I delete a file here** (the copy goes to the Location's trash).

It uploads new files the moment you add them and re-checks every half hour. Each synced folder has **Sync now**, **Pause/Resume**, **Open folder** and **Remove** (nothing is deleted on either side).

> **Available:** macOS · Windows · Linux · iOS ✗

### For power users: scripted uploads
A script can queue Location uploads without the GUI by writing an `upload-queue.json` into DropBeam's config folder (all desktop platforms), or on Windows/Linux by launching `DropBeam --location-upload '{"friendId":…,"locationId":…,"relPath":…,"paths":[…]}'`.

> **Available:** macOS (queue file) · Windows · Linux · iOS ✗

---

## 9. History and Recoverable files

### Recents
Every send and receive, newest first, grouped **Today / Yesterday / Last 7 days / by month**, with **search** by file name or person. Each row shows direction, the other person, size, time and the path it took (Local/Direct/Relay).
- **Desktop:** History → **Recents**. Integrity details per row; **Show in folder** / **Open folder** for received files; **Clear list** empties it (your files aren't touched).
- **iOS:** History → **Recents**. Photo/video thumbnails; tap to preview, long-press for **Share**, **Copy Name**, **Remove** (one entry); ••• → **Clear History**.

> **Available:** macOS · Windows · Linux · iOS (per-entry Remove on iOS only)

### Recoverable files
Deleted or overwritten files from **total-sync** shared folders, kept so you can get them back.
- A **storage gauge** shows how much the saved copies use against your limit.
- Per folder: **Restore** a file (it goes back into the folder and re-syncs to everyone), **Delete forever**, or **Empty** the folder's copies.
- **Free up space** / **Empty All** removes every saved copy (live files untouched).
- **Automatic cleanup:** copies older than your chosen age (7 / 30 / 90 days / Forever; default 30) or beyond the per-folder storage limit (500 MB / 2 GB / 5 GB / No limit; default 2 GB) are removed oldest-first. Set these in Settings → Recoverable files.

Desktop: History → **Recoverable files** (also reachable from a folder's **History** button). iOS: History → **Recoverable**, and Settings → Advanced → **Recoverable Files**.

> **Available:** macOS · Windows · Linux · iOS

---

## 10. Menu bar / tray, pop-ups and system integration

### Always ready in the background
DropBeam starts at login quietly (menu bar/tray only, no window) and closing the window tucks it away instead of quitting — so friends' files arrive even when you haven't opened it. On macOS it has no Dock icon while the window is closed. Both behaviours can be turned off in Settings → Behavior. Quit from the menu-bar/tray menu.

> **Available:** macOS · Windows · Linux · iOS ✗ (iOS apps can't run in the background — see [Troubleshooting](#15-troubleshooting))

### The quick menu (click the menu-bar / tray icon)
A small panel with: your friends (search, online dots, click to send, drop a file on a name to send it — macOS), recent transfers, **Send a file** (Quick Send), and **↓ Receive with a code** (paste a code; scanning, folder invites and device codes open the main window). The button at the top left opens the full app; the header also has Quit and Close.

> **Available:** macOS (plus drag-onto-icon) · Windows · Linux ✗ (the Linux tray shows only *Open DropBeam* / *Quit*) · iOS ✗

### Floating transfer card
A compact card in the bottom-right corner for one-off sends and receives — sender, file name, a progress ring, speed and time, then *Sent ✓* / *Done*. For manual-accept offers it has **Accept** (with **Save to…**) and **Decline**.

> **Available:** macOS · Windows · Linux · iOS ✗

### Folder-sync popup
A small floating card at the top of the screen while a shared folder is moving files. Turn off with *Show the folder-sync popup*.

> **Available:** macOS · Windows · Linux · iOS ✗

### Unread badge
The sidebar shows unread chats on all desktops; macOS also badges the Dock icon. iOS shows the count on the Chat tab (not on the home-screen icon).

> **Available:** macOS · Windows (in-app only) · Linux (in-app only) · iOS (tab badge)

### Banners you might see (desktop)
- **Local Network** — *"DropBeam can't reach a device on your network directly…"* with **Open Settings** (see [Troubleshooting](#15-troubleshooting)).
- **Install location (macOS)** — if DropBeam is running from Downloads or a disk image, it asks you to move it to Applications (otherwise macOS forgets folder permissions every launch).

---

## 11. Settings reference

Desktop: sidebar → **Settings**. iOS: **Settings** tab. Settings are per device and are not synced between your devices.

| Setting | What it does | Default | Where |
|---|---|---|---|
| **Devices** | Link/remove your own devices; sync now | — | all (see [section 1](#my-devices--one-account-across-your-phone-and-computers)) |
| **Locations** | Share folders/NAS with friends | none | macOS, Linux (Windows shows it but can't host) |
| **Display name** | The name friends see | device name | all |
| **Save received files to** | Default download folder | Downloads | desktop |
| **Clear transfer cache** | Deletes leftovers of interrupted transfers now (also auto-cleaned after a week) | — | all |
| **Theme / Appearance** | System, Light or Dark | System | all |
| **Stay ready in the background** | Start at login, quietly in the menu bar/tray | On | desktop |
| **Keep running when you close the window** | Close = hide to menu bar/tray | On | desktop |
| **Notify when a file arrives** | System notification for incoming files | On | all |
| **Chat message notifications** | System notification for new messages | On | all |
| **Send read receipts** | Let friends see when you've read their messages | On | all |
| **GIFs (Giphy key)** | A free key from developers.giphy.com enables GIF search | blank (hidden) | all |
| **Play sounds** | Soft cues on send/receive/offers/messages | On | all |
| **Show the folder-sync popup** | Floating card during folder syncs | On | desktop |
| **Test direct connection** | Confirms the peer-to-peer engine is running | — | all (iOS: How Transfers Connect → Test Connection) |
| **Local network access** | Shortcut to the OS permission (see Troubleshooting) | — | macOS; iOS shows instructions |
| **Only send over direct connections** | Refuse the relay: a send fails rather than using it (Quick Send + friend sends; shared folders always use the best path) | Off | all |
| **Wait for a direct connection** | Hold relay-only sends while trying for a direct path, with a "Send over relay anyway" escape | Off | all |
| **Use parallel streams for big files** | Splits files ≥16 MB across several streams for speed; turn off if transfers stall on a network | On | all |
| **Limit internet upload speed** | Mbps cap for internet transfers (presets 50/100/150/300/Unlimited); local transfers stay full speed | Unlimited | all |
| **Show speeds in megabits** | Mbps instead of MB/s | Off | all |
| **Recoverable files** | Keep copies for (7/30/90 days/Forever), storage per folder (500 MB/2 GB/5 GB/No limit), **Free up space now** | 30 days, 2 GB | all |
| **Custom relay** | Your own relay server URL, used when a direct link fails. Set the same URL on both devices; restart to apply (desktop has a **Restart** button). Setup guide: `RELAY-SETUP.md` | blank (public relays) | all |
| **Version / Updates** | Check for updates, install & restart | auto | desktop (iOS shows the version) |
| **Detailed logging** | Adds deep network logs for reproducing a hard connection problem; restart to apply | Off | all |
| **Export logs** | Bundles logs into one file (desktop: Downloads; iOS: opens the Share sheet) | — | all |
| **Share background diagnostics** | Sends a small redacted error/performance summary about once a day | On | all |
| **Diagnostics endpoint** + **Send test** | Override where diagnostics go (advanced) | built-in | desktop |
| **Lab Mode** | Lets one trusted developer device run tests and push builds to this device; only the entered Operator ID is accepted | Off | desktop |
| **Feedback** | Send feedback / show the feedback button | — | desktop sidebar *Feedback*; iOS Settings → Feedback |

Per-folder and per-friend options live on the folder card (⚙) and friend card respectively.

---

## 12. Diagnostics and feedback

- **Send feedback:** desktop sidebar → **Feedback**; iOS Settings → Feedback → **Send Feedback** (iOS can also show a floating feedback button). Describe the problem, optionally attach screenshots; it goes to the developer's issue tracker.
- **Export logs:** Settings → Diagnostics → **Export logs**, then send the file to the developer. Logs contain no passwords or file contents.
- **Background diagnostics** (on by default): a redacted summary of errors and transfer performance is uploaded about once a day. File names, folder paths, device IDs, IP addresses and email addresses are stripped before anything leaves the device, and file contents are never included. Turn it off in Settings → Diagnostics.
- **Detailed logging:** turn on only while reproducing a connection problem, then restart.

> **Available:** macOS · Windows · Linux · iOS

---

## 13. Updates

- **macOS, Windows, Linux (AppImage):** DropBeam checks at launch, every 6 hours and when the network returns. When an update is ready, Settings → Updates shows **Install & restart** (with progress). If GitHub can't be reached (some networks block it), Settings offers **Get the latest from GitHub** for a manual download.
- **Linux .deb:** install new versions from the release page (in-app install is designed around the AppImage).
- **iOS:** updates come through TestFlight / the App Store.
- **First launch of a download:** macOS may say it "could not verify" DropBeam → System Settings → Privacy & Security → **Open Anyway**. Windows SmartScreen → **More info → Run anyway**.

> **Available:** macOS · Windows · Linux (AppImage) · iOS (TestFlight)

---

## 14. Privacy and security

- **Direct and end-to-end encrypted.** Every connection is an encrypted QUIC link between the two devices, authenticated by each device's own key. When a direct link can't be made, an encrypted relay forwards the traffic — the relay can't read it.
- **No account server.** Your identity is a key on your device. Friends, chats and your device list sync *directly* between your own devices. There's nothing to sign up for and no cloud copy of your files or messages.
- **Integrity checks.** Every file is SHA-256 fingerprinted on both ends.
- **Only friends get in.** Friend sends, chats, shared folders and Locations accept only devices you've added. Location access is checked per friend and per right (browse / upload / manage), and a host's files can't be reached outside the shared folder.
- **What stays on your device:** settings, friends, chats, transfer history, shared-folder and Location configuration, and your device key — in DropBeam's app-data folder (macOS `~/Library/Application Support/com.dropbeam.app`, Linux `~/.config/com.dropbeam.app`, Windows `%APPDATA%\com.dropbeam.app`, iOS the app's private storage).
- **What does leave your device, and to whom:**
  - Files and messages → only to the friend/device you chose (possibly through an encrypted relay).
  - Background diagnostics (on by default, opt-out) → a redacted summary to the developer's collector.
  - Feedback you choose to send → the developer.
  - GIF searches → Giphy, using your key. A received GIF is downloaded from Giphy.
  - Update checks → GitHub (desktop).
- **Lab Mode** is off by default and, when on, obeys only the single device ID you enter.

---

## 15. Troubleshooting

**Transfers to a device in the same room are slow / say "Relay".**
The operating system is probably blocking local-network access.
- **macOS:** System Settings → Privacy & Security → **Local Network** → turn on DropBeam — on **both** devices. The desktop banner's **Open Settings** goes straight there.
- **iOS:** Settings → Privacy & Security → **Local Network** → DropBeam on. Check it on the other device too.
- **Windows:** when Windows Firewall asks, allow DropBeam on **private networks**; make sure your Wi-Fi is set to a *Private* network. (The banner's *Open Settings* button only works on macOS.)
- **Linux:** make sure your firewall allows DropBeam's UDP traffic on the LAN.
- VPNs and proxy apps that capture all traffic can force every connection through the relay; exclude DropBeam if you can.

**Direct vs relay across the internet.** Most connections become Direct after a few seconds (watch for *upgrading to direct…*). Some networks (strict corporate or mobile carrier NAT) only allow the relay. If the public relays are slow for you, set up your own (**Custom relay**) on both devices. *Only send over direct connections* and *Wait for a direct connection* control what happens when only the relay is available.

**The QR scanner can't use the camera.**
- **macOS:** System Settings → Privacy & Security → **Camera** → DropBeam.
- **iOS:** Settings → DropBeam → **Camera**. Without it, use *Paste code instead*.
- **Desktop, no camera:** use **Scan from image…**, drop a screenshot of the QR onto the scanner, or copy a screenshot (⇧⌘⌃4 / Win+Shift+S) and paste it.

**iPhone: messages or files don't arrive when the app is closed.**
iOS pauses apps in the background, so DropBeam on iPhone sends and receives only while it's open. Keep it open (screen on) until a transfer finishes. Messages sent to you while it was closed arrive the next time you open it and your friend's device is online.

**A friend shows offline but is online.** Tap **Check** on them, or open Friends/Locations (which re-checks). Make sure DropBeam is running on their side.

**macOS keeps asking for folder permission / the app hangs on first launch.** Move DropBeam into Applications (the banner will tell you). If a *Files and Folders* prompt is pending (Downloads, Desktop), answer it.

**Something's wrong.** Settings → Diagnostics → **Export logs** and send the file with **Feedback**.

---

## 16. Every DropBeam code at a glance

You can paste any of these anywhere DropBeam asks for a code — the desktop *Have a code?* field routes each to the right place.

| Code starts with | What it is | Where to use it |
|---|---|---|
| `direct…` | One-time **Quick Send** code | Send & Receive → Have a code? (iOS: Send → Have a code?) |
| `dropbeam:` | Someone's permanent **friend code** | Friends → Add friend |
| `dropbeamf1:` | One-time **friend invite** (from a friend card's *Invite*) | Friends → Add friend |
| `dropbeam1:` | **Shared-folder invite** | Shared Folders → Accept invite (iOS: Friends → + → Join Shared Folder) |
| `dropbeamjoin1:` | "**Join my account**" code shown by a device that has your account | Devices → Join / Scan Code |
| `dropbeamlink1:` | "**Link this device**" code shown by a new device | Devices → Add a device → Scan the other device |

Codes still work when they're inside a sentence or a link, and QR codes can be scanned from the camera or a screenshot.
