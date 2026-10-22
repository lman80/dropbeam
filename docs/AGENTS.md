# DropBeam for AI agents — command line, MCP and remote commands

DropBeam is AI-first the way the owner's thesis means it: it doesn't build AI in, it gives
an AI agent **the same control a person has**. Every verb in the DropBeam window is an *op*
an agent can call, with the same rules (each op calls the same code the window calls),
written down (each op describes itself) and recorded (every call lands in the activity log
with who, what, why and the result).

Feedback behind it: #82 (a CLI so agents can send files between computers, phones and each
other), #70 (DropBeam as the way agents talk to each other), #86 (new computer: install
DropBeam, scan the QR, the agent does the rest).

- [Quick start](#quick-start)
- [The `dropbeam` command](#the-dropbeam-command)
- [MCP setup (Claude Code, Codex, others)](#mcp-setup)
- [Agents talking to agents](#agents-talking-to-agents)
- [Running commands on your own devices](#running-commands-on-your-own-devices)
- [Safety model](#safety-model)
- [The control API (for developers)](#the-control-api)
- [Op catalog](#op-catalog)
- [What stays in the window on purpose](#what-stays-in-the-window-on-purpose)

## Quick start

1. Open DropBeam → **Settings → Agents** → **Install Command-Line Tool**.
   - macOS: links `/usr/local/bin/dropbeam` to the app (asks for your password once if that
     folder isn't writable; if you cancel, it uses `~/.local/bin/dropbeam`).
   - Windows: writes `%LOCALAPPDATA%\DropBeam\bin\dropbeam.cmd` and adds that folder to your
     user PATH. Open a new terminal afterwards.
   - Linux: the `.deb` / `.rpm` install `/usr/bin/dropbeam`. AppImage and other installs: the
     button links `~/.local/bin/dropbeam`.
   - Without installing: run the app's own program file with `cli` first, e.g.
     `/Applications/DropBeam.app/Contents/MacOS/DropBeam cli status`.
2. Connect your agent: `claude mcp add dropbeam -- dropbeam mcp` (see [MCP setup](#mcp-setup)).
3. Ask it: "send ~/report.pdf to my iPhone", "tell Alex's agent the build is done and wait
   for its answer", "set up SSH on my new Linux box".

If DropBeam isn't running, `dropbeam` starts it in the background (menu bar / tray, no
window) and waits for it. A Linux machine with no desktop session has nothing to start it
in: run it under a virtual display, e.g. `xvfb-run -a DropBeam --minimized &` (package
`xvfb`), or log in to the desktop once with **Open at login** on. (The windowless
`DropBeam --server` mode runs only the Transfer Server and has no control API.)

## The `dropbeam` command

```
dropbeam status                        this device, network, linked devices
dropbeam friends                       your friends and your own devices
dropbeam devices                       devices linked to your account
dropbeam send <who> <paths…>           send files/folders (waits until done; --no-wait, --device <one of theirs>)
dropbeam chat <who> <text…>            send a message (- reads it from stdin)
dropbeam read <who> [--since 10m]      show messages (--limit N)
dropbeam wait [--from <who>] [--timeout 60]   wait for a new message (exit 124 on timeout)
dropbeam ls <who>:<location>[/path]    list a friend's shared Location
dropbeam get <who>:<location>/<path>…  download from a Location
dropbeam put <paths…> <who>:<location>[/path]   upload into a Location
dropbeam quicksend <paths…>            make a one-time download code
dropbeam receive <code>                download a Quick Send
dropbeam transfers [--active]          transfers since DropBeam started
dropbeam exec <device> -- <command…>   run a command on one of YOUR devices
dropbeam events [--types chat,transfer]   stream live events as JSON lines
dropbeam ops [--json]                  every op an agent can call
dropbeam op <name> [--input '{…}'] [--confirm] [--dry-run]   call any op
dropbeam mcp                           MCP server on stdin/stdout
```

- `<who>`: a friend's name, one of your devices' names, or an id (a unique start of a device
  id works). A name that fits several people is refused with the choices.
- Sending to a person with several devices sends to each of them, the same as the window
  (online ones now, the others when they come online or through a Transfer Server).
  `--device` picks one.
- `--json` works everywhere. `--agent <name>` and `--reason <why>` (or `DROPBEAM_AGENT` /
  `DROPBEAM_REASON`) show in the activity log; the default agent name is "command line".
- Exit codes: 0 done, 1 failed, 2 usage, 124 timed out, 130 canceled; `exec` returns the
  remote command's own exit code.
- Windows: use `dropbeam` (the `dropbeam.cmd` the button installs). It waits for the
  command and passes its exit code back; the app attaches to the terminal for output.

## MCP setup

`dropbeam mcp` is a stdio MCP server (JSON-RPC 2.0: `initialize`, `tools/list`, `tools/call`,
`ping`). Its tools are generated from the same catalog as the command line — one tool per
op, `.` written as `_` (`chat.send` → `chat_send`). Each tool's description gives its risk
and whether it needs `confirm: true`; every tool also takes `reason`, which the person sees
in the activity log next to the agent's name (the name comes from the MCP client, e.g.
"claude-code"). The server's instructions explain friends vs own devices, messaging another
agent and waiting for replies.

- **Claude Code:** `claude mcp add dropbeam -- dropbeam mcp`
- **Codex:** `codex mcp add dropbeam -- dropbeam mcp`, or in `~/.codex/config.toml`:
  ```toml
  [mcp_servers.dropbeam]
  command = "dropbeam"
  args = ["mcp"]
  ```
- **Any MCP client (JSON config):** `{"mcpServers": {"dropbeam": {"command": "dropbeam", "args": ["mcp"]}}}`

Without the installed command, use the app's program file with `cli mcp` as its arguments;
Settings → Agents shows the exact line for this computer.

## Agents talking to agents

A chat message sent through the control API carries `fromAgent: {name}` and shows as
"via claude-code" under the bubble on both sides (older DropBeam builds ignore the field).
Two agents on two computers converse like this:

```
agent A:  dropbeam chat Bob "build 412 is green — can you deploy?" --agent ci-bot
agent B:  dropbeam wait --from Alice --timeout 600        # prints the message
agent B:  dropbeam chat Alice "deploying now"
agent A:  dropbeam wait --from Bob --timeout 600
```

Over MCP: `chat_send`, then `chat_wait {from, since, timeoutSecs}`. `chat_wait` returns
messages newer than `since` (default: now) — pass the time you last looked so nothing is
missed. Files go the same way: `files_send`, then `transfers_wait`.

## Running commands on your own devices

For #86 ("every time I get a new computer…"), on the NEW computer:

1. Install DropBeam and link it to your account (Settings → Devices → scan the code shown on
   your other device, or the other way round).
2. Settings → Agents → **Let my other devices run commands here** → **For 1 Hour** (or
   **Until I Turn It Off**).

Your agent on your usual computer can now run commands there:

```
dropbeam exec linux-box -- 'mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys' < ~/.ssh/id_ed25519.pub
dropbeam exec linux-box --timeout 900 -- sudo -n apt-get install -y openssh-server
```

Output streams back live (stdout and stderr kept apart), piped input goes to the command,
Ctrl-C stops it (its whole process group on macOS/Linux, its process tree on Windows), and
`--timeout` (default 300 s, at most 24 h) stops it too. `--shell sh|bash|cmd|powershell` and
`--cwd` are optional; the default is `/bin/sh -c` (Windows: `cmd /C`) in the home folder.
Over MCP: `exec_run` (returns stdout and stderr, each capped at 4 MB).

### Who may run what

The **target** decides, every time, from facts it holds itself:

| Caller | Switch on the target | Result |
|---|---|---|
| A device linked to this account (judged by the connection's authenticated id, never by an id the caller claims) | on (for 1 hour and not expired, or until turned off) | runs |
| A device linked to this account | off or expired | refused: "turn on … in Settings → Agents" |
| A friend's device, or anyone else | anything | refused: "only runs commands from devices linked to its own account" — the same answer whether the switch is on or off |
| any | — on iOS, or in the windowless `--server` mode | refused: not supported |
| an older DropBeam (no `exec`) | — | it ignores the request; the caller says "update DropBeam on that device" |

Every command, run or refused, shows a notification on the target ("DropBeam ran a
command") and is written to the target's activity log with the device, agent, command and
exit code. The switch lives in `agents.json`, can only be changed in the window (there is
deliberately no op for it), is off by default and isn't offered on iOS.

What turning it on means: any device in your account — and any agent you let use one — can
run any command on this computer **as you** (not as administrator; `sudo` asks as usual).
Turn it on while you set a computer up. A lost device: remove it from your account
(Settings → Devices) and it is refused at once.

## Safety model

- **Who can connect:** only programs running as the same OS user.
  - macOS / Linux: a Unix socket `<config>/agent/agent.sock` (or `/tmp/dropbeam-<uid>/agent.sock`
    when the config path is too long for a socket, owner-checked); the folder is `0700`, the
    socket `0600`, and every connection's peer uid is checked against ours.
  - Windows: a named pipe with a random name, kept in `<config>\agent\pipe` (readable only
    by you); created first-instance-only, refusing remote clients, with a security
    descriptor that grants access to your user SID alone.
  - No TCP port; nothing is reachable from the network. `<config>` is
    `~/Library/Application Support/com.dropbeam.app`, `%APPDATA%\com.dropbeam.app` or
    `~/.config/com.dropbeam.app`.
- **Off switch:** Settings → Agents → *Let AI agents on this computer use DropBeam* (on by
  default, since only your own user can reach the socket). Off = every op is refused
  (`forbidden`), and the refusal is logged.
- **Same rules as the window:** ops call the window's own command functions. Inputs are
  strict: unknown fields, wrong types and missing fields are refused before anything runs.
- **Destructive ops need `confirm: true`.** Without it nothing happens, and the
  `confirmation_required` error carries a preview. Many ops accept `dryRun: true`.
- **What an agent can't do:** turn agent access on or off, allow remote commands, clear the
  activity log, change Lab Mode (`settings.update` refuses `labModeEnabled` /
  `labOperatorId`), or anything that only exists on screen (pickers, windows).
- **Recorded:** every call (except pure self-description: `status`, `ops.describe`,
  `agents.log`, `exec.allowedHere`) is appended to `<config>/agent-actions.jsonl` — time,
  agent, reason, op, a short summary (message text and paths shortened, codes cut), result,
  duration — rotated at 1 MB (one older file kept) and listed live in Settings → Agents.

## The control API

One JSON object per line over the socket or pipe:

```
→ {"id": 1, "op": "chat.send", "input": {"to": "Alex", "text": "hi"}, "agent": {"name": "my-bot", "reason": "status update"}}
← {"id": 1, "ok": true, "data": {…}}
← {"id": 1, "ok": false, "error": {"code": "not_found", "message": "…", "preview": …}}
```

Several requests may be in flight on one connection. Streaming ops (`events.subscribe`, and
`exec.run` with `"stream": true`) first send `{"id": 1, "event": {…}}` lines; while `exec.run`
streams, the client may send `{"id": 1, "stdin": "<base64>"}`, `{"id": 1, "eof": true}` and
`{"id": 1, "cancel": true}`. Closing the connection stops whatever it was waiting for.
Error codes: `invalid_input`, `unknown_op`, `not_found`, `ambiguous`, `forbidden`,
`confirmation_required`, `failed`, `rejected`, `unreachable`, `timeout`, `turned_off`,
`unsupported`, `needs_update`.

Code: `src-tauri/src/agent_ops.rs` (catalog and dispatch; a test fails when a new Tauri
command has no decision), `agent_ctl.rs` (socket, log, events, Settings → Agents),
`agent_exec.rs` (remote commands), `cli.rs` (command line and MCP).

## Op catalog

`dropbeam ops` (or the `ops.describe` op) prints the live catalog with full JSON schemas.
Bold arguments are required. 141 ops.

| Op | Risk | Input | What it does |
|---|---|---|---|
| `status` | read |  | This device: name, id, whether the network is up, how many of your devices are linked, and whether agents and remote commands are allowed here. |
| `ops.describe` | read | name | The catalog of every op: name, description, input schema, output, risk and promises. |
| `friends.list` | read |  | Your friends and your own linked devices. A person with several devices has one record per device; `person` is the same for all of them. |
| `friends.myCode` | read |  | Your permanent friend code. Anyone who adds it becomes your friend (they show up as a friend request first). |
| `friends.add` | write | **code** | Add a friend by their DropBeam code (the text under their QR code). |
| `friends.createInvite` | write | **name** | Make a one-time invite for a new friend (older way to add someone). |
| `friends.acceptInvite` | write | **invite** | Accept a one-time invite someone made for you. |
| `friends.invite` | read | **who** | The invite text for a friend you already have (to re-share it). |
| `friends.requests` | read |  | People who added your code and are waiting for you to accept. |
| `friends.acceptRequest` | write | **endpointId** | Accept a friend request. |
| `friends.declineRequest` | write | block, **endpointId** | Decline a friend request, optionally blocking them. |
| `friends.rename` | write | **name**, **who** | Rename a friend (only on your devices). |
| `friends.setAutoAccept` | write | **autoAccept**, **who** | Whether files from this friend save automatically (true) or wait for you to accept (false). |
| `friends.remove` | destructive (confirm, dryRun) | **who** | Remove a friend. You stop seeing each other until one of you adds the other again. |
| `friends.block` | destructive (confirm, dryRun) | **who** | Block a person: they can't send you files, messages or requests. |
| `friends.unblock` | write | **id** | Unblock a person. |
| `friends.blocked` | read |  | People you blocked. |
| `friends.lookAlike` | read |  | Friends with confusingly similar names (to check before trusting a name). |
| `friends.ping` | read | **who** | Check whether a friend's device answers right now. |
| `friends.connection` | read | **who** | How this device reaches a friend right now (local network, direct or relay, speed estimates). |
| `chat.list` | read |  | Every conversation with its last message, newest first. |
| `chat.read` | read | limit, since, **who** | Messages in a conversation, oldest first. |
| `chat.send` | write (dryRun) | replyTo, **text**, **to** | Send a text message. It shows as sent via your agent's name. Delivered right away when they're online, otherwise as soon as they are. |
| `chat.wait` | read | from, since, timeoutSecs | Wait for new messages (long poll). Use this to talk with a person or another agent: send with chat.send, then wait here for the reply. |
| `chat.react` | write | add, **emoji**, **messageId**, **who** | Add or remove an emoji reaction on a message. |
| `chat.edit` | write | **messageId**, **text**, **who** | Edit a message you sent. |
| `chat.delete` | destructive (confirm) | **messageId**, **who** | Unsend a message you sent (it disappears for both of you). |
| `chat.typing` | write | **on**, **who** | Show or hide 'typing…' in their window. |
| `chat.markSeen` | write | **who** | Mark a conversation read: clears the badge on your devices and tells them you read it (if read receipts are on). |
| `files.send` | write (dryRun) | device, **paths**, **to** | Send files or folders to a friend or to one of your own devices. A person with several devices gets them on each (online ones now, the others when they come online), the same as the window. A card also appears in your chat with them. |
| `quicksend.create` | write (dryRun) | **paths** | Make a one-time code anyone with DropBeam can use to download these files (no friendship needed). Keep DropBeam open until they've downloaded. |
| `quicksend.receive` | write | **code** | Download files from a Quick Send code into your download folder. |
| `transfers.list` | read | active | Transfers since DropBeam started (sending and receiving), newest first. |
| `transfers.wait` | read | **id**, timeoutSecs | Wait until a transfer finishes (completed, failed, canceled, paused or held). |
| `transfers.cancel` | write | **id** | Stop a transfer. What already arrived is discarded. |
| `transfers.pause` | write | **id** | Pause a send, keeping what already arrived. Resume with transfers.retry. |
| `transfers.retry` | write | **id** | Resume a paused or failed send that an agent started (it skips what already arrived). |
| `transfers.respond` | write | **accept**, **id**, saveTo | Accept or decline files a friend is offering (when their files wait for your OK). |
| `transfers.verify` | write | **id** | Re-check a finished send by comparing checksums on both devices. The result appears on the transfer. |
| `transfers.cancelVerify` | write | **id** | Stop a running check started by transfers.verify. |
| `transfers.clearCache` | destructive (confirm) |  | Delete leftovers of unfinished transfers (they can't be resumed afterwards). |
| `history.list` | read |  | Finished transfers (the History screen). |
| `history.remove` | destructive (confirm) | **id** | Remove one entry from History (files stay). |
| `history.clear` | destructive (confirm) |  | Clear History (files stay). |
| `folders.list` | read |  | Shared folders on this device. |
| `folders.status` | read |  | Sync state of every shared folder. |
| `folders.create` | write | **folder**, mirror, peerName, twoWay | Share a local folder: returns an invite the other person accepts with folders.accept. |
| `folders.accept` | write | **folder**, folderName, **invite** | Accept a shared-folder invite into a local folder. |
| `folders.update` | write | autoDelete, deleteMode, **id**, mirror, peerName, twoWay | Change a shared folder's options. |
| `folders.remove` | destructive (confirm) | **id** | Stop sharing a folder (your files stay where they are). |
| `folders.invite` | read | **id** | The invite text for an existing shared folder. |
| `folders.addPerson` | write | **id** | Make a new invite so one more person can join a shared folder. |
| `folders.inviteFriend` | write | code, **id**, **who** | Invite a friend into a shared folder directly. |
| `folders.setMemberRole` | write | **id**, **viewer** | Make a person in a shared folder view-only (true) or able to change files (false). |
| `folders.verify` | read | **id** | Compare a shared folder with the other side's copy. |
| `folders.verifyAll` | write |  | Start a comparison of every shared folder. |
| `folders.stopTransfer` | write | **id** | Stop the shared folder's transfer that's running now. |
| `folders.pause` | write | **id**, **paused** | Pause or resume syncing a shared folder. |
| `folders.history` | read | **id** | Recoverable (deleted or replaced) files of a shared folder. |
| `folders.historySummary` | read |  | How much space recoverable copies use, per folder. |
| `folders.restoreItem` | write | **id**, **itemId** | Put a recoverable file back. |
| `folders.forgetItem` | destructive (confirm) | **id**, **itemId** | Delete one recoverable copy for good. |
| `folders.clearHistory` | destructive (confirm) | **id** | Delete every recoverable copy of one folder. |
| `folders.clearAllHistory` | destructive (confirm) |  | Delete every recoverable copy of every folder. |
| `locations.hosted` | read |  | Folders this device shares as Locations. |
| `locations.hostedStatus` | read | **id** | Whether one of this device's Locations is reachable, how much space is left, who used it last. |
| `locations.save` | write | **location** | Share a folder as a Location, or change one (same object the Locations settings save). |
| `locations.unshare` | destructive (confirm) | **id** | Stop sharing one of this device's Locations (the folder itself stays). |
| `locations.activity` | read |  | Recent changes people made in this device's Locations. |
| `locations.mountCandidates` | read |  | Disks and network shares this device can see (for a new Location). |
| `locations.list` | read | **who** | Locations a friend (or your own device) shares with you. |
| `locations.ls` | read | cursor, **location**, path, query, **who** | List a folder inside a friend's Location. |
| `locations.download` | write | **location**, **paths**, **who** | Download files or folders from a friend's Location into your download folder. |
| `locations.upload` | write (dryRun) | **location**, path, **paths**, **who** | Upload files into a friend's Location (if they gave you upload rights). Never overwrites: a same-named file lands beside it. |
| `locations.mkdir` | write | **location**, **path**, **who** | Make a folder inside a friend's Location (needs manage rights). |
| `locations.rename` | write | **location**, **path**, **to**, **who** | Rename or move an item inside a friend's Location (needs manage rights). |
| `locations.trash` | destructive (confirm) | **location**, **path**, **who** | Move an item in a friend's Location to that Location's trash (recoverable there). |
| `locations.stat` | read | **location**, **path**, **who** | Type, size, modified time and version tag of one item in a friend's Location. |
| `locations.readUrl` | read | **location**, **path**, **who** | A link on this computer (http://127.0.0.1, works only here, only while DropBeam runs) that streams a file from a friend's Location without downloading it first; seeking works (HTTP Range). |
| `locations.cacheFile` | read | **location**, **path**, **who** | Bring a whole file from a friend's Location into DropBeam's cache and return its path (kept a day). |
| `locations.connection` | read | **who** | How this device reaches the device sharing a Location: Local, Direct or Relay, round trip and live speed. |
| `locations.clearCache` | write |  | Empty the cache of streamed Location files. |
| `drives.status` | read |  | Whether Locations show as a drive on this computer, where it's mounted, and the drive settings. |
| `drives.mount` | write |  | Show Locations as a drive (Finder, File Explorer or Files). |
| `drives.unmount` | write |  | Stop showing Locations as a drive. |
| `drives.refresh` | write |  | Re-read which Locations the drive shows. |
| `drives.settings` | write | **settings** | Change Settings → Drives (enabled, autoMount, cacheGb); only what's given changes. |
| `speed.findSafe` | write |  | Find my safe speed: test sends while watching the router, then a recommended upload limit. |
| `speed.cancelFind` | write |  | Stop a running speed.findSafe. |
| `syncedFolders.list` | read |  | Local folders this device keeps copied into a friend's Location. |
| `syncedFolders.status` | read |  | What each synced folder is doing now. |
| `syncedFolders.add` | write | deleteRemote, **localPath**, **location**, path, **who** | Keep a local folder copied into a friend's Location. |
| `syncedFolders.update` | write | deleteRemote, enabled, **id** | Turn a synced folder on/off or change delete behavior. |
| `syncedFolders.remove` | destructive (confirm) | **id** | Stop syncing a folder (files on both sides stay). |
| `syncedFolders.syncNow` | write | **id** | Check a synced folder for changes now. |
| `devices.list` | read |  | This device and every device linked to your account, with which one is this. |
| `devices.activity` | read |  | What your other devices are sending or receiving right now. |
| `devices.syncNow` | write |  | Bring friends and chats in step with your other devices now. |
| `devices.linkBegin` | write |  | Show a 'link me' code on THIS device: another device that has your account enters it with devices.join. |
| `devices.linkCancel` | write |  | Cancel the code from devices.linkBegin. |
| `devices.hostBegin` | write |  | Show a 'join me' code on THIS device: a new device enters it with devices.join to join your account. |
| `devices.hostCancel` | write |  | Cancel the code from devices.hostBegin. |
| `devices.linkPreview` | read | **code** | Check a device code before linking: who it belongs to and the safety code both screens show. |
| `devices.join` | destructive (confirm) | **code**, confirmToken | Link this device and the one whose code you have into one account (friends and chats come along). Both must be online. |
| `devices.linkPending` | read |  | The 'do the safety codes match?' question waiting on this device during a link, if any (also sent as a `link` event). |
| `devices.confirmLink` | write | **accept**, **endpointId** | Answer 'do the safety codes match?' on this device during a link. |
| `devices.approve` | write | **endpointId** | Approve a device that shows 'Needs approval'. |
| `devices.remove` | destructive (confirm) | **endpointId** | Remove one of your devices from your account (on every device). |
| `devices.leaveAccount` | destructive (confirm) |  | Take THIS device out of your account (it keeps its own friends). |
| `exec.run` | destructive (confirm, dryRun, streams) | **command**, cwd, **device**, shell, stdin, timeoutSecs | Run a command on one of YOUR OWN linked devices (not friends'), if that device has 'Let my other devices run commands here' turned on. Output streams back; the device shows a notification and logs it. |
| `exec.allowedHere` | read |  | Whether this device lets your other devices run commands here, and until when. |
| `settings.get` | read |  | All settings. |
| `settings.update` | write (dryRun) | **settings** | Change settings: pass only the ones to change (same names as settings.get). Lab Mode can only be changed in the window. |
| `settings.defaultDownloadDir` | read |  | The download folder DropBeam uses when none is set. |
| `profile.setPicture` | write | **path** | Use an image file as your profile picture (friends see it). |
| `profile.clearPicture` | write |  | Remove your profile picture. |
| `server.check` | read |  | Whether this device can be a Transfer Server (always on, space). |
| `server.folderSpace` | read | **path** | Free and total space of a folder's disk. |
| `server.status` | read |  | This device's Transfer Server: on/off, space, people, held items. |
| `server.configure` | write | **patch** | Set up or change this device's Transfer Server (same fields as its settings screen). |
| `server.removePerson` | destructive (confirm) | **personId** | Stop holding files for a person and drop what's held for them. |
| `server.restorePerson` | write | **personId** | Let a removed person use the server again. |
| `server.wipe` | destructive (confirm) |  | Delete everything this Transfer Server holds. |
| `server.disable` | destructive (confirm) | deleteItems | Turn this device's Transfer Server off. |
| `server.setOwner` | write | **account** | Set which account owns this Transfer Server. |
| `background.status` | read |  | Whether DropBeam keeps running when no one is signed in (Windows: a switch; Linux: the command to run; macOS: not available). |
| `background.set` | write | **enabled** | Turn "Keep running when no one is signed in" on or off (Windows). Windows shows a permission (UAC) prompt on this computer's screen, so a person must approve it. |
| `servers.list` | read |  | Transfer Servers your friends offer you. |
| `servers.prefs` | write | **eid**, holdForMe, offer, shareFriends, useIt | Choose whether and how to use a friend's Transfer Server. |
| `servers.forget` | write | **eid** | Stop using a friend's Transfer Server. |
| `servers.fetchNow` | write |  | Collect anything Transfer Servers hold for you now. |
| `servers.holdRoute` | read | **who** | Which Transfer Server would hold things for a friend while they're offline. |
| `servers.pendingFiles` | read |  | Files a Transfer Server holds for you that wait for your OK. |
| `servers.decideFile` | write | **accept**, **linkId** | Accept or decline a held file. |
| `push.status` | read |  | Phone notification setup status. |
| `push.setPreviews` | write | **on** | Show message text in phone notifications (true) or not. |
| `diagnostics.selfTest` | read |  | Check that the network engine works on this device. |
| `diagnostics.lanBlocked` | read |  | Whether the system blocks local-network access for DropBeam. |
| `diagnostics.export` | write |  | Save a diagnostics file (logs without file names or contents) and return its path. |
| `diagnostics.send` | write |  | Send a diagnostics test report now. |
| `diagnostics.installHint` | read |  | On macOS: advice if DropBeam runs from a place updates can't replace. |
| `events.subscribe` | read (streams) | timeoutSecs, types | Stream events as they happen: transfer progress, new messages, friends coming online, typing, friend requests, linking. One JSON line per event until you disconnect (or timeoutSecs). |
| `agents.log` | read | limit | The agent activity log of this device (newest last). |

## What stays in the window on purpose

Tauri commands no op covers (`ops.describe` also lists them as `humanOnly`):

| Command(s) | Why |
|---|---|
| `pick_files`, `pick_directory`, `pick_photos`, `share_files` | pickers and share sheets on screen; agents pass paths |
| `reveal_path`, `open_path`, `open_url`, `open_mailto`, `open_local_network_settings`, `open_privacy_settings` | open something on screen |
| `open_main_window`, `hide_popover`, `set_card_active`, `set_popover_rows`, `traydrag_debug`, `frontend_log`, `take_launch_file`, `take_open_urls`, `set_active_chat`, `set_unread_badge`, `set_keep_awake` | the window's own plumbing |
| `network_changed`, `lan_self_info`, `lan_peer_found`, `push_unregister_all`, `network_shutdown`, `push_set_device` | iOS native shell plumbing |
| `save_pasted_image`, `paste_clipboard_image`, `clipboard_file_paths`, `download_gif`, `send_chat_gif` | clipboard paste and the GIF picker; agents send text and files |
| `restart_app`, `quit_app` | would end the agent's own connection |
| `force_relay` | developer test that slows one connection on purpose |
| `location_thumb`, `location_open`, `drive_open` | picture previews for the browser grid; open a file or the drive on screen (agents use `locations.readUrl` / `locations.cacheFile`) |
| `drive_fix_windows` | asks for administrator permission on screen |
| `agent_settings`, `agent_set_access`, `agent_set_exec`, `agent_log`, `agent_clear_log`, `agent_install_cli` | Settings → Agents: an agent can't widen its own access or erase its trace |
