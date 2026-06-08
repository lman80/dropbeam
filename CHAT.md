# Chat (experimental) — `feature/chat` branch

Direct messaging + in-conversation file sharing with friends, riding the **same
iroh endpoint** as every other transfer. This is an experimental branch, not yet
merged to `main` or released.

## What works

- **Text chat** with any friend, real-time, over iroh (dial-by-EndpointId).
- **File sharing in the thread** — the paperclip sends the file via the normal
  friend transfer (so it lands in Downloads + History + the HUD on the other
  side) *and* drops a file card into the conversation on both sides.
- **Persistence** — every conversation is stored in `chats.json` (capped at the
  last 2000 messages per friend) and survives restarts.
- **Unread badges** on the Chat tab + per-conversation; **online dots** reuse the
  same presence signal as Friends (recent contact or a live folder peer).
- **Shared-folder integration** — if you share a folder with the friend, the
  conversation header has an *Open shared folder* button, and the folder's
  online state feeds the presence dot.
- A soft incoming cue plays for new messages (respects the global sound toggle),
  main-window only.

## Design

One ALPN (`dropbeam/1`) carries every stream; each is dispatched by a `kind`
field in its first JSON frame. Chat adds:

- **`kind: "chat"`** frame: `{ msgKind: "text"|"file", friendId, id, text?, files?,
  bytes?, ts }`. Sent by `iroh_net::send_chat()` (mirrors `send_folder_ctrl`),
  handled by the `"chat"` arm in `serve_stream`.
- The sender is identified by their EndpointId, falling back to the `friendId`
  carried in the frame; receiving a chat also *learns* the sender's EndpointId
  (self-healing, like `friend-hello`).

Backend: `src-tauri/src/chat.rs` (store) + commands `get_chat_messages`,
`list_chats`, `send_chat_message`, `send_chat_file_note`. Events: `chat://message`.

Frontend: `src/views/ChatView.tsx`, store slice in `src/store.ts`
(`chats` / `chatOverview` / `chatUnread` / `activeChatId`), API in `src/lib/api.ts`.

## Known limitations (v1)

- **Online-only delivery.** A message to an offline friend is stored locally and
  shown in your thread, but there is **no store-and-forward** yet — it is not
  re-sent when they come back. (The file path already has offline catch-up; chat
  does not.)
- Chat lives in the **main window only** (not the menu-bar popover).
- Folder-derived friends that never exchanged an EndpointId can't be reached
  until one is learned (the receive path now learns it on first contact).

## Next steps (if we keep it)

1. Store-and-forward: queue undelivered messages and flush on presence/reconnect
   (reuse the folder sender's queue+backoff pattern).
2. Read receipts / delivery ticks.
3. Open a chat file card straight to its saved location.
4. Optional: surface chat in the menu-bar popover.
