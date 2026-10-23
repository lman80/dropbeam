//! Every verb a person has in the DropBeam window, as a named op an AI agent can
//! call (#82, #70, #86). One catalog feeds the local control API (agent_ctl.rs),
//! the `dropbeam` command line and the MCP server (cli.rs), so they never drift.
//!
//! Rules (from the owner's thesis: an agent gets the same control as a human,
//! not AI built into the app):
//! - An op calls the SAME function the window's Tauri command calls — never a
//!   second implementation — so it obeys the same checks.
//! - Each op describes itself: name, plain-words description, JSON input schema,
//!   what it returns, its risk (read / write / destructive) and what it promises.
//! - Destructive ops need `confirm: true`; some ops take `dryRun: true`.
//! - Every call is written to the agent activity log with who, what, why and the
//!   result (agent_ctl.rs) — an agent that acts without a trace is a bug.
//!
//! `COMMAND_DECISIONS` (tested) holds a decision for every `#[tauri::command]`
//! in lib.rs: covered by an op, or left out on purpose with the reason.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::models::Friend;
use crate::{commands, friends, AppState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk { Read, Write, Destructive }

#[derive(Clone, Copy)]
enum Ty { S, B, I, Strs, Obj }

/// One op in the catalog.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    /// Destructive: refused with `confirmation_required` unless `confirm: true`.
    pub confirm: bool,
    /// Accepts `dryRun: true` (checks everything and says what would happen).
    pub dry_run: bool,
    /// Sends several result lines (events / command output) before the last one.
    pub streaming: bool,
    pub input: Value,
    pub output: &'static str,
    pub guarantees: &'static str,
    /// The Tauri commands this op covers (the window's code path it calls).
    #[serde(skip)]
    pub commands: &'static [&'static str],
}

struct B(OpSpec, Map<String, Value>, Vec<&'static str>);

fn op(name: &'static str, risk: Risk, description: &'static str) -> B {
    B(OpSpec { name, description, risk, confirm: risk == Risk::Destructive, dry_run: false, streaming: false,
        input: Value::Null, output: "", guarantees: "", commands: &[] }, Map::new(), vec![])
}

impl B {
    fn arg(mut self, name: &'static str, ty: Ty, desc: &'static str, required: bool) -> Self {
        let schema = match ty {
            Ty::S => json!({"type": "string", "description": desc}),
            Ty::B => json!({"type": "boolean", "description": desc}),
            Ty::I => json!({"type": "integer", "minimum": 0, "description": desc}),
            Ty::Strs => json!({"type": "array", "items": {"type": "string"}, "description": desc}),
            Ty::Obj => json!({"type": "object", "description": desc}),
        };
        self.1.insert(name.into(), schema);
        if required { self.2.push(name); }
        self
    }
    fn req(self, name: &'static str, ty: Ty, desc: &'static str) -> Self { self.arg(name, ty, desc, true) }
    fn opt(self, name: &'static str, ty: Ty, desc: &'static str) -> Self { self.arg(name, ty, desc, false) }
    fn out(mut self, s: &'static str) -> Self { self.0.output = s; self }
    fn promise(mut self, s: &'static str) -> Self { self.0.guarantees = s; self }
    fn cmds(mut self, c: &'static [&'static str]) -> Self { self.0.commands = c; self }
    fn dry(mut self) -> Self { self.0.dry_run = true; self }
    fn streams(mut self) -> Self { self.0.streaming = true; self }
    fn build(self) -> OpSpec {
        let B(mut spec, mut props, required) = self;
        if spec.confirm {
            props.insert("confirm".into(), json!({"type": "boolean",
                "description": "Must be true to do it. Without it nothing happens and you get a preview (error code confirmation_required). Ask the person first."}));
        }
        if spec.dry_run {
            props.insert("dryRun".into(), json!({"type": "boolean", "description": "Check everything and say what would happen, without doing it."}));
        }
        if spec.guarantees.is_empty() {
            spec.guarantees = match spec.risk {
                Risk::Read => "Changes nothing.",
                Risk::Write => "Same checks as the DropBeam window. Recorded in the agent activity log.",
                Risk::Destructive => "Needs confirm: true. Same checks as the DropBeam window. Recorded in the agent activity log.",
            };
        }
        spec.input = json!({"type": "object", "properties": props, "required": required, "additionalProperties": false});
        spec
    }
}

const WHO: &str = "Who: a friend's name, one of your own devices' names, or an id (friend id or device id; a unique start of a device id works).";
const PATHS: &str = "Full paths of files or folders on this computer.";

pub static CATALOG: LazyLock<Vec<OpSpec>> = LazyLock::new(catalog);

pub fn find(name: &str) -> Option<&'static OpSpec> {
    CATALOG.iter().find(|o| o.name == name)
}

#[rustfmt::skip]
fn catalog() -> Vec<OpSpec> {
    use Risk::*;
    use Ty::*;
    vec![
        // ── about DropBeam itself ──
        op("status", Read, "This device: name, id, whether the network is up, how many of your devices are linked, and whether agents and remote commands are allowed here.")
            .out("{version, deviceName, displayName, endpointId, ready, linkedDevices, agentAccess, commandsAllowedHere}")
            .cmds(&["my_endpoint_id", "iroh_node_id"]).build(),
        op("ops.describe", Read, "The catalog of every op: name, description, input schema, output, risk and promises.")
            .opt("name", S, "Only this op.").out("{ops: [OpSpec], humanOnly: [{command, why}] — what stays in the window on purpose}").build(),

        // ── friends ──
        op("friends.list", Read, "Your friends and your own linked devices. A person with several devices has one record per device; `person` is the same for all of them.")
            .out("[{id, name, person, endpointId, ownDevice, deviceKind, deviceOs, deviceModel, autoAccept}]").cmds(&["list_friends"]).build(),
        op("friends.myCode", Read, "Your permanent friend code. Anyone who adds it becomes your friend (they show up as a friend request first).")
            .out("code (text)").cmds(&["my_invite_code"]).build(),
        op("friends.add", Write, "Add a friend by their DropBeam code (the text under their QR code).")
            .req("code", S, "Their code, e.g. dropbeam:…").out("the new friend").cmds(&["add_friend_by_code"]).build(),
        op("friends.createInvite", Write, "Make a one-time invite for a new friend (older way to add someone).")
            .req("name", S, "What you call them.").out("{friend, invite}").cmds(&["create_friend"]).build(),
        op("friends.acceptInvite", Write, "Accept a one-time invite someone made for you.")
            .req("invite", S, "The invite text.").out("the new friend").cmds(&["accept_friend"]).build(),
        op("friends.invite", Read, "The invite text for a friend you already have (to re-share it).")
            .req("who", S, WHO).out("invite (text)").cmds(&["friend_invite"]).build(),
        op("friends.requests", Read, "People who added your code and are waiting for you to accept.")
            .out("[{endpointId, name, …}]").cmds(&["list_friend_requests"]).build(),
        op("friends.acceptRequest", Write, "Accept a friend request.")
            .req("endpointId", S, "From friends.requests.").out("the new friend").cmds(&["accept_friend_request"]).build(),
        op("friends.declineRequest", Write, "Decline a friend request, optionally blocking them.")
            .req("endpointId", S, "From friends.requests.").opt("block", B, "Also block them.").cmds(&["decline_friend_request"]).build(),
        op("friends.rename", Write, "Rename a friend (only on your devices).")
            .req("who", S, WHO).req("name", S, "New name.").cmds(&["rename_friend"]).build(),
        op("friends.setAutoAccept", Write, "Whether files from this friend save automatically (true) or wait for you to accept (false).")
            .req("who", S, WHO).req("autoAccept", B, "true = save automatically.").cmds(&["set_friend_auto_accept"]).build(),
        op("friends.remove", Destructive, "Remove a friend. You stop seeing each other until one of you adds the other again.")
            .req("who", S, WHO).dry().cmds(&["remove_friend"]).build(),
        op("friends.block", Destructive, "Block a person: they can't send you files, messages or requests.")
            .req("who", S, WHO).dry().out("the device ids now blocked").cmds(&["block_friend"]).build(),
        op("friends.unblock", Write, "Unblock a person.")
            .req("id", S, "Id from friends.blocked.").cmds(&["unblock_person"]).build(),
        op("friends.blocked", Read, "People you blocked.").out("[{id, name, …}]").cmds(&["list_blocked"]).build(),
        op("friends.lookAlike", Read, "Friends with confusingly similar names (to check before trusting a name).")
            .out("[[{id, name}]]").cmds(&["friends_look_alike"]).build(),
        op("friends.ping", Read, "Check whether a friend's device answers right now.")
            .req("who", S, WHO).out("true if it answered").cmds(&["ping_friend"]).build(),
        op("friends.connection", Read, "How this device reaches a friend right now (local network, direct or relay, speed estimates).")
            .req("who", S, WHO).out("connection details or null").cmds(&["probe_connection"]).build(),

        // ── chat ──
        op("chat.list", Read, "Every conversation with its last message, newest first.")
            .out("[{peerId, last, count, …}]").cmds(&["list_chats"]).build(),
        op("chat.read", Read, "Messages in a conversation, oldest first.")
            .req("who", S, WHO).opt("since", I, "Only messages after this time (ms since 1970).").opt("limit", I, "At most this many (newest). Default 50.")
            .out("[{id, fromMe, text, kind, files, ts, status, fromAgent}]").cmds(&["get_chat_messages"]).build(),
        op("chat.send", Write, "Send a text message. It shows as sent via your agent's name. Delivered right away when they're online, otherwise as soon as they are.")
            .req("to", S, WHO).req("text", S, "The message (up to 4000 characters).")
            .opt("replyTo", S, "Id of the message this answers.").dry()
            .out("the stored message {id, ts, status}").promise("Stored before it returns; DropBeam keeps retrying delivery. Recorded in the agent activity log.")
            .cmds(&["send_chat_message"]).build(),
        op("chat.wait", Read, "Wait for new messages (long poll). Use this to talk with a person or another agent: send with chat.send, then wait here for the reply.")
            .opt("from", S, "Only from this person (WHO). Leave out for anyone.").opt("since", I, "Messages after this time (ms). Default: now.")
            .opt("timeoutSecs", I, "Give up after this many seconds (default 60, at most 3600).")
            .out("{messages: [...], timedOut}").build(),
        op("chat.react", Write, "Add or remove an emoji reaction on a message.")
            .req("who", S, WHO).req("messageId", S, "Message id.").req("emoji", S, "The emoji.").opt("add", B, "false removes it. Default true.")
            .cmds(&["react_to_message"]).build(),
        op("chat.edit", Write, "Edit a message you sent.")
            .req("who", S, WHO).req("messageId", S, "Message id.").req("text", S, "New text.").cmds(&["edit_chat_message"]).build(),
        op("chat.delete", Destructive, "Unsend a message you sent (it disappears for both of you).")
            .req("who", S, WHO).req("messageId", S, "Message id.").cmds(&["delete_chat_message"]).build(),
        op("chat.typing", Write, "Show or hide 'typing…' in their window.")
            .req("who", S, WHO).req("on", B, "true = typing.").cmds(&["send_typing"]).build(),
        op("chat.markSeen", Write, "Mark a conversation read: clears the badge on your devices and tells them you read it (if read receipts are on).")
            .req("who", S, WHO).cmds(&["chat_mark_seen", "send_read_receipt"]).build(),

        // ── files ──
        op("files.send", Write, "Send files or folders to a friend or to one of your own devices. A person with several devices gets them on each (online ones now, the others when they come online), the same as the window. A card also appears in your chat with them.")
            .req("to", S, WHO).req("paths", Strs, PATHS).opt("device", S, "Only this one of their devices (name or id).").dry()
            .out("the transfer {id, state, bytesTotal, …}; follow it with transfers.wait")
            .promise("Files are checked end to end; a stopped send can be resumed with transfers.retry. Recorded in the agent activity log.")
            .cmds(&["send_to_friend", "send_chat_file_note"]).build(),
        op("quicksend.create", Write, "Make a one-time code anyone with DropBeam can use to download these files (no friendship needed). Keep DropBeam open until they've downloaded.")
            .req("paths", Strs, PATHS).dry().out("the transfer, with `code`").cmds(&["iroh_send"]).build(),
        op("quicksend.receive", Write, "Download files from a Quick Send code into your download folder.")
            .req("code", S, "The code.").out("the transfer").cmds(&["iroh_receive"]).build(),

        // ── transfers ──
        op("transfers.list", Read, "Transfers since DropBeam started (sending and receiving), newest first.")
            .opt("active", B, "Only ones still running or waiting.").out("[transfer]").build(),
        op("transfers.wait", Read, "Wait until a transfer finishes (completed, failed, canceled, paused or held).")
            .req("id", S, "Transfer id.").opt("timeoutSecs", I, "Default 3600.").out("the transfer, or {timedOut:true}").build(),
        op("transfers.cancel", Write, "Stop a transfer. What already arrived is discarded.")
            .req("id", S, "Transfer id.").cmds(&["cancel_transfer"]).build(),
        op("transfers.pause", Write, "Pause a send, keeping what already arrived. Resume with transfers.retry.")
            .req("id", S, "Transfer id.").cmds(&["pause_transfer"]).build(),
        op("transfers.retry", Write, "Resume a paused or failed send that an agent started (it skips what already arrived).")
            .req("id", S, "Transfer id.").out("the new transfer").build(),
        op("transfers.respond", Write, "Accept or decline files a friend is offering (when their files wait for your OK).")
            .req("id", S, "Transfer id (state waitingForAccept).").req("accept", B, "true = accept.").opt("saveTo", S, "Folder to save into (default: download folder).")
            .cmds(&["respond_to_offer"]).build(),
        op("transfers.verify", Write, "Re-check a finished send by comparing checksums on both devices. The result appears on the transfer.")
            .req("id", S, "Transfer id.").cmds(&["verify_transfer"]).build(),
        op("transfers.cancelVerify", Write, "Stop a running check started by transfers.verify.")
            .req("id", S, "Transfer id.").cmds(&["cancel_verify"]).build(),
        op("transfers.clearCache", Destructive, "Delete leftovers of unfinished transfers (they can't be resumed afterwards).")
            .out("bytes freed").cmds(&["clear_transfer_cache"]).build(),
        op("history.list", Read, "Finished transfers (the History screen).").out("[entry]").cmds(&["get_history"]).build(),
        op("history.remove", Destructive, "Remove one entry from History (files stay).").req("id", S, "Entry id.").cmds(&["remove_history_entry"]).build(),
        op("history.clear", Destructive, "Clear History (files stay).").cmds(&["clear_history"]).build(),

        // ── shared folders (two-way synced folders with friends) ──
        op("folders.list", Read, "Shared folders on this device.").out("[folder]").cmds(&["list_pairs"]).build(),
        op("folders.status", Read, "Sync state of every shared folder.").out("[status]").cmds(&["get_folder_statuses"]).build(),
        op("folders.create", Write, "Share a local folder: returns an invite the other person accepts with folders.accept.")
            .req("folder", S, "Full path of the folder.").opt("twoWay", B, "Both sides can change it (default true).").opt("peerName", S, "Who it's for.").opt("mirror", B, "Keep deleted files recoverable.")
            .out("{pair, invite}").cmds(&["create_pair"]).build(),
        op("folders.accept", Write, "Accept a shared-folder invite into a local folder.")
            .req("invite", S, "The invite.").req("folder", S, "Full path of the local folder to use.").opt("folderName", S, "Name to show.").cmds(&["accept_pair"]).build(),
        op("folders.update", Write, "Change a shared folder's options.")
            .req("id", S, "Folder id.").opt("twoWay", B, "").opt("autoDelete", B, "").opt("deleteMode", S, "trash | permanent").opt("peerName", S, "").opt("mirror", B, "")
            .cmds(&["update_pair"]).build(),
        op("folders.remove", Destructive, "Stop sharing a folder (your files stay where they are).").req("id", S, "Folder id.").cmds(&["remove_pair"]).build(),
        op("folders.invite", Read, "The invite text for an existing shared folder.").req("id", S, "Folder id.").cmds(&["pair_invite"]).build(),
        op("folders.addPerson", Write, "Make a new invite so one more person can join a shared folder.").req("id", S, "Folder id.").out("invite").cmds(&["folder_add_person"]).build(),
        op("folders.inviteFriend", Write, "Invite a friend into a shared folder directly.")
            .req("id", S, "Folder id.").req("who", S, WHO).opt("code", S, "Reuse this pending invite.").cmds(&["invite_friend_to_folder"]).build(),
        op("folders.setMemberRole", Write, "Make a person in a shared folder view-only (true) or able to change files (false).")
            .req("id", S, "Folder id.").req("viewer", B, "true = view only.").cmds(&["set_member_role"]).build(),
        op("folders.verify", Read, "Compare a shared folder with the other side's copy.").req("id", S, "Folder id.").out("result").cmds(&["verify_folder"]).build(),
        op("folders.verifyAll", Write, "Start a comparison of every shared folder.").cmds(&["verify_folders"]).build(),
        op("folders.stopTransfer", Write, "Stop the shared folder's transfer that's running now.").req("id", S, "Folder id.").cmds(&["stop_folder_transfer"]).build(),
        op("folders.pause", Write, "Pause or resume syncing a shared folder.").req("id", S, "Folder id.").req("paused", B, "true = pause.").cmds(&["set_folder_paused"]).build(),
        op("folders.history", Read, "Recoverable (deleted or replaced) files of a shared folder.").req("id", S, "Folder id.").cmds(&["list_folder_history"]).build(),
        op("folders.historySummary", Read, "How much space recoverable copies use, per folder.").cmds(&["folder_history_summary"]).build(),
        op("folders.restoreItem", Write, "Put a recoverable file back.").req("id", S, "Folder id.").req("itemId", S, "Item id.").cmds(&["restore_folder_item"]).build(),
        op("folders.forgetItem", Destructive, "Delete one recoverable copy for good.").req("id", S, "Folder id.").req("itemId", S, "Item id.").cmds(&["forget_folder_item"]).build(),
        op("folders.clearHistory", Destructive, "Delete every recoverable copy of one folder.").req("id", S, "Folder id.").out("bytes freed").cmds(&["clear_folder_history"]).build(),
        op("folders.clearAllHistory", Destructive, "Delete every recoverable copy of every folder.").out("bytes freed").cmds(&["clear_all_folder_history"]).build(),

        // ── Locations (folders a friend or own device shares for browsing) ──
        op("locations.hosted", Read, "Folders this device shares as Locations.").out("[location]").cmds(&["list_locations"]).build(),
        op("locations.hostedStatus", Read, "Whether one of this device's Locations is reachable, how much space is left, who used it last.")
            .req("id", S, "Location id.").cmds(&["hosted_location_status"]).build(),
        op("locations.save", Write, "Share a folder as a Location, or change one (same object the Locations settings save).")
            .req("location", Obj, "{id, name, path, friendIds:[friend id], rights:{upload, manage}, byteCap} — to change one, start from locations.hosted and keep its marker and device").cmds(&["save_location"]).build(),
        op("locations.unshare", Destructive, "Stop sharing one of this device's Locations (the folder itself stays).")
            .req("id", S, "Location id.").build(),
        op("locations.activity", Read, "Recent changes people made in this device's Locations.").cmds(&["location_activity"]).build(),
        op("locations.mountCandidates", Read, "Disks and network shares this device can see (for a new Location).").cmds(&["list_mount_candidates"]).build(),
        op("locations.list", Read, "Locations a friend (or your own device) shares with you.")
            .req("who", S, WHO).out("[{id, name, rights}]").cmds(&["location_request"]).build(),
        op("locations.ls", Read, "List a folder inside a friend's Location.")
            .req("who", S, WHO).req("location", S, "Location name or id.").opt("path", S, "Folder inside it ('' = top).").opt("cursor", S, "nextCursor from the previous page.").opt("query", S, "Only names containing this.")
            .out("{entries:[{name,isDir,size,modified}], hasMore, nextCursor, total}").build(),
        op("locations.download", Write, "Download files or folders from a friend's Location into your download folder.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("paths", Strs, "Paths inside the Location.").out("{transferId, receiveId} — follow receiveId (the incoming transfer here) with transfers.wait").build(),
        op("locations.upload", Write, "Upload files into a friend's Location (if they gave you upload rights). Never overwrites: a same-named file lands beside it. An exact copy: every file goes, hidden and OS files too (only .dropbeam-* stays); anything unreadable refuses the upload before it starts; symlinks and names a Windows host can't store are listed in the result's uploadNotes.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("paths", Strs, PATHS).opt("path", S, "Folder inside it ('' = top).").dry()
            .out("the transfer").cmds(&["upload_to_location"]).build(),
        op("locations.mkdir", Write, "Make a folder inside a friend's Location (needs manage rights).")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "The new folder's path.").build(),
        op("locations.rename", Write, "Rename or move an item inside a friend's Location (needs manage rights).")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "Item path.").req("to", S, "New path.").build(),
        op("locations.trash", Destructive, "Move an item in a friend's Location to that Location's trash (recoverable there).")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "Item path.").out("{trashPath}").build(),
        op("locations.stat", Read, "Type, size, modified time and version tag of one item in a friend's Location.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "Item path.")
            .out("{isDir, size, modified, etag}").cmds(&["location_stat"]).build(),
        op("locations.readUrl", Read, "A link on this computer (http://127.0.0.1, works only here, only while DropBeam runs) that streams a file from a friend's Location without downloading it first; seeking works (HTTP Range). Hand it to a player or reader.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "File path.")
            .out("{url, kind, mime, size, etag}").cmds(&["location_read_url"]).build(),
        op("locations.cacheFile", Read, "Bring a whole file from a friend's Location into DropBeam's cache on this computer and return its path (kept a day; reused while unchanged). Use locations.download to keep a copy in your download folder.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("path", S, "File path.")
            .out("path (text)").cmds(&["location_cache_file"]).build(),
        op("locations.connection", Read, "How this device reaches the device sharing a Location right now: Local, Direct or Relay, round trip and live speed.")
            .req("who", S, WHO).out("{path, rttMs, …}").cmds(&["stream_status"]).build(),
        op("locations.clearCache", Write, "Empty the cache of streamed Location files (they're fetched again when opened).")
            .out("bytes freed").cmds(&["stream_clear_cache"]).build(),
        op("drives.status", Read, "Whether friends' and your devices' Locations show as a drive on this computer, where it's mounted, and the drive settings.")
            .cmds(&["drive_status"]).build(),
        op("drives.mount", Write, "Show Locations as a drive on this computer (Finder, File Explorer or Files).").out("where it's mounted").cmds(&["drive_mount"]).build(),
        op("drives.unmount", Write, "Stop showing Locations as a drive.").cmds(&["drive_unmount"]).build(),
        op("drives.refresh", Write, "Re-read which Locations the drive shows (after friends share or stop sharing).").cmds(&["drive_refresh"]).build(),
        op("drives.settings", Write, "Change Settings → Drives: show Locations as a drive, mount it at launch, cache size.")
            .req("settings", Obj, "{\"enabled\": true, \"autoMount\": true, \"cacheGb\": 10}").cmds(&["drive_settings_set"]).build(),
        op("speed.findSafe", Write, "Find my safe speed: send test data to an online device or friend, faster and faster, while watching the router, and recommend an upload limit (apply it with settings.update uploadLimitMbps). Takes up to a minute or two.")
            .out("{recommended limit, target, local, routerMs, …}").cmds(&["find_safe_speed"]).build(),
        op("speed.cancelFind", Write, "Stop a running speed.findSafe.").cmds(&["cancel_safe_speed"]).build(),
        op("syncedFolders.list", Read, "Local folders this device keeps copied into a friend's Location.").cmds(&["list_synced_folders"]).build(),
        op("syncedFolders.status", Read, "What each synced folder is doing now.").cmds(&["synced_folder_statuses"]).build(),
        op("syncedFolders.add", Write, "Keep a local folder copied into a friend's Location.")
            .req("who", S, WHO).req("location", S, "Location name or id.").req("localPath", S, "Full path of the local folder.").opt("path", S, "Folder inside the Location.").opt("deleteRemote", B, "Deleting here also deletes there (default false).")
            .cmds(&["add_synced_folder"]).build(),
        op("syncedFolders.update", Write, "Turn a synced folder on/off or change delete behavior.")
            .req("id", S, "Synced folder id.").opt("enabled", B, "").opt("deleteRemote", B, "").cmds(&["update_synced_folder"]).build(),
        op("syncedFolders.remove", Destructive, "Stop syncing a folder (files on both sides stay).").req("id", S, "Synced folder id.").cmds(&["remove_synced_folder"]).build(),
        op("syncedFolders.syncNow", Write, "Check a synced folder for changes now.").req("id", S, "Synced folder id.").cmds(&["sync_folder_now"]).build(),

        // ── your devices ──
        op("devices.list", Read, "This device and every device linked to your account, with which one is this.")
            .out("{endpointId, name, accountPub, devices:[{endpointId, name, deviceKind, thisDevice, needsApproval}]}").cmds(&["my_device_info"]).build(),
        op("devices.activity", Read, "What your other devices are sending or receiving right now.").cmds(&["other_device_activity"]).build(),
        op("devices.syncNow", Write, "Bring friends and chats in step with your other devices now.").cmds(&["account_sync_now"]).build(),
        op("devices.linkCancel", Write, "Cancel a 'link me' code shown on THIS device (linking itself happens in the window).").cmds(&["link_device_cancel"]).build(),
        op("devices.hostCancel", Write, "Cancel a 'join me' code shown on THIS device (linking itself happens in the window).").cmds(&["link_host_cancel"]).build(),
        op("devices.linkPreview", Read, "Check a device code before linking: who it belongs to and the safety code both screens show. (Linking itself happens in the window.)")
            .req("code", S, "dropbeamlink1:… or dropbeamjoin1:…").cmds(&["link_device_prepare"]).build(),
        op("devices.linkPending", Read, "The 'do the safety codes match?' question waiting on this device during a link, if any (also sent as a `link` event). Only the person answers it, in the window.")
            .out("{endpointId, name, safety, joining} or null").build(),
        op("devices.remove", Destructive, "Remove one of your devices from your account (on every device).").req("endpointId", S, "Device id.").cmds(&["account_remove_device"]).build(),
        op("devices.leaveAccount", Destructive, "Take THIS device out of your account (it keeps its own friends).").cmds(&["account_leave"]).build(),
        op("exec.run", Destructive, "Run a command on one of YOUR OWN linked devices (not friends'), if that device has 'Let my other devices run commands here' turned on. Output streams back; the device shows a notification and logs it.")
            .req("device", S, "One of your own devices (name or id).").req("command", S, "The command line, run by the device's shell (sh -c, or cmd /C on Windows).")
            .opt("shell", S, "sh (default on macOS/Linux), cmd (default on Windows) or powershell.").opt("cwd", S, "Folder to run in (default: home).")
            .opt("stdin", S, "Text to feed the command's input.").opt("timeoutSecs", I, "Stop it after this long (default 300, at most 86400).").dry().streams()
            .out("{exitCode, stdout, stderr, timedOut} (with streaming: stdout/stderr events first)")
            .promise("Refused unless the target is linked to your account AND has the switch on. Never runs on a friend's device.").build(),
        op("exec.allowedHere", Read, "Whether this device lets your other devices run commands here, and until when.").out("{on, until}").build(),

        // ── settings ──
        op("settings.get", Read, "All settings.").cmds(&["get_settings"]).build(),
        op("settings.update", Write, "Change settings: pass only the ones to change (same names as settings.get). Lab Mode can only be changed in the window.")
            .req("settings", Obj, "e.g. {\"downloadDir\": \"/Users/me/Downloads\", \"uploadLimitMbps\": 100}").dry().out("all settings after the change").cmds(&["update_settings"]).build(),
        op("settings.defaultDownloadDir", Read, "The download folder DropBeam uses when none is set.").cmds(&["get_default_download_dir"]).build(),
        op("profile.setPicture", Write, "Use an image file as your profile picture (friends see it).").req("path", S, "Full path of the image.").cmds(&["set_profile_avatar"]).build(),
        op("profile.clearPicture", Write, "Remove your profile picture.").cmds(&["clear_profile_avatar"]).build(),

        // ── Transfer Server ──
        op("server.check", Read, "Whether this device can be a Transfer Server (always on, space).").cmds(&["server_check_device"]).build(),
        op("server.folderSpace", Read, "Free and total space of a folder's disk.").req("path", S, "Full path.").out("[free, total] or null").cmds(&["server_folder_space"]).build(),
        op("server.status", Read, "This device's Transfer Server: on/off, space, people, held items.").cmds(&["server_status"]).build(),
        op("server.configure", Write, "Set up or change this device's Transfer Server (same fields as its settings screen).")
            .req("patch", Obj, "{name, root, capBytes, fileDays, chatDays, itemMax, access, allowed, through, paused, enabled, udpPort} — only what changes.").cmds(&["server_configure"]).build(),
        op("server.removePerson", Destructive, "Stop holding files for a person and drop what's held for them.").req("personId", S, "Person id from server.status.").cmds(&["server_remove_person"]).build(),
        op("server.restorePerson", Write, "Let a removed person use the server again.").req("personId", S, "Person id.").cmds(&["server_restore_person"]).build(),
        op("server.wipe", Destructive, "Delete everything this Transfer Server holds.").cmds(&["server_wipe"]).build(),
        op("server.disable", Destructive, "Turn this device's Transfer Server off.").opt("deleteItems", B, "Also delete what it holds.").cmds(&["server_disable"]).build(),
        op("server.setOwner", Write, "Set which account owns this Transfer Server.").req("account", S, "Account key.").cmds(&["server_set_owner"]).build(),
        op("background.status", Read, "Whether DropBeam keeps running when no one is signed in (starts with the computer and keeps sharing Locations and holding messages; the app takes over while it's open). Windows: a switch; Linux: a command to run (command); macOS: not available.")
            .out("{supported, enabled, running, current, command}").cmds(&["background_host_status"]).build(),
        op("background.set", Write, "Turn \"Keep running when no one is signed in\" on or off (Windows). Windows shows a permission (UAC) prompt on this computer's screen, so a person must be there to approve it; without that it fails and nothing changes.")
            .req("enabled", B, "true = on, false = off.").out("the new status (see background.status)").cmds(&["background_host_set"])
            .promise("Changes nothing unless the person at this computer approves the Windows permission prompt. Recorded in the agent activity log.").build(),
        op("servers.list", Read, "Transfer Servers your friends offer you.").cmds(&["mailbox_servers"]).build(),
        op("servers.prefs", Write, "Choose whether and how to use a friend's Transfer Server.")
            .req("eid", S, "Server device id.").opt("useIt", B, "").opt("holdForMe", B, "").opt("offer", S, "").opt("shareFriends", B, "").cmds(&["mailbox_server_prefs"]).build(),
        op("servers.forget", Write, "Stop using a friend's Transfer Server.").req("eid", S, "Server device id.").cmds(&["mailbox_forget_server"]).build(),
        op("servers.fetchNow", Write, "Collect anything Transfer Servers hold for you now.").cmds(&["mailbox_fetch_now"]).build(),
        op("servers.holdRoute", Read, "Which Transfer Server would hold things for a friend while they're offline.").req("who", S, WHO).cmds(&["mailbox_hold_route"]).build(),
        op("servers.pendingFiles", Read, "Files a Transfer Server holds for you that wait for your OK.").cmds(&["mailbox_pending_files"]).build(),
        op("servers.decideFile", Write, "Accept or decline a held file.").req("linkId", S, "From servers.pendingFiles.").req("accept", B, "true = download it.").cmds(&["mailbox_decide_file"]).build(),
        op("push.status", Read, "Phone notification setup status.").cmds(&["push_status"]).build(),
        op("push.setPreviews", Write, "Show message text in phone notifications (true) or not.").req("on", B, "").cmds(&["push_set_previews"]).build(),

        // ── diagnostics ──
        op("diagnostics.selfTest", Read, "Check that the network engine works on this device.").out("text").cmds(&["iroh_selftest"]).build(),
        op("diagnostics.lanBlocked", Read, "Whether the system blocks local-network access for DropBeam.").cmds(&["lan_network_blocked"]).build(),
        op("diagnostics.export", Write, "Save a diagnostics file (logs without file names or contents) and return its path.").cmds(&["export_diagnostics"]).build(),
        op("diagnostics.send", Write, "Send a diagnostics test report now.").cmds(&["diagnostics_test"]).build(),
        op("diagnostics.installHint", Read, "On macOS: advice if DropBeam runs from a place updates can't replace.").cmds(&["macos_install_hint"]).build(),

        // ── calls (docs/CALLS.md) ──
        op("calls.status", Read, "The call this device is in right now (ringing, connecting or active: who, audio or video, since when, recording state) and the last one that ended.")
            .out("{call, last, capable}").cmds(&["call_status"]).build(),
        op("calls.history", Read, "Call history, newest first: who, when, audio or video, in or out, how it ended and how long it lasted.")
            .opt("who", S, WHO).opt("limit", I, "At most this many. Default 50.")
            .out("[{friendId, name, at, outgoing, video, outcome, durationMs, text}]").cmds(&["call_history"]).build(),
        op("calls.recordings", Read, "Call recordings saved on this device (made with both people's agreement), newest first, with which file holds whose voice.")
            .out("[{dir, meta: {with, startedAt, endedAt, tracks}}]").cmds(&["call_recordings"]).build(),
        op("calls.start", Write, "Ask the person at this computer to call a friend: the call screen opens with \"Call <name>? (audio/video) — Call / Cancel\". Nothing rings and no microphone or camera opens until they click Call; then it rings all the friend's devices like any call. Recording is never started by an agent.")
            .req("who", S, WHO).opt("video", B, "Video call (default: audio only).").dry()
            .out("{confirming: true}").promise("Shows a call confirmation here; only a person's click places the call. Recorded in the agent activity log.").build(),
        op("calls.hangup", Write, "End the current call, or stop it ringing (an incoming call is declined).")
            .out("{ended: true}").cmds(&["call_hangup", "call_decline"]).build(),

        // ── live events ──
        op("events.subscribe", Read, "Stream events as they happen: transfer progress, new messages, friends coming online, typing, friend requests, linking. One JSON line per event until you disconnect (or timeoutSecs).")
            .opt("types", Strs, "Any of: transfer, chat, presence, typing, requests, friends, link. Default: all.").opt("timeoutSecs", I, "Stop after this long (default: never on the socket; 60 over MCP).").streams()
            .out("events {type, data}").build(),
        op("agents.log", Read, "The agent activity log of this device (newest last).").opt("limit", I, "Default 100.").out("[{t, agent, reason, op, summary, ok, error}]").build(),
    ]
}

/// Every `#[tauri::command]` registered in lib.rs that NO op covers, and why.
/// Window plumbing (pickers, focus, badges) and the agent switches themselves
/// (an agent must not be able to widen its own access) stay human-only.
pub const EXCLUDED: &[(&str, &str)] = &[
    ("call_capable", "the call screen reports whether its web view can do WebRTC"),
    ("call_start", "call screen WebRTC plumbing (calls.start only asks the person, who clicks Call)"),
    ("call_offer", "call screen WebRTC plumbing"),
    ("call_accept", "answering is the person's decision on their call screen"),
    ("call_answer", "answering is the person's decision on their call screen"),
    ("call_media_up", "call screen WebRTC plumbing"),
    ("call_signal", "mid-call messages between the two call screens (mute, camera, recording consent)"),
    ("call_stats", "call screen quality indicator"),
    ("call_record_open", "recording needs both people's yes on their call screens; agents never start one"),
    ("call_record_chunk", "recording plumbing"),
    ("call_record_close", "recording plumbing"),
    ("call_record_import", "recording plumbing"),
    ("pick_files", "native file picker on screen; agents pass paths"),
    ("pick_directory", "native folder picker on screen; agents pass paths"),
    ("pick_photos", "iOS photo picker on screen"),
    ("share_files", "iOS share sheet on screen"),
    ("network_changed", "iOS native shell plumbing"),
    ("lan_self_info", "iOS native shell plumbing"),
    ("lan_peer_found", "iOS native shell plumbing"),
    ("push_unregister_all", "iOS native shell plumbing"),
    ("network_shutdown", "iOS native shell plumbing"),
    ("push_set_device", "iOS native shell hands over its notification token"),
    ("push_set_voip_token", "iOS native shell plumbing (PushKit VoIP token for ringing calls)"),
    ("reveal_path", "shows a file in Finder/Explorer on screen"),
    ("open_path", "opens a file on screen"),
    ("open_url", "opens a browser on screen"),
    ("open_mailto", "opens the mail app on screen"),
    ("open_local_network_settings", "opens system settings on screen"),
    ("open_privacy_settings", "opens system settings on screen"),
    ("open_main_window", "window management"),
    ("hide_popover", "window management"),
    ("set_card_active", "window management"),
    ("set_popover_rows", "menu-bar drag layout"),
    ("traydrag_debug", "debug logging from the menu-bar drag"),
    ("frontend_log", "the window's own logging"),
    ("take_launch_file", "window startup handoff"),
    ("take_open_urls", "window startup handoff"),
    ("save_pasted_image", "clipboard paste in the chat box; agents send files by path"),
    ("paste_clipboard_image", "clipboard paste in the chat box"),
    ("clipboard_file_paths", "clipboard paste in the chat box"),
    ("set_keep_awake", "set automatically by the window while files move"),
    ("set_active_chat", "which chat is on screen (notification focus)"),
    ("set_unread_badge", "the Dock/taskbar badge"),
    ("download_gif", "GIF picker preview cache"),
    ("send_chat_gif", "GIF picker; agents send text or files"),
    ("restart_app", "would end the agent's own connection; restart from the window"),
    ("quit_app", "would end the agent's own connection; quit from the menu"),
    ("force_relay", "developer test that slows one connection on purpose"),
    ("location_thumb", "picture previews for the Locations browser grid"),
    ("location_open", "opens a file in an app on screen (agents use locations.readUrl or locations.cacheFile)"),
    ("drive_open", "opens Finder/File Explorer/Files on screen"),
    ("drive_fix_windows", "asks for administrator permission on screen"),
    // Adding a device to the account hands it the account's friends, chats and
    // `exec`: a person decides that in the window (scan the code, compare the
    // safety code). Agents keep the read-only list/preview/pending ops.
    ("link_device_begin", "Settings → Devices: linking a device to your account is the person's decision"),
    ("link_host_begin", "Settings → Devices: linking a device to your account is the person's decision"),
    ("link_device_join", "Settings → Devices: linking a device to your account is the person's decision"),
    ("link_device_send", "Settings → Devices: linking a device to your account is the person's decision"),
    ("link_confirm", "Settings → Devices: only the person compares the safety codes"),
    ("account_approve_device", "Settings → Devices: approving a device into your account is the person's decision"),
    ("agent_settings", "Settings → Agents: an agent can't change its own access"),
    ("agent_set_access", "Settings → Agents: an agent can't change its own access"),
    ("agent_set_exec", "Settings → Agents: an agent can't let itself run commands"),
    ("agent_log", "Settings → Agents list (agents use agents.log)"),
    ("agent_clear_log", "Settings → Agents: the log is the person's record of what agents did"),
    ("agent_install_cli", "Settings → Agents button (installs the command on PATH)"),
];

// ── errors and input helpers ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct OpError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
}

impl OpError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self { OpError { code, message: message.into(), preview: None } }
}
fn bad(m: impl Into<String>) -> OpError { OpError::new("invalid_input", m) }
fn failed(m: impl std::fmt::Display) -> OpError { OpError::new("failed", m.to_string()) }
fn not_found(m: impl Into<String>) -> OpError { OpError::new("not_found", m) }
pub type R = Result<Value, OpError>;
fn ok<T: Serialize>(t: T) -> R { serde_json::to_value(t).map_err(failed) }
fn done() -> R { Ok(json!({"done": true})) }

/// Strict input check against the op's schema: unknown fields, missing
/// required fields and wrong types are refused before anything runs.
pub fn validate(spec: &OpSpec, input: &Value) -> Result<(), OpError> {
    let empty = Map::new();
    let obj = match input { Value::Null => &empty, Value::Object(o) => o, _ => return Err(bad("input must be a JSON object")) };
    let props = spec.input["properties"].as_object().cloned().unwrap_or_default();
    for (k, v) in obj {
        let Some(p) = props.get(k) else {
            let mut valid: Vec<&String> = props.keys().collect();
            valid.sort();
            return Err(bad(format!("unknown field {k:?} for {}; valid fields: {}", spec.name,
                if valid.is_empty() { "none".into() } else { valid.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") })));
        };
        let fits = match p["type"].as_str() {
            Some("string") => v.is_string(),
            Some("boolean") => v.is_boolean(),
            Some("integer") => v.as_u64().is_some(),
            Some("array") => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            Some("object") => v.is_object(),
            _ => true,
        };
        if !fits { return Err(bad(format!("{k:?} must be {}", match p["type"].as_str() { Some("array") => "a list of strings", Some("integer") => "a whole number ≥ 0", Some(t) => t, None => "set" }))); }
    }
    for r in spec.input["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if obj.get(r).is_none_or(|v| v.is_null() || v.as_str() == Some("")) { return Err(bad(format!("{r:?} is required for {}", spec.name))); }
    }
    Ok(())
}

fn s(i: &Value, k: &str) -> Result<String, OpError> {
    i[k].as_str().map(str::to_owned).ok_or_else(|| bad(format!("{k:?} is required")))
}
fn os(i: &Value, k: &str) -> Option<String> { i[k].as_str().map(str::to_owned) }
fn ob(i: &Value, k: &str) -> Option<bool> { i[k].as_bool() }
fn bv(i: &Value, k: &str) -> Result<bool, OpError> { i[k].as_bool().ok_or_else(|| bad(format!("{k:?} is required"))) }
fn strs(i: &Value, k: &str) -> Vec<String> {
    i[k].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}
/// Paths an agent passes must be absolute (the CLI resolves relative ones
/// against its own working folder before sending).
fn abs_paths(i: &Value, k: &str) -> Result<Vec<String>, OpError> {
    let paths = strs(i, k);
    if paths.is_empty() { return Err(bad(format!("{k:?}: give at least one path"))); }
    if let Some(p) = paths.iter().find(|p| !Path::new(p).is_absolute()) { return Err(bad(format!("use a full path, not {p:?}"))); }
    if let Some(p) = paths.iter().find(|p| !Path::new(p).exists()) { return Err(not_found(format!("not found: {p}"))); }
    Ok(paths)
}

// ── who is who ──────────────────────────────────────────────────────────────

fn friend_view(dir: &Path, f: &Friend, own: &[String]) -> Value {
    let person = friends::thread_owner(dir, &f.id).map(|o| o.id).unwrap_or_else(|| f.id.clone());
    json!({"id": f.id, "name": f.name, "person": person, "endpointId": f.endpoint_id,
        "ownDevice": f.endpoint_id.as_ref().is_some_and(|e| own.contains(e)),
        "deviceKind": f.device_kind, "deviceOs": f.device_os, "deviceModel": f.device_model, "autoAccept": f.auto_accept})
}

fn own_ids(dir: &Path) -> Vec<String> {
    crate::account::own_devices(dir).into_iter().filter_map(|f| f.endpoint_id).collect()
}

/// A friend record from a name or id the way a person would say it. Several
/// records of ONE person (their devices) resolve to that person's record;
/// names that fit several different people are refused with the choices.
pub(crate) fn resolve(dir: &Path, who: &str) -> Result<Friend, OpError> {
    let who = who.trim();
    if who.is_empty() { return Err(bad("say who: a name or id")); }
    let all = friends::load(dir);
    if let Some(f) = all.iter().find(|f| f.id == who || f.endpoint_id.as_deref() == Some(who)) { return Ok(f.clone()); }
    let lower = who.to_lowercase();
    let pick = |hits: Vec<&Friend>| -> Option<Result<Friend, OpError>> {
        if hits.is_empty() { return None; }
        let owners: std::collections::BTreeSet<String> = hits.iter()
            .map(|f| friends::thread_owner(dir, &f.id).map(|o| o.id).unwrap_or_else(|| f.id.clone())).collect();
        if owners.len() == 1 {
            let owner = owners.into_iter().next().unwrap_or_default();
            return Some(Ok(hits.iter().find(|f| f.id == owner).copied().unwrap_or(hits[0]).clone()));
        }
        let names: Vec<String> = hits.iter().map(|f| format!("{} ({})", f.name, f.endpoint_id.as_deref().map(|e| &e[..e.len().min(8)]).unwrap_or("?"))).collect();
        Some(Err(OpError::new("ambiguous", format!("{who:?} could be: {}. Use an id instead.", names.join(", ")))))
    };
    if let Some(r) = pick(all.iter().filter(|f| f.name.to_lowercase() == lower).collect()) { return r; }
    if who.len() >= 6 {
        if let Some(r) = pick(all.iter().filter(|f| f.endpoint_id.as_deref().is_some_and(|e| e.starts_with(who))).collect()) { return r; }
    }
    if let Some(r) = pick(all.iter().filter(|f| f.name.to_lowercase().starts_with(&lower)).collect()) { return r; }
    Err(not_found(format!("no friend or device called {who:?} (see friends.list)")))
}

/// The id of the conversation a person's messages live under.
/// What `calls.start` asks the call screen: a confirmation to show, never a call
/// to place (`confirm` is always set; the screens start media only on a click).
fn dial_request(friend_id: &str, name: &str, video: bool) -> Value {
    json!({"friendId": friend_id, "name": name, "video": video, "confirm": true, "fromAgent": true})
}

fn thread_id(dir: &Path, f: &Friend) -> String {
    friends::thread_owner(dir, &f.id).map(|o| o.id).unwrap_or_else(|| f.id.clone())
}

/// One of THIS account's own devices (never a friend's).
pub(crate) fn resolve_own_device(dir: &Path, who: &str) -> Result<Friend, OpError> {
    let own = crate::account::own_devices(dir);
    let lower = who.trim().to_lowercase();
    let hit = |f: &&Friend| f.id == who || f.endpoint_id.as_deref() == Some(who) || f.name.to_lowercase() == lower
        || (who.len() >= 6 && f.endpoint_id.as_deref().is_some_and(|e| e.starts_with(who)));
    let hits: Vec<&Friend> = own.iter().filter(hit).collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 if own.is_empty() => Err(not_found("this device isn't linked to any other device of yours (Settings → Devices)")),
        0 => Err(not_found(format!("{who:?} isn't one of your linked devices; yours are: {}",
            own.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")))),
        _ => Err(OpError::new("ambiguous", format!("several of your devices fit {who:?}; use its id"))),
    }
}

/// A friend's Location by name or id, on whichever of the person's devices
/// shares it: (friend record that hosts it, location id).
async fn find_location(app: &AppHandle, dir: &Path, who: &str, location: &str) -> Result<(Friend, String), OpError> {
    let f = resolve(dir, who)?;
    let mut records = vec![f.clone()];
    records.extend(friends::person_records(dir, &f.id));
    let mut last = None;
    for r in records {
        match commands::location_request(app.state(), app.state(), r.id.clone(), json!({"kind": "locations.list"})).await {
            Ok(list) => {
                let lower = location.to_lowercase();
                if let Some(l) = list.as_array().into_iter().flatten()
                    .find(|l| l["id"] == location || l["name"].as_str().is_some_and(|n| n.to_lowercase() == lower)) {
                    return Ok((r, l["id"].as_str().unwrap_or_default().to_owned()));
                }
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last { Some(e) => failed(e), None => not_found(format!("{} doesn't share a Location called {location:?} with you", f.name)) })
}

// ── the calls ───────────────────────────────────────────────────────────────

/// Who is calling and what they asked for beyond the input.
#[derive(Clone, Default)]
pub struct Ctx {
    pub agent: String,
    pub reason: Option<String>,
    pub dry_run: bool,
}

fn state(app: &AppHandle) -> Arc<AppState> { app.state::<Arc<AppState>>().inner().clone() }

fn plan(what: Value) -> R { Ok(json!({"dryRun": true, "wouldDo": what})) }

/// Run one (non-streaming) op. `input` was validated against the schema and
/// confirmation was checked by the caller (agent_ctl::call).
pub async fn run(app: &AppHandle, name: &str, i: &Value, ctx: &Ctx) -> R {
    let st = state(app);
    let dir = st.config_dir.clone();
    let dry = ctx.dry_run;
    match name {
        "status" => {
            let me = commands::my_endpoint_id(app.state());
            let node = commands::iroh_node_id(app.state()).await.map_err(failed)?;
            let s = st.settings.lock().unwrap().clone();
            let (on, until) = crate::agent_ctl::exec_allowed_now(&dir);
            Ok(json!({"version": app.package_info().version.to_string(), "displayName": s.display_name,
                "deviceName": crate::account::device_name(&s.device_kind), "endpointId": me.or(node.clone()), "ready": node.is_some(),
                "linkedDevices": crate::account::own_devices(&dir).len(), "agentAccess": crate::agent_ctl::access_on(&dir),
                "commandsAllowedHere": on, "commandsAllowedUntil": until, "os": std::env::consts::OS}))
        }
        "ops.describe" => match os(i, "name") {
            Some(n) => find(&n).map(|o| json!(o)).ok_or_else(|| not_found(format!("no op {n:?}"))),
            None => Ok(json!({"ops": &*CATALOG, "humanOnly": EXCLUDED.iter().map(|(c, why)| json!({"command": c, "why": why})).collect::<Vec<_>>()})),
        },

        "friends.list" => {
            let own = own_ids(&dir);
            Ok(Value::Array(commands::list_friends(app.state()).iter().map(|f| friend_view(&dir, f, &own)).collect()))
        }
        "friends.myCode" => ok(commands::my_invite_code(app.state(), app.state()).map_err(failed)?),
        "friends.add" => ok(commands::add_friend_by_code(app.state(), app.state(), app.state(), app.clone(), s(i, "code")?).map_err(failed)?),
        "friends.createInvite" => ok(commands::create_friend(app.state(), app.state(), app.state(), s(i, "name")?).map_err(failed)?),
        "friends.acceptInvite" => ok(commands::accept_friend(app.state(), app.state(), app.state(), s(i, "invite")?).map_err(failed)?),
        "friends.invite" => { let f = resolve(&dir, &s(i, "who")?)?; ok(commands::friend_invite(app.state(), app.state(), f.id).map_err(failed)?) }
        "friends.requests" => ok(commands::list_friend_requests(app.state())),
        "friends.acceptRequest" => ok(commands::accept_friend_request(app.clone(), app.state(), app.state(), app.state(), s(i, "endpointId")?).map_err(failed)?),
        "friends.declineRequest" => { commands::decline_friend_request(app.clone(), app.state(), s(i, "endpointId")?, ob(i, "block").unwrap_or(false)).map_err(failed)?; done() }
        "friends.rename" => { let f = resolve(&dir, &s(i, "who")?)?; commands::rename_friend(app.state(), app.state(), f.id, s(i, "name")?).map_err(failed)?; done() }
        "friends.setAutoAccept" => { let f = resolve(&dir, &s(i, "who")?)?; commands::set_friend_auto_accept(app.state(), app.state(), f.id, bv(i, "autoAccept")?).map_err(failed)?; done() }
        "friends.remove" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            if dry { return plan(json!({"remove": f.name, "id": f.id})); }
            commands::remove_friend(app.clone(), app.state(), app.state(), f.id).map_err(failed)?; done()
        }
        "friends.block" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            if dry { return plan(json!({"block": f.name, "id": f.id})); }
            ok(commands::block_friend(app.clone(), app.state(), app.state(), f.id).map_err(failed)?)
        }
        "friends.unblock" => { commands::unblock_person(app.clone(), app.state(), s(i, "id")?).map_err(failed)?; done() }
        "friends.blocked" => ok(commands::list_blocked(app.state())),
        "friends.lookAlike" => ok(commands::friends_look_alike(app.state())),
        "friends.ping" => { let f = resolve(&dir, &s(i, "who")?)?; ok(commands::ping_friend(app.state(), app.state(), f.id).await.map_err(failed)?) }
        "friends.connection" => { let f = resolve(&dir, &s(i, "who")?)?; ok(commands::probe_connection(app.state(), app.state(), f.id).await.map_err(failed)?) }

        "chat.list" => ok(commands::list_chats(app.state()).await.map_err(failed)?),
        "chat.read" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            let since = i["since"].as_u64().unwrap_or(0);
            let limit = i["limit"].as_u64().unwrap_or(50).clamp(1, 1000) as usize;
            let msgs = commands::get_chat_messages(app.state(), thread_id(&dir, &f)).await.map_err(failed)?;
            let mut msgs: Vec<_> = msgs.into_iter().filter(|m| m.ts > since).collect();
            if msgs.len() > limit { msgs.drain(..msgs.len() - limit); }
            ok(msgs)
        }
        "chat.send" => {
            let f = resolve(&dir, &s(i, "to")?)?;
            let text = s(i, "text")?;
            if dry { return plan(json!({"to": f.name, "text": text})); }
            ok(commands::send_chat_message(app.state(), app.state(), app.clone(), thread_id(&dir, &f), text, os(i, "replyTo"), None).await.map_err(failed)?)
        }
        "chat.wait" => crate::agent_ctl::chat_wait(app, i).await,
        "calls.status" => ok(crate::calls::commands::call_status(app.state())),
        "calls.history" => {
            let who = match os(i, "who") { Some(w) => Some(thread_id(&dir, &resolve(&dir, &w)?)), None => None };
            let limit = i["limit"].as_u64().unwrap_or(50).clamp(1, 1000) as usize;
            ok(crate::calls::commands::call_history(app.state(), who, Some(limit)))
        }
        "calls.recordings" => ok(crate::calls::commands::call_recordings(app.clone(), app.state()).map_err(failed)?),
        "calls.start" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            let video = i["video"].as_bool().unwrap_or(false);
            if dry { return plan(json!({"call": f.name, "video": video})); }
            let iroh = app.state::<Arc<crate::iroh_net::IrohState>>();
            if !crate::calls::calls_of(&iroh).capable() { return Err(failed(crate::calls::UNSUPPORTED)); }
            if crate::calls::calls_of(&iroh).status()["call"].is_object() { return Err(failed("Already in a call.")); }
            // The window asks the person: only their click opens the microphone or
            // camera and rings (WebRTC lives on the call screen).
            #[cfg(desktop)]
            crate::show_main_window(app);
            app.emit_to("main", "call://dial", dial_request(&thread_id(&dir, &f), &f.name, video)).map_err(failed)?;
            Ok(json!({"confirming": true, "who": f.name, "video": video}))
        }
        "calls.hangup" => {
            let iroh = app.state::<Arc<crate::iroh_net::IrohState>>();
            let st = crate::calls::calls_of(&iroh).status();
            let Some(id) = st["call"]["id"].as_str().map(str::to_owned) else { return Err(not_found("No call right now.")) };
            if st["call"]["dir"] == "in" && st["call"]["phase"] == "ringing" { crate::calls::commands::call_decline(iroh, id); }
            else { crate::calls::commands::call_hangup(iroh, id); }
            Ok(json!({"ended": true}))
        }
        "chat.react" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            commands::react_to_message(app.state(), app.clone(), thread_id(&dir, &f), s(i, "messageId")?, s(i, "emoji")?, ob(i, "add").unwrap_or(true)).await.map_err(failed)?; done()
        }
        "chat.edit" => { let f = resolve(&dir, &s(i, "who")?)?; commands::edit_chat_message(app.state(), app.clone(), thread_id(&dir, &f), s(i, "messageId")?, s(i, "text")?).await.map_err(failed)?; done() }
        "chat.delete" => { let f = resolve(&dir, &s(i, "who")?)?; commands::delete_chat_message(app.state(), app.clone(), thread_id(&dir, &f), s(i, "messageId")?).await.map_err(failed)?; done() }
        "chat.typing" => { let f = resolve(&dir, &s(i, "who")?)?; commands::send_typing(app.state(), app.state(), thread_id(&dir, &f), bv(i, "on")?).await.map_err(failed)?; done() }
        "chat.markSeen" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            let id = thread_id(&dir, &f);
            commands::chat_mark_seen(app.state(), id.clone());
            // Same rule as the window: tell them only if read receipts are on.
            let newest = crate::chat::messages(&dir, &id).iter().filter(|m| !m.from_me).map(|m| m.ts).max().unwrap_or(0);
            if newest > 0 && st.settings.lock().unwrap().send_read_receipts {
                commands::send_read_receipt(app.state(), app.state(), id, newest).await.map_err(failed)?;
            }
            done()
        }

        "files.send" => {
            let f = resolve(&dir, &s(i, "to")?)?;
            let paths = abs_paths(i, "paths")?;
            let device = match os(i, "device") {
                None => None,
                Some(d) => {
                    let mut recs = vec![f.clone()];
                    recs.extend(friends::person_records(&dir, &f.id));
                    let lower = d.to_lowercase();
                    let hit = recs.iter().find(|r| r.endpoint_id.as_deref() == Some(d.as_str()) || r.id == d || r.name.to_lowercase() == lower
                        || r.device_model.as_deref().is_some_and(|m| m.to_lowercase() == lower)
                        || (d.len() >= 6 && r.endpoint_id.as_deref().is_some_and(|e| e.starts_with(&d))))
                        .ok_or_else(|| not_found(format!("{} has no device {d:?}", f.name)))?;
                    hit.endpoint_id.clone()
                }
            };
            if dry { return plan(json!({"to": f.name, "device": device, "paths": paths})); }
            let person = thread_id(&dir, &f);
            let u = commands::send_to_friend(app.clone(), app.state(), app.state(), person.clone(), paths.clone(), None, None, device).map_err(failed)?;
            // The window does the same: the send also lands as a card in the chat.
            let names = paths.iter().map(|p| Path::new(p).file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())).collect();
            if let Err(e) = commands::send_chat_file_note(app.state(), app.state(), app.clone(), person, names, u.bytes_total, paths, None, Some(u.id.clone())).await {
                log::info!("agent files.send: chat card not posted: {e}");
            }
            crate::agent_ctl::remember_retry(&u.id, "files.send", i);
            ok(u)
        }
        "quicksend.create" => {
            let paths = abs_paths(i, "paths")?;
            if dry { return plan(json!({"quickSend": paths})); }
            let u = commands::iroh_send(app.clone(), app.state(), paths).map_err(failed)?;
            crate::agent_ctl::remember_retry(&u.id, "quicksend.create", i);
            ok(u)
        }
        "quicksend.receive" => ok(commands::iroh_receive(app.clone(), app.state(), app.state(), s(i, "code")?).map_err(failed)?),

        "transfers.list" => Ok(crate::agent_ctl::transfers(ob(i, "active").unwrap_or(false))),
        "transfers.wait" => crate::agent_ctl::transfer_wait(&s(i, "id")?, i["timeoutSecs"].as_u64().unwrap_or(3600)).await,
        "transfers.cancel" => { commands::cancel_transfer(app.clone(), app.state(), s(i, "id")?); done() }
        "transfers.pause" => { commands::pause_transfer(app.clone(), app.state(), s(i, "id")?); done() }
        "transfers.retry" => {
            let id = s(i, "id")?;
            let (op_name, input) = crate::agent_ctl::retry_payload(&id)
                .ok_or_else(|| not_found("DropBeam only resumes sends an agent started this session; resume others from the window"))?;
            if let Some(t) = crate::agent_ctl::transfer(&id) {
                if !matches!(t["state"].as_str(), Some("failed" | "paused" | "canceled")) { return Err(OpError::new("rejected", "that transfer hasn't stopped")); }
            }
            Box::pin(run(app, op_name, &input, ctx)).await
        }
        "transfers.respond" => { commands::respond_to_offer(app.state(), s(i, "id")?, bv(i, "accept")?, os(i, "saveTo")); done() }
        "transfers.verify" => { commands::verify_transfer(app.clone(), app.state(), s(i, "id")?).map_err(failed)?; done() }
        "transfers.cancelVerify" => { commands::cancel_verify(app.state(), s(i, "id")?); done() }
        "transfers.clearCache" => ok(commands::clear_transfer_cache(app.state(), app.state())),
        "history.list" => ok(commands::get_history(app.state())),
        "history.remove" => { crate::history::remove_history_entry(app.state(), s(i, "id")?).map_err(failed)?; done() }
        "history.clear" => { commands::clear_history(app.state()); done() }

        "folders.list" => ok(commands::list_pairs(app.state())),
        "folders.status" => ok(commands::get_folder_statuses(app.state())),
        "folders.create" => ok(commands::create_pair(app.state(), app.state(), app.state(), s(i, "folder")?, ob(i, "twoWay").unwrap_or(true), os(i, "peerName"), ob(i, "mirror")).map_err(failed)?),
        "folders.accept" => ok(commands::accept_pair(app.state(), app.state(), app.state(), s(i, "invite")?, s(i, "folder")?, os(i, "folderName")).map_err(failed)?),
        "folders.update" => {
            let mode = match os(i, "deleteMode").as_deref() {
                None => None, Some("trash") => Some(crate::models::DeleteMode::Trash), Some("permanent") => Some(crate::models::DeleteMode::Permanent),
                Some(o) => return Err(bad(format!("deleteMode must be trash or permanent, not {o:?}"))),
            };
            ok(commands::update_pair(app.state(), app.state(), s(i, "id")?, ob(i, "twoWay"), ob(i, "autoDelete"), mode, os(i, "peerName"), ob(i, "mirror")).map_err(failed)?)
        }
        "folders.remove" => { commands::remove_pair(app.state(), app.state(), s(i, "id")?).map_err(failed)?; done() }
        "folders.invite" => ok(commands::pair_invite(app.state(), app.state(), s(i, "id")?).map_err(failed)?),
        "folders.addPerson" => ok(commands::folder_add_person(app.state(), app.state(), app.state(), app.clone(), s(i, "id")?).map_err(failed)?),
        "folders.inviteFriend" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            commands::invite_friend_to_folder(app.state(), app.state(), app.state(), app.clone(), s(i, "id")?, f.id, os(i, "code")).map_err(failed)?; done()
        }
        "folders.setMemberRole" => { commands::set_member_role(app.clone(), app.state(), app.state(), app.state(), s(i, "id")?, bv(i, "viewer")?).map_err(failed)?; done() }
        "folders.verify" => ok(commands::verify_folder(app.state(), s(i, "id")?).await.map_err(failed)?),
        "folders.verifyAll" => { commands::verify_folders(app.state()); done() }
        "folders.stopTransfer" => { commands::stop_folder_transfer(app.state(), s(i, "id")?); done() }
        "folders.pause" => { commands::set_folder_paused(app.state(), s(i, "id")?, bv(i, "paused")?); done() }
        "folders.history" => ok(commands::list_folder_history(app.state(), s(i, "id")?)),
        "folders.historySummary" => ok(commands::folder_history_summary(app.state()).await.map_err(failed)?),
        "folders.restoreItem" => { commands::restore_folder_item(app.state(), s(i, "id")?, s(i, "itemId")?).map_err(failed)?; done() }
        "folders.forgetItem" => { commands::forget_folder_item(app.clone(), app.state(), s(i, "id")?, s(i, "itemId")?); done() }
        "folders.clearHistory" => ok(commands::clear_folder_history(app.clone(), app.state(), s(i, "id")?).await.map_err(failed)?),
        "folders.clearAllHistory" => ok(commands::clear_all_folder_history(app.clone(), app.state()).await.map_err(failed)?),

        "locations.hosted" => ok(commands::list_locations(app.state()).await.map_err(failed)?),
        "locations.hostedStatus" => ok(commands::hosted_location_status(app.state(), s(i, "id")?).await.map_err(failed)?),
        "locations.save" => {
            // A misspelt field (friend_ids) would otherwise be dropped silently and share with no one.
            const KNOWN: [&str; 9] = ["id", "name", "path", "friendIds", "rights", "byteCap", "device", "marker", "safePublish"];
            if let Some(k) = i["location"].as_object().and_then(|o| o.keys().find(|k| !KNOWN.contains(&k.as_str()))) {
                return Err(bad(format!("location: unknown field \"{k}\" (fields: {})", KNOWN.join(", "))));
            }
            let l: crate::locations::Location = serde_json::from_value(i["location"].clone()).map_err(|e| bad(format!("location: {e}")))?;
            ok(commands::save_location(app.clone(), app.state(), app.state(), Some(l), None).await.map_err(failed)?)
        }
        "locations.unshare" => ok(commands::save_location(app.clone(), app.state(), app.state(), None, Some(s(i, "id")?)).await.map_err(failed)?),
        "locations.activity" => ok(commands::location_activity(app.state())),
        "locations.mountCandidates" => ok(crate::mounts::list_mount_candidates().await),
        "locations.list" => {
            let f = resolve(&dir, &s(i, "who")?)?;
            let mut recs = vec![f.clone()];
            recs.extend(friends::person_records(&dir, &f.id));
            let mut out = vec![];
            let mut last_err = None;
            for r in recs {
                match commands::location_request(app.state(), app.state(), r.id.clone(), json!({"kind": "locations.list"})).await {
                    Ok(list) => out.extend(list.as_array().cloned().unwrap_or_default().into_iter().map(|mut l| { l["device"] = json!(r.name); l })),
                    Err(e) => last_err = Some(e),
                }
            }
            if out.is_empty() { if let Some(e) = last_err { return Err(failed(e)); } }
            Ok(Value::Array(out))
        }
        "locations.ls" | "locations.download" | "locations.mkdir" | "locations.rename" | "locations.trash" => {
            let (host, loc) = find_location(app, &dir, &s(i, "who")?, &s(i, "location")?).await?;
            let path = os(i, "path").unwrap_or_default().trim_matches('/').to_owned();
            let mut req = json!({"kind": name, "id": loc, "rel_path": path});
            match name {
                "locations.ls" => {
                    if let Some(c) = os(i, "cursor") { req["cursor"] = json!(c); }
                    req["query"] = json!(os(i, "query").unwrap_or_default());
                    req["sort"] = json!("name");
                }
                "locations.download" => { req["rel_path"] = json!(""); req["paths"] = json!(strs(i, "paths").iter().map(|p| p.trim_matches('/').to_owned()).collect::<Vec<_>>()); }
                "locations.rename" => req["to"] = json!(s(i, "to")?.trim_matches('/')),
                _ => {}
            }
            if name != "locations.download" {
                return ok(commands::location_request(app.state(), app.state(), host.id, req).await.map_err(failed)?);
            }
            // The host pushes the files back as a new incoming transfer with its
            // own id here: find it, so the caller can follow it (transfers.wait).
            let wanted: Vec<String> = strs(i, "paths").iter().filter_map(|p| p.trim_matches('/').rsplit('/').next().map(str::to_owned)).collect();
            let mut events = crate::agent_ctl::events();
            let before: Vec<Value> = crate::agent_ctl::transfers(false).as_array().cloned().unwrap_or_default();
            let mut r = commands::location_request(app.state(), app.state(), host.id, req).await.map_err(failed)?;
            if r["transferId"].is_string() {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, events.recv()).await {
                    let t = &ev["data"];
                    let names: Vec<&str> = t["fileNames"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    let ours = t["peer"] == host.name.as_str() || wanted.iter().any(|w| names.contains(&w.as_str()));
                    if ev["type"] == "transfer" && t["direction"] == "receive" && ours && !before.iter().any(|b| b["id"] == t["id"]) {
                        r["receiveId"] = t["id"].clone();
                        break;
                    }
                }
            }
            Ok(r)
        }
        "locations.upload" => {
            let paths = abs_paths(i, "paths")?;
            let (host, loc) = find_location(app, &dir, &s(i, "who")?, &s(i, "location")?).await?;
            let target = crate::locations::Target { location_id: loc, rel_path: os(i, "path").unwrap_or_default().trim_matches('/').to_owned() };
            if dry { return plan(json!({"to": host.name, "location": target.location_id, "path": target.rel_path, "paths": paths})); }
            let u = commands::upload_to_location(app.clone(), app.state(), app.state(), host.id, target, paths, Some(false)).await.map_err(failed)?;
            crate::agent_ctl::remember_retry(&u.id, "locations.upload", i);
            ok(u)
        }
        "locations.stat" | "locations.readUrl" | "locations.cacheFile" => {
            let (host, loc) = find_location(app, &dir, &s(i, "who")?, &s(i, "location")?).await?;
            let path = s(i, "path")?.trim_matches('/').to_owned();
            match name {
                "locations.stat" => ok(crate::stream::location_stat(app.state(), host.id, loc, path).await.map_err(failed)?),
                "locations.readUrl" => ok(crate::stream::location_read_url(app.state(), host.id, loc, path).await.map_err(failed)?),
                _ => ok(crate::stream::location_cache_file(app.clone(), app.state(), host.id, loc, path).await.map_err(failed)?),
            }
        }
        "locations.connection" => { let f = resolve(&dir, &s(i, "who")?)?; ok(crate::stream::stream_status(app.state(), f.id).map_err(failed)?) }
        "locations.clearCache" => ok(crate::stream::stream_clear_cache().map_err(failed)?),
        "drives.status" => ok(crate::drive::drive_status().await.map_err(failed)?),
        "drives.mount" => ok(crate::drive::drive_mount().await.map_err(failed)?),
        "drives.unmount" => { crate::drive::drive_unmount().await.map_err(failed)?; done() }
        "drives.refresh" => ok(crate::drive::drive_refresh().await.map_err(failed)?),
        "drives.settings" => {
            // Partial: start from what's saved, change only what was given.
            let mut cur = serde_json::to_value(crate::stream::load_settings(&dir)).map_err(failed)?;
            for (k, v) in i["settings"].as_object().into_iter().flatten() {
                if cur.get(k).is_none() { return Err(bad(format!("no drive setting {k:?} (enabled, autoMount, cacheGb)"))); }
                cur[k] = v.clone();
            }
            let next: crate::stream::DriveSettings = serde_json::from_value(cur).map_err(|e| bad(format!("settings: {e}")))?;
            ok(crate::drive::drive_settings_set(next).await.map_err(failed)?)
        }
        "speed.findSafe" => ok(commands::find_safe_speed(app.clone(), app.state(), app.state()).await.map_err(failed)?),
        "speed.cancelFind" => { commands::cancel_safe_speed(); done() }
        "syncedFolders.list" => ok(crate::location_sync::list_synced_folders(app.state())),
        "syncedFolders.status" => ok(crate::location_sync::synced_folder_statuses()),
        "syncedFolders.add" => {
            let (host, loc) = find_location(app, &dir, &s(i, "who")?, &s(i, "location")?).await?;
            ok(crate::location_sync::add_synced_folder(app.state(), host.id, loc, os(i, "path").unwrap_or_default(), s(i, "localPath")?, ob(i, "deleteRemote").unwrap_or(false)).map_err(failed)?)
        }
        "syncedFolders.update" => ok(crate::location_sync::update_synced_folder(app.state(), s(i, "id")?, ob(i, "enabled"), ob(i, "deleteRemote")).map_err(failed)?),
        "syncedFolders.remove" => ok(crate::location_sync::remove_synced_folder(app.state(), s(i, "id")?).map_err(failed)?),
        "syncedFolders.syncNow" => { crate::location_sync::sync_folder_now(s(i, "id")?).map_err(failed)?; done() }

        "devices.list" => ok(crate::link::my_device_info(app.state(), app.state()).map_err(failed)?),
        "devices.activity" => ok(crate::device_activity::other_device_activity()),
        "devices.syncNow" => { crate::account::account_sync_now(); done() }
        "devices.linkCancel" => { crate::link::link_device_cancel(app.state()); done() }
        "devices.hostCancel" => { crate::link::link_host_cancel(app.state()); done() }
        "devices.linkPreview" => ok(crate::link::link_device_prepare(app.state(), app.state(), s(i, "code")?).await.map_err(failed)?),
        "devices.remove" => { crate::account::account_remove_device(app.clone(), app.state(), s(i, "endpointId")?).map_err(failed)?; done() }
        "devices.leaveAccount" => { crate::account::account_leave(app.clone(), app.state(), app.state()).await.map_err(failed)?; done() }
        "devices.linkPending" => Ok(crate::agent_ctl::link_pending()),
        "exec.allowedHere" => { let (on, until) = crate::agent_ctl::exec_allowed_now(&dir); Ok(json!({"on": on, "until": until})) }
        "exec.run" => crate::agent_exec::run_collect(app, i, ctx).await,

        "settings.get" => ok(commands::get_settings(app.state())),
        "settings.update" => {
            let mut cur = serde_json::to_value(commands::get_settings(app.state())).map_err(failed)?;
            let patch = i["settings"].as_object().ok_or_else(|| bad("settings must be an object"))?;
            for (k, v) in patch {
                if matches!(k.as_str(), "labModeEnabled" | "labOperatorId") { return Err(OpError::new("forbidden", "Lab Mode can only be changed in the DropBeam window")); }
                if cur.get(k).is_none() { return Err(bad(format!("no setting {k:?} (see settings.get)"))); }
                cur[k] = v.clone();
            }
            let next: crate::models::Settings = serde_json::from_value(cur).map_err(|e| bad(format!("settings: {e}")))?;
            if dry { return plan(json!(next)); }
            ok(commands::update_settings(app.clone(), app.state(), app.state(), next).map_err(failed)?)
        }
        "settings.defaultDownloadDir" => ok(commands::get_default_download_dir(app.clone())),
        "profile.setPicture" => ok(commands::set_profile_avatar(app.clone(), app.state(), Some(s(i, "path")?)).await.map_err(failed)?),
        "profile.clearPicture" => ok(commands::clear_profile_avatar(app.clone(), app.state()).map_err(failed)?),

        "server.check" => ok(crate::mailbox::cmds::server_check_device(app.state()).await.map_err(failed)?),
        "server.folderSpace" => ok(crate::mailbox::cmds::server_folder_space(s(i, "path")?).await),
        "server.status" => ok(crate::mailbox::cmds::server_status(app.state()).await.map_err(failed)?),
        "server.configure" => {
            let patch: crate::mailbox::cmds::ServerPatch = serde_json::from_value(i["patch"].clone()).map_err(|e| bad(format!("patch: {e}")))?;
            ok(crate::mailbox::cmds::server_configure(app.clone(), app.state(), app.state(), patch).await.map_err(failed)?)
        }
        "server.removePerson" => ok(crate::mailbox::cmds::server_remove_person(app.clone(), app.state(), app.state(), s(i, "personId")?).await.map_err(failed)?),
        "server.restorePerson" => ok(crate::mailbox::cmds::server_restore_person(app.clone(), app.state(), app.state(), s(i, "personId")?).await.map_err(failed)?),
        "server.wipe" => ok(crate::mailbox::cmds::server_wipe(app.clone(), app.state()).await.map_err(failed)?),
        "server.disable" => ok(crate::mailbox::cmds::server_disable(app.clone(), app.state(), app.state(), ob(i, "deleteItems").unwrap_or(false)).await.map_err(failed)?),
        "server.setOwner" => ok(crate::mailbox::cmds::server_set_owner(app.clone(), app.state(), app.state(), s(i, "account")?).await.map_err(failed)?),
        "background.status" => ok(crate::background_host::background_host_status(app.state()).await.map_err(failed)?),
        "background.set" => ok(crate::background_host::background_host_set(app.state(), bv(i, "enabled")?).await.map_err(failed)?),
        "servers.list" => ok(crate::mailbox::cmds::mailbox_servers(app.state())),
        "servers.prefs" => ok(crate::mailbox::cmds::mailbox_server_prefs(app.clone(), app.state(), app.state(), s(i, "eid")?, ob(i, "useIt"), ob(i, "holdForMe"), os(i, "offer"), ob(i, "shareFriends")).map_err(failed)?),
        "servers.forget" => ok(crate::mailbox::cmds::mailbox_forget_server(app.clone(), app.state(), app.state(), s(i, "eid")?)),
        "servers.fetchNow" => { crate::mailbox::cmds::mailbox_fetch_now(); done() }
        "servers.holdRoute" => { let f = resolve(&dir, &s(i, "who")?)?; ok(crate::mailbox::cmds::mailbox_hold_route(app.state(), app.state(), thread_id(&dir, &f))) }
        "servers.pendingFiles" => ok(crate::mailbox::cmds::mailbox_pending_files(app.state())),
        "servers.decideFile" => { crate::mailbox::cmds::mailbox_decide_file(app.clone(), app.state(), s(i, "linkId")?, bv(i, "accept")?); done() }
        "push.status" => ok(crate::mailbox::cmds::push_status(app.state())),
        "push.setPreviews" => { crate::mailbox::cmds::push_set_previews(app.clone(), app.state(), app.state(), bv(i, "on")?); done() }

        "diagnostics.selfTest" => ok(commands::iroh_selftest(app.state()).await.map_err(failed)?),
        "diagnostics.lanBlocked" => ok(commands::lan_network_blocked()),
        "diagnostics.export" => ok(commands::export_diagnostics(app.clone(), app.state()).await.map_err(failed)?),
        "diagnostics.send" => ok(commands::diagnostics_test(app.clone(), app.state()).await.map_err(failed)?),
        "diagnostics.installHint" => ok(commands::macos_install_hint()),
        "agents.log" => Ok(Value::Array(crate::agent_ctl::read_log(&dir, i["limit"].as_u64().unwrap_or(100) as usize))),
        "events.subscribe" => Err(OpError::new("streaming", "events.subscribe streams; call it with stream: true on the socket, or through `dropbeam events`")),
        other => Err(OpError::new("unknown_op", format!("no op {other:?} (see ops.describe)"))),
    }
}

/// One short line for the activity log: the op and the parts of its input a
/// person would want to see (message text and paths shortened).
pub fn summarize(name: &str, i: &Value) -> String {
    let mut parts = vec![];
    for k in ["to", "who", "device", "from", "location", "path", "id", "endpointId", "code", "name", "command", "text"] {
        if let Some(v) = i[k].as_str() {
            let v: String = if matches!(k, "code") { format!("{}…", v.chars().take(14).collect::<String>()) }
                else if v.chars().count() > 80 { format!("{}…", v.chars().take(80).collect::<String>()) } else { v.to_owned() };
            parts.push(format!("{k}={v:?}"));
        }
    }
    let paths = strs(i, "paths");
    if !paths.is_empty() {
        let first = Path::new(&paths[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        parts.push(if paths.len() == 1 { format!("file={first:?}") } else { format!("files={first:?}+{}", paths.len() - 1) });
    }
    if let Some(o) = i["settings"].as_object() { parts.push(format!("settings={}", o.keys().cloned().collect::<Vec<_>>().join(","))); }
    if parts.is_empty() { name.to_owned() } else { format!("{name} {}", parts.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_dial_only_asks_the_person() {
        let d = dial_request("f1", "Bea", true);
        assert_eq!(d["confirm"], true);
        assert_eq!(d["fromAgent"], true);
        assert_eq!((d["name"].as_str(), d["video"].as_bool()), (Some("Bea"), Some(true)));
        // The op can't open media itself: answering, offering and recording stay human-only.
        for c in ["call_start", "call_offer", "call_accept", "call_answer", "call_record_open"] {
            assert!(EXCLUDED.iter().any(|e| e.0 == c), "{c}");
            assert!(!CATALOG.iter().any(|o| o.commands.contains(&c)), "{c}");
        }
    }

    /// Every command lib.rs registers, as written in `generate_handler![…]`.
    fn registered_commands() -> Vec<String> {
        let src = include_str!("lib.rs");
        let start = src.find("generate_handler![").expect("generate_handler") + "generate_handler![".len();
        let body = &src[start..start + src[start..].find("])").expect("end of handler list")];
        body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
            .map(|l| l.trim_end_matches(',').rsplit("::").next().unwrap_or("").to_owned()).collect()
    }

    /// Fails when a new Tauri command is added without deciding whether agents
    /// get it: add it to an op's `.cmds(…)` or to EXCLUDED with the reason.
    #[test]
    fn every_command_has_a_decision() {
        let registered = registered_commands();
        assert!(registered.len() > 100, "parsed only {} commands", registered.len());
        let covered: Vec<&str> = CATALOG.iter().flat_map(|o| o.commands.iter().copied()).collect();
        let excluded: Vec<&str> = EXCLUDED.iter().map(|e| e.0).collect();
        let missing: Vec<&String> = registered.iter().filter(|c| !covered.contains(&c.as_str()) && !excluded.contains(&c.as_str())).collect();
        assert!(missing.is_empty(), "commands with no agent decision (add to an op's cmds or EXCLUDED): {missing:?}");
        let stale: Vec<&&str> = covered.iter().chain(excluded.iter()).filter(|c| !registered.iter().any(|r| r == **c)).collect();
        assert!(stale.is_empty(), "decisions for commands lib.rs doesn't register: {stale:?}");
        let both: Vec<&&str> = excluded.iter().filter(|c| covered.contains(c)).collect();
        assert!(both.is_empty(), "both covered and excluded: {both:?}");
    }

    /// Adding a device to the account (it gets friends, chats and `exec`) is the
    /// person's decision in the window: no op links, joins, confirms or approves.
    #[test]
    fn device_linking_is_window_only() {
        for name in ["devices.linkBegin", "devices.hostBegin", "devices.join", "devices.confirmLink", "devices.approve"] { assert!(find(name).is_none(), "{name}"); }
        for c in ["link_device_begin", "link_host_begin", "link_device_join", "link_device_send", "link_confirm", "account_approve_device"] {
            assert!(EXCLUDED.iter().any(|e| e.0 == c), "{c}");
            assert!(!CATALOG.iter().any(|o| o.commands.contains(&c)), "{c}");
        }
        for name in ["devices.list", "devices.linkPreview", "devices.linkPending"] { assert_eq!(find(name).unwrap().risk, Risk::Read, "{name}"); }
    }

    #[test]
    fn catalog_is_well_formed() {
        let mut names = std::collections::HashSet::new();
        for o in CATALOG.iter() {
            assert!(names.insert(o.name), "duplicate op {}", o.name);
            assert!(!o.description.is_empty() && o.description.len() < 600, "{}", o.name);
            assert!(o.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'), "{}", o.name);
            assert_eq!(o.input["type"], "object");
            assert_eq!(o.input["additionalProperties"], false);
            for r in o.input["required"].as_array().unwrap() { assert!(o.input["properties"].get(r.as_str().unwrap()).is_some(), "{}: {r}", o.name); }
            if o.risk == Risk::Destructive { assert!(o.confirm && o.input["properties"].get("confirm").is_some(), "{}", o.name); }
        }
    }

    #[test]
    fn validation_is_strict() {
        let send = find("chat.send").unwrap();
        assert!(validate(send, &json!({"to": "Al", "text": "hi"})).is_ok());
        assert_eq!(validate(send, &json!({"to": "Al"})).unwrap_err().code, "invalid_input");
        let e = validate(send, &json!({"to": "Al", "text": "hi", "txt": 1})).unwrap_err();
        assert!(e.message.contains("unknown field") && e.message.contains("text"), "{}", e.message);
        assert!(validate(send, &json!({"to": "Al", "text": 5})).is_err());
        assert!(validate(send, &json!([1])).is_err());
        let files = find("files.send").unwrap();
        assert!(validate(files, &json!({"to": "Al", "paths": ["/a", 3]})).is_err());
        assert!(validate(files, &json!({"to": "Al", "paths": ["/a"], "dryRun": true})).is_ok());
        assert!(validate(find("status").unwrap(), &Value::Null).is_ok());
        assert!(validate(find("friends.remove").unwrap(), &json!({"who": "x", "confirm": true})).is_ok());
    }

    #[test]
    fn summary_is_short_and_hides_codes() {
        let s = summarize("chat.send", &json!({"to": "Alex", "text": "x".repeat(300)}));
        assert!(s.starts_with("chat.send to=\"Alex\"") && s.len() < 140, "{s}");
        let s = summarize("friends.add", &json!({"code": "dropbeam:abcdefghijklmnopqrstuvwxyz"}));
        assert!(!s.contains("xyz"), "{s}");
        assert_eq!(summarize("files.send", &json!({"to": "A", "paths": ["/x/a.txt", "/x/b"]})), "files.send to=\"A\" files=\"a.txt\"+1");
    }

    #[test]
    fn resolves_people_and_refuses_ambiguity() {
        let dir = std::env::temp_dir().join(format!("dropbeam-ops-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = friends::upsert_by_endpoint(&dir, &"a".repeat(64), "Alex");
        let _b = friends::upsert_by_endpoint(&dir, &"b".repeat(64), "Alexis");
        assert_eq!(resolve(&dir, "alex").unwrap().id, a.id);
        assert_eq!(resolve(&dir, &a.id).unwrap().id, a.id);
        assert_eq!(resolve(&dir, "aaaaaaa").unwrap().id, a.id);
        assert_eq!(resolve(&dir, "Ale").unwrap_err().code, "ambiguous");
        assert_eq!(resolve(&dir, "Zed").unwrap_err().code, "not_found");
        // A friend is never "one of your own devices".
        assert_eq!(resolve_own_device(&dir, "Alex").unwrap_err().code, "not_found");
        let _ = std::fs::remove_dir_all(dir);
    }
}
