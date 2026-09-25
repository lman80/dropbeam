import { MobileHeader } from '../components/MobileHeader'
import { TransferCard } from '../components/TransferCard'
import { ChevronLeft } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence, motion } from 'framer-motion'
import {
  ArrowDown,
  ArrowUp,
  Ban,
  ChevronDown,
  ChevronUp,
  Copy,
  CornerUpLeft,
  Flag,
  FolderOpen,
  MessageCircle,
  MoreHorizontal,
  Paperclip,
  Pencil,
  Play,
  Search,
  Smile,
  SquarePen,
  Trash2,
  X,
} from 'lucide-react'
import { api, fileSrc, HAS_TAURI, type ChatMessage, type ConnDetail, type Friend, type TransferUpdate } from '../lib/api'
import { useStore, byOrder, type FolderActivityEvent } from '../store'
import { completedChatItems, restoredChatTransfer } from '../lib/chatTransfer'
import { ChatTransferProgress } from '../components/ChatTransferProgress'
import { ConnInfo } from '../components/ConnInspector'
import { GifPicker } from '../components/GifPicker'
import { avatarGradient } from '../lib/avatar'
import { FriendAvatar } from '../components/FriendAvatar'
import { useOwnDeviceLabels } from '../lib/ownDevices'
import { personGroups } from '../lib/deviceIcons'
import { FileIcon as TypeIcon, fileKind as typeKind } from '../components/FileIcon'
import { formatBytes } from '../lib/format'
import { linkify } from '../lib/linkify'
import { friendOnlineState, friendPresence, presenceLabel } from '../lib/presence'
import { EmptyState, IconButton, MenuButton, MenuPopover, type MenuItem } from '../components/ui'

/** Stable empty array so the messages selector doesn't return a fresh ref each render. */
const EMPTY_MSGS: ChatMessage[] = []

/** Quick-reaction emojis (the hover tray) + a compact composer emoji set. */
const QUICK = ['👍', '❤️', '😂', '🔥', '😮', '😢', '🙏']
const EMOJIS =
  '😀 😂 🥹 😊 😍 😎 🤩 🥳 😅 😭 😡 🤔 🙄 😴 🤝 🙏 👍 👎 👏 🙌 💪 🤞 👌 ✌️ 🔥 ✨ ⭐ 🎉 🎈 💯 ❤️ 🧡 💛 💚 💙 💜 🖤 💔 💖 ✅ ❌ ⚡ 💡 📎 📁 🎁 🍕 ☕ 🍺 🎵 🚀 🌟 👀 😬 😏'.split(
    ' ',
  )

const IMG = /\.(png|jpe?g|gif|webp|bmp|svg|avif|heic)$/i
const VIDEO = /\.(mp4|mov|m4v|webm|ogv)$/i
const AUDIO = /\.(mp3|wav|m4a|aac|flac|ogg|aiff)$/i
const TEXT =
  /\.(txt|md|markdown|csv|tsv|log|json|ya?ml|xml|html?|css|jsx?|tsx?|rs|py|go|java|kt|c|cc|cpp|h|hpp|sh|bash|zsh|toml|ini|conf|sql|rb|php|swift)$/i

type Kind = 'image' | 'video' | 'audio' | 'text' | 'file'
function fileKind(name: string | undefined): Kind {
  if (!name) return 'file'
  if (IMG.test(name)) return 'image'
  if (VIDEO.test(name)) return 'video'
  if (AUDIO.test(name)) return 'audio'
  if (TEXT.test(name)) return 'text'
  return 'file'
}

const DAY_MS = 24 * 60 * 60 * 1000
/** Within one day, a quiet time label appears after a pause this long. */
const TIME_GAP_MS = 60 * 60 * 1000

/** A short wall-clock label, e.g. "3:42 PM". */
function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
}
const startOfDay = (ms: number) => {
  const d = new Date(ms)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}
const sameDay = (a: number, b: number) => startOfDay(a) === startOfDay(b)
/** A day label: Today / Yesterday / weekday (this week) / a date. */
function dayLabel(ms: number): string {
  const today = startOfDay(Date.now())
  const day = startOfDay(ms)
  if (day === today) return 'Today'
  if (day === today - DAY_MS) return 'Yesterday'
  if (today - day < 6 * DAY_MS) return new Date(ms).toLocaleDateString([], { weekday: 'long' })
  const d = new Date(ms)
  return d.toLocaleDateString([], {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
  })
}
/** The time column of the conversation list: 3:42 PM / Yesterday / Tuesday / 9/18/26. */
function listTime(ms: number | undefined): string {
  if (!ms) return ''
  const today = startOfDay(Date.now())
  const day = startOfDay(ms)
  if (day === today) return clock(ms)
  if (day === today - DAY_MS) return 'Yesterday'
  if (today - day < 6 * DAY_MS) return new Date(ms).toLocaleDateString([], { weekday: 'long' })
  return new Date(ms).toLocaleDateString([], { month: 'numeric', day: 'numeric', year: '2-digit' })
}
/** Full timestamp for a tooltip. */
function fullTime(ms: number): string {
  return `${dayLabel(ms)} ${clock(ms)}`
}

/** The engine's list preview carries emoji markers ("📎 Beach.jpg", "🎞️ GIF").
 *  Say it in words instead. */
function humanPreview(text: string | undefined): string {
  const t = (text ?? '').trim()
  if (!t) return ''
  if (/^🎞️?\s*GIF$/u.test(t)) return 'GIF'
  const att = /^📎\s*(.*)$/u.exec(t)
  if (att) {
    const rest = att[1].trim()
    const many = /^(\d+) files$/.exec(rest)
    if (many) return `${many[1]} attachments`
    if (!rest || rest === 'File') return 'Attachment'
    const k = typeKind(rest)
    if (k === 'image') return 'Photo'
    if (k === 'video') return 'Video'
    if (k === 'audio') return 'Audio'
    return `Attachment: ${rest}`
  }
  return t
}

/** 1–3 emoji and nothing else → shown large, without a bubble (like Messages). */
function isJumboEmoji(text: string): boolean {
  const t = text.trim()
  if (!t || t.length > 32 || !/^(?:\p{Extended_Pictographic}|\p{Emoji_Component}|\s)+$/u.test(t)) return false
  if (/^[\d#*\s]+$/.test(t)) return false
  try {
    const n = [...new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(t.replace(/\s+/g, ''))].length
    return n >= 1 && n <= 3
  } catch {
    return false
  }
}

/** Best-effort placeholder friend for a conversation whose real friend record is
 *  missing (lost across an update, or not yet self-healed). Keeps a thread VISIBLE
 *  and selectable so a stored conversation can never be silently swallowed
 *  (GitHub #18/#19). `name` prefers the stored overview name, then "Unknown
 *  contact". The Rust receive path recreates a real, replyable record durably —
 *  this is purely the UI's belt-and-suspenders so nothing ever disappears. */
function placeholderFriend(id: string, name?: string): Friend {
  return {
    id,
    role: 'b',
    name: name && name.trim() ? name.trim() : 'Unknown contact',
    secret: '',
    createdAt: 0,
    autoAccept: true,
    endpointId: null,
    avatar: null,
  }
}

export function ChatView() {
  const friends = useStore((s) => s.friends)
  const overview = useStore((s) => s.chatOverview)
  const chats = useStore((s) => s.chats)
  const unread = useStore((s) => s.chatUnread)
  const activeChatId = useStore((s) => s.activeChatId)
  const openChat = useStore((s) => s.openChat)
  const setView = useStore((s) => s.setView)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)

  const myAccount = useStore((s) => s.myDevice?.account_pub)
  const rows = useMemo(() => {
    const byId = new Map(friends.map((f) => [f.id, f]))
    const ordered: { friend: Friend; last?: string; ts?: number; conversation: boolean }[] = []
    // A friend's extra devices speak in the person's thread — never their own row.
    const seen = new Set<string>(Object.keys(personGroups(friends, myAccount)))
    // Only current friends can create visible rows. Detached transcripts and
    // stale file-note caches must not resurrect an Unknown contact.
    for (const o of overview) {
      if (seen.has(o.peerId)) continue
      const f = byId.get(o.peerId)
      if (!f) continue
      ordered.push({ friend: f, last: o.lastText, ts: o.lastTs, conversation: true })
      seen.add(o.peerId)
    }
    // 2) Any thread that has messages but somehow isn't in the overview yet.
    for (const peerId of Object.keys(chats)) {
      if (seen.has(peerId) || !(chats[peerId]?.length)) continue
      const f = byId.get(peerId)
      if (!f) continue
      const last = chats[peerId]!.at(-1)
      ordered.push({ friend: f, ts: last?.ts, conversation: true })
      seen.add(peerId)
    }
    // 3) Friends with no conversation yet, so you can start one.
    for (const f of friends) if (!seen.has(f.id)) ordered.push({ friend: f, conversation: false })
    return ordered
  }, [friends, overview, chats, myAccount])

  useEffect(() => {
    if (MOBILE_UI || activeChatId) return
    const first = rows.find((r) => r.conversation)
    if (first) void openChat(first.friend.id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // A remembered selection is only being viewed while Chat is mounted.
  useEffect(() => {
    void api.setActiveChat(activeChatId)
    if (activeChatId) useStore.getState().markChatRead(activeChatId)
    return () => { void api.setActiveChat(null) }
  }, [activeChatId])

  const online = (f: Friend) => friendOnlineState(f.name, friendSeen, folderStatuses) === true
  const ownLabels = useOwnDeviceLabels()

  if (MOBILE_UI) return activeChatId ? <div className="mobile-page mobile-conversation"><Conversation key={activeChatId} friendId={activeChatId} /></div> : <div className="mobile-page mobile-chats"><MobileHeader title="Chats" /><div className="ios-list">{rows.map(({ friend, last }) => {
    const recent = chats[friend.id]?.at(-1)
    const ts = overview.find(o => o.peerId === friend.id)?.lastTs ?? recent?.ts
    const count = unread[friend.id] ?? 0
    return <button className="ios-row mobile-chat-row" key={friend.id} onClick={() => void openChat(friend.id)}><span className="mobile-chat-avatar"><FriendAvatar friend={friend} /></span><span className="mobile-grow"><span className="ios-headline mobile-ellipsis">{ownLabels[friend.id] ?? friend.name}</span><span className="ios-footnote mobile-ellipsis">{humanPreview(last) || recent?.text || 'No messages yet'}</span></span><span className="mobile-chat-trailing"><time className="ios-footnote">{ts ? clock(ts) : ''}</time>{count > 0 && <span className="mobile-unread">{count > 99 ? '99+' : count}</span>}</span></button>
  })}</div>{!rows.length && <div className="mobile-empty"><MessageCircle /><h2 className="ios-title2">No chats yet</h2><p className="ios-footnote">Add a friend to start a conversation.</p><button className="ios-button ios-primary" onClick={() => setView('friends')}>Add a friend</button></div>}</div>

  // Only show the "nobody to chat with" empty state when there are genuinely no
  // conversations AND no friends — never when a stored thread exists (otherwise a
  // lost friend record would hide a real conversation).
  if (rows.length === 0 && !activeChatId) {
    return (
      <div className="page">
        <div className="page-header titlebar-drag">
          <h1 className="page-title">Chat</h1>
        </div>
        <EmptyState
          title="No conversations yet"
          hint="Add a friend to start chatting."
          action={<button className="btn btn-primary" onClick={() => setView('friends')}>Add a friend</button>}
          style={{ paddingTop: 96 }}
        />
      </div>
    )
  }

  // Conversations first. Friends you haven't talked to yet live behind "New
  // message" — except the one you just started, which shows at the top.
  const conversations = rows.filter((r) => r.conversation || r.friend.id === activeChatId)
  const startable = rows.filter((r) => !r.conversation && r.friend.id !== activeChatId)
  const nameOf = (f: Friend) => ownLabels[f.id] ?? f.name
  const listRows = conversations.length ? conversations : startable

  return (
    <div className={`chat-layout${activeChatId ? ' thread-open' : ''}`}>
      <div className="chat-list-pane">
        <div className="chat-list-head titlebar-drag">
          <h1 className="chat-list-title">Chat</h1>
          {conversations.length > 0 && startable.length > 0 && (
            <MenuButton
              label="New message"
              icon={<SquarePen />}
              items={[
                { heading: 'New message to…' },
                ...startable.map((r) => ({ label: nameOf(r.friend), onSelect: () => void openChat(r.friend.id) })),
              ]}
            />
          )}
        </div>
        <div className="scroll-area chat-list">
          {!conversations.length && <div className="chat-list-note">No conversations yet</div>}
          {listRows.map(({ friend, last, ts, conversation }) => {
            const active = friend.id === activeChatId
            const u = unread[friend.id] ?? 0
            const preview = conversation ? humanPreview(last) || humanPreview(chats[friend.id]?.at(-1)?.text) : ''
            return (
              <button
                key={friend.id}
                className={`chat-row${active ? ' active' : ''}${u > 0 ? ' unread' : ''}${conversation ? '' : ' compact'}`}
                aria-current={active ? 'true' : undefined}
                onClick={() => void openChat(friend.id)}
              >
                <span className="chat-row-dot" aria-hidden />
                <span className="chat-avatar" style={{ background: avatarGradient(friend.id) }}>
                  <FriendAvatar friend={friend} />
                  {online(friend) && <span className="chat-dot" />}
                </span>
                <span className="chat-row-main">
                  <span className="chat-row-top">
                    <span className="chat-row-name" title={nameOf(friend)}>{nameOf(friend)}</span>
                    {conversation && <time className="chat-row-time tnum">{listTime(ts)}</time>}
                  </span>
                  {conversation && <span className="chat-row-last">{preview || (active ? 'New message' : '')}</span>}
                </span>
                {u > 0 && <span className="sr-only">{u} unread</span>}
              </button>
            )
          })}
        </div>
      </div>

      <div className="chat-conversation-pane">
        {activeChatId ? (
          <Conversation key={activeChatId} friendId={activeChatId} />
        ) : (
          <>
            <div className="titlebar-drag chat-header" />
            <EmptyState title="No conversation selected" style={{ flex: 1 }} />
          </>
        )}
      </div>
    </div>
  )
}

/** One row of the thread: a message or a shared-folder event, plus the
 *  separator (if any) that precedes it. */
type Sep = { day: string | null; time: string }
type ThreadRow =
  | { kind: 'msg'; ts: number; m: ChatMessage; sep: Sep | null; firstOfRun: boolean; lastOfRun: boolean; meta: MetaKind }
  | { kind: 'activity'; ts: number; ev: FolderActivityEvent; folderName: string; folder: string; sep: Sep | null }
/** What the quiet line under a message says: the delivery state of your latest
 *  message, or "Sending…" for one still on its way. */
type MetaKind = 'status' | 'pending' | null

function Conversation({ friendId }: { friendId: string }) {
  const realFriend = useStore((s) => s.friends.find((f) => f.id === friendId))
  const messages = useStore((s) => s.chats[friendId] ?? EMPTY_MSGS)
  // Tolerate a missing friend record: render the conversation against a
  // placeholder so an open thread whose friend was lost (GitHub #18/#19) is never
  // a blank pane. The Rust receive path self-heals a real, replyable record; this
  // just keeps the UI alive until that lands (or for read-only history).
  const friend: Friend = realFriend ?? placeholderFriend(friendId)
  const pairs = useStore((s) => s.pairs)
  const folderActivity = useStore((s) => s.folderActivity)
  const sendChat = useStore((s) => s.sendChat)
  const sendGif = useStore((s) => s.sendGif)
  const editChat = useStore((s) => s.editChatMessage)
  const shareFilesInChat = useStore((s) => s.shareFilesInChat)
  const stagedFiles = useStore((s) => s.chatDraftFiles)
  const stageChatFiles = useStore((s) => s.stageChatFiles)
  const unstageChatFile = useStore((s) => s.unstageChatFile)
  const clearChatDraftFiles = useStore((s) => s.clearChatDraftFiles)
  const openSafety = useStore((s) => s.openSafety)
  // PRIMITIVE presence selectors, not the whole friendSeen/folderStatuses records:
  // folder://status events fire continuously during a big folder sync, and a
  // record-level subscription re-rendered this entire (up to 2000-row) thread on
  // every tick. A boolean/string only re-renders when presence actually changes.
  const onlineNow = useStore((s) =>
    friend ? friendOnlineState(friend.name, s.friendSeen, s.folderStatuses) === true : false,
  )
  const presenceText = useStore((s) => {
    if (!friend) return ''
    const p = friendPresence(friend.name, s.friendSeen, s.folderStatuses)
    return p.status === 'online' ? 'Online' : presenceLabel(p)
  })
  const typing = useStore((s) => !!s.chatTyping[friendId])
  const ownLabel = useOwnDeviceLabels()[friendId] as string | undefined
  const giphyKey = useStore((s) => s.settings?.giphyApiKey ?? '')
  const setView = useStore((s) => s.setView)
  const toast = useStore((s) => s.toast)
  const displayName = ownLabel ?? friend.name

  const [picking, setPicking] = useState(false)
  const [text, setText] = useState('')
  const [reply, setReply] = useState<ChatMessage | null>(null)
  const [editing, setEditing] = useState<ChatMessage | null>(null)
  const [showEmoji, setShowEmoji] = useState(false)
  const [showGif, setShowGif] = useState(false)
  const [lightbox, setLightbox] = useState<string | null>(null)
  const [newCount, setNewCount] = useState(0)
  const [conn, setConn] = useState<ConnDetail | null>(null)
  // In-thread search ("where's that link you sent?"): filters the loaded thread
  // client-side; ↑/↓ jump between matches, Esc closes. Reaches the same ~2000-message
  // window the thread itself keeps — no backend.
  const [searchOpen, setSearchOpen] = useState(false)
  const [searchQ, setSearchQ] = useState('')
  const [searchIdx, setSearchIdx] = useState(0)

  const scrollRef = useRef<HTMLDivElement>(null)
  const trackRef = useRef<HTMLDivElement>(null)
  const taRef = useRef<HTMLTextAreaElement>(null)
  const atBottomRef = useRef(true)
  const prevLenRef = useRef(messages.length)
  const typingSentRef = useRef(false)
  const typingOnAtRef = useRef(0)
  const typingTimer = useRef<number | undefined>(undefined)

  // Live connection path to this friend, refreshed whenever they come online.
  // Shown only on request (the ⓘ in the header).
  useEffect(() => {
    if (!onlineNow) {
      setConn(null)
      return
    }
    let cancelled = false
    api
      .probeConnection(friendId)
      .then((d) => {
        if (!cancelled) setConn(d)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [onlineNow, friendId])

  const isAtBottom = () => {
    const el = scrollRef.current
    if (!el) return true
    return el.scrollHeight - el.scrollTop - el.clientHeight < 70
  }
  const scrollToBottom = (smooth = false) => {
    const el = scrollRef.current
    atBottomRef.current = true
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
    setNewCount(0)
  }

  // Smart autoscroll: stick to the bottom only when you're already there (or the
  // new message is yours). Otherwise leave the scroll alone and surface a pill.
  useLayoutEffect(() => {
    const grew = messages.length > prevLenRef.current
    const last = messages[messages.length - 1]
    if (!grew) {
      // status/edit/reaction update of an existing message — don't yank.
      prevLenRef.current = messages.length
      return
    }
    if (atBottomRef.current || last?.fromMe) {
      scrollToBottom()
    } else {
      setNewCount((n) => n + 1)
    }
    prevLenRef.current = messages.length
  }, [messages])

  // On open, jump to bottom.
  useLayoutEffect(() => {
    atBottomRef.current = true
    scrollToBottom()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [friendId])

  // Stay anchored to the newest message while the thread settles: images, video
  // posters and previews load after open and grow the track, and the composer
  // (reply bar, staged files) shrinks the viewport. Follow only while the reader
  // is at the bottom — once they scroll up, leave them where they are.
  const hasRows = messages.length > 0 || myPairsCount(pairs, friend.name, folderActivity) > 0
  useLayoutEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const observer = new ResizeObserver(() => {
      if (atBottomRef.current) el.scrollTop = el.scrollHeight
    })
    observer.observe(el)
    if (trackRef.current) observer.observe(trackRef.current)
    return () => observer.disconnect()
  }, [friendId, hasRows])

  // Stop signalling "typing" when leaving / unmounting.
  useEffect(() => {
    return () => {
      if (typingSentRef.current) void api.sendTyping(friendId, false)
      window.clearTimeout(typingTimer.current)
    }
  }, [friendId])

  // Auto-grow the composer up to a few lines. Empty → let CSS hold a single row
  // (measuring scrollHeight on an empty field can read stale/inflated values).
  useLayoutEffect(() => {
    const ta = taRef.current
    if (!ta) return
    ta.style.height = 'auto'
    if (text) ta.style.height = `${Math.min(ta.scrollHeight, 132)}px`
  }, [text])

  // Every shared folder with THIS friend (matched by peer name), so we can weave
  // its sync activity into the timeline and offer "Open shared folder".
  const myPairs = useMemo(
    () =>
      pairs.filter(
        (p) => !!p.peerName && p.peerName.trim().toLowerCase() === friend.name.trim().toLowerCase(),
      ),
    [pairs, friend.name],
  )
  const sharedFolder = myPairs[0]

  // Combined chat timeline (GitHub #23): messages + shared-folder sync events,
  // ordered by time. Separators: one per day ("Today 2:14 PM"), then a quiet time
  // label after a long pause within the day. Runs of bubbles break at separators.
  const items = useMemo<ThreadRow[]>(() => {
    type Base =
      | { kind: 'msg'; ts: number; m: ChatMessage }
      | { kind: 'activity'; ts: number; ev: FolderActivityEvent; folderName: string; folder: string }
    const rows: Base[] = [
      ...messages.map((m) => ({ kind: 'msg' as const, ts: m.ts, m })),
      ...myPairs.flatMap((p) => {
        const folderName = p.folder.split(/[/\\]/).filter(Boolean).pop() || 'shared folder'
        return (folderActivity[p.id] ?? []).map((ev) => ({
          kind: 'activity' as const,
          ts: ev.ts,
          ev,
          folderName,
          folder: p.folder,
        }))
      }),
    ].sort((a, b) =>
      // Messages keep their Lamport seq order (peer clock skew — e.g. the China/VPN
      // path — can make a later message carry an earlier wall-clock ts, which a pure
      // ts sort would render out of order, notably a reply ahead of its original).
      // Folder-activity rows have no seq, so they interleave by their local ts.
      a.kind === 'msg' && b.kind === 'msg'
        ? byOrder(a.m, b.m)
        : a.ts - b.ts || (a.kind === 'msg' ? -1 : 1),
    )
    // Separators. Seq order can put a slightly older ts after a newer one, so
    // compare against the running max to never print a day twice.
    const seps: (Sep | null)[] = []
    let lastTs: number | null = null
    for (const row of rows) {
      let sep: Sep | null = null
      if (lastTs == null || (!sameDay(row.ts, lastTs) && row.ts > lastTs)) sep = { day: dayLabel(row.ts), time: clock(row.ts) }
      else if (row.ts - lastTs > TIME_GAP_MS) sep = { day: null, time: clock(row.ts) }
      lastTs = lastTs == null ? row.ts : Math.max(lastTs, row.ts)
      seps.push(sep)
    }
    // Delivery status shows once, under your newest message (like Messages).
    let lastMine = -1
    for (let i = rows.length - 1; i >= 0; i--) {
      const r = rows[i]
      if (r.kind === 'msg' && r.m.fromMe && !r.m.deleted) { lastMine = i; break }
    }
    return rows.map((row, i) => {
      const sep = seps[i]
      if (row.kind !== 'msg') return { ...row, sep }
      const prev = rows[i - 1]
      const next = rows[i + 1]
      const pm = prev && prev.kind === 'msg' ? prev.m : undefined
      const nm = next && next.kind === 'msg' ? next.m : undefined
      const firstOfRun = !pm || pm.fromMe !== row.m.fromMe || !!sep
      const lastOfRun = !nm || nm.fromMe !== row.m.fromMe || !!seps[i + 1]
      const pendingStatus = row.m.status === 'sending' || row.m.status === 'failed' || row.m.status == null
      const meta: MetaKind =
        !row.m.fromMe || row.m.deleted ? null
          : i === lastMine ? 'status'
            : lastOfRun && pendingStatus ? 'pending' : null
      return { ...row, sep, firstOfRun, lastOfRun, meta }
    })
  }, [messages, myPairs, folderActivity])
  const online = onlineNow

  // Message ids matching the search, in thread order (oldest → newest). Matches
  // text, file names, and GIF attachments' titles.
  const searchMatches = useMemo(() => {
    const q = searchQ.trim().toLowerCase()
    if (!searchOpen || !q) return []
    return messages
      .filter(
        (m) =>
          !m.deleted &&
          (m.text.toLowerCase().includes(q) ||
            m.files.some((f) => f.toLowerCase().includes(q))),
      )
      .map((m) => m.id)
  }, [searchOpen, searchQ, messages])
  const jumpToMatch = (idx: number) => {
    if (!searchMatches.length) return
    const clamped = Math.max(0, Math.min(idx, searchMatches.length - 1))
    setSearchIdx(clamped)
    document
      .getElementById(`msg-${searchMatches[clamped]}`)
      ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
  }
  // Start at the oldest match; down always moves toward newer messages.
  useEffect(() => {
    if (searchMatches.length) jumpToMatch(0)
    else setSearchIdx(0)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchQ, searchOpen])
  const closeSearch = () => {
    setSearchOpen(false)
    setSearchQ('')
  }

  const onType = (v: string) => {
    setText(v)
    // Throttled typing beacon: heartbeat "on" while typing, refresh an "off" timer.
    if (!editing) {
      const now = Date.now()
      // Re-emit "on" at most every ~3s. The single-shot "on" used to never repeat,
      // so the peer's receiver-side auto-clear (~8s, store.onChatTyping) would hide
      // "typing…" mid-way through a long pause-free message. Heartbeating keeps it
      // alive; the receiver clears promptly once the "off"/heartbeats stop.
      if (v.trim() && (!typingSentRef.current || now - typingOnAtRef.current > 3000)) {
        typingSentRef.current = true
        typingOnAtRef.current = now
        void api.sendTyping(friendId, true)
      }
      window.clearTimeout(typingTimer.current)
      typingTimer.current = window.setTimeout(() => {
        if (typingSentRef.current) {
          typingSentRef.current = false
          void api.sendTyping(friendId, false)
        }
      }, 2500)
    }
  }

  const stopTyping = () => {
    window.clearTimeout(typingTimer.current)
    if (typingSentRef.current) {
      typingSentRef.current = false
      void api.sendTyping(friendId, false)
    }
  }

  const submit = () => {
    const body = text.trim()
    if (editing) {
      // Editing is text-only; the attach/staging UI is disabled in this mode.
      if (!body) return
      void editChat(friendId, editing.id, body)
      setEditing(null)
      setText('')
      stopTyping()
      scrollToBottom()
      return
    }
    // Nothing staged and nothing typed → no-op.
    if (!body && stagedFiles.length === 0) return
    if (stagedFiles.length) {
      void shareFilesInChat(friendId, [...stagedFiles], body)
      clearChatDraftFiles()
      setReply(null)
    } else if (body) {
      void sendChat(friendId, body, reply)
      setReply(null)
    }
    setText('')
    stopTyping()
    scrollToBottom()
  }

  const beginEdit = useCallback((m: ChatMessage) => {
    setEditing(m)
    setReply(null)
    setText(m.text)
    setShowEmoji(false)
    setShowGif(false)
    setTimeout(() => taRef.current?.focus({ preventScroll: MOBILE_UI }), 0)
  }, [])
  // Stable per-thread callbacks the memoized rows invoke with their OWN message —
  // inline `() => {...row.m...}` closures would mint a fresh prop per row per render
  // and defeat React.memo entirely.
  const beginReply = useCallback((m: ChatMessage) => {
    setReply(m)
    setEditing(null)
    taRef.current?.focus({ preventScroll: MOBILE_UI })
  }, [])
  const cancelEdit = () => {
    setEditing(null)
    setText('')
  }

  const attach = async () => {
    // Stage the picked files in the composer (chips) rather than firing them off
    // immediately — they send with the next message (GitHub #23). Same staging a
    // drag-and-drop uses.
    if (picking) return
    setPicking(true)
    try {
      const paths = await api.pickFiles()
      if (paths.length) stageChatFiles(paths)
    } catch (e) {
      useStore.getState().toast('error', String(e))
    } finally {
      setPicking(false)
    }
  }

  // Cmd/Ctrl-V a screenshot straight into the chat: save the clipboard image to an
  // app-managed folder and stage it as a chip — the same proven flow a dropped file
  // uses. Text pastes are untouched (we only intercept when an image is present).
  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = Array.from(e.clipboardData?.items ?? [])
    const img = items.find((i) => i.kind === 'file' && i.type.startsWith('image/'))
    const blob = img?.getAsFile() ?? Array.from(e.clipboardData.files).find((f) => f.type.startsWith('image/'))
    if (!blob) {
      // Let normal text/file pastes proceed. WebKitGTK can expose an image-only
      // clipboard as an entirely empty DataTransfer: ask the native clipboard.
      if (items.some((i) => i.kind === 'file') || e.clipboardData.files.length ||
          e.clipboardData.getData('text/plain') || e.clipboardData.getData('text/html')) return
      e.preventDefault()
      try {
        if (!HAS_TAURI) throw new Error('No usable image is on the clipboard.')
        stageChatFiles([await api.pasteClipboardImage()])
      } catch (err) {
        toast('error', String(err))
      }
      return
    }
    e.preventDefault()
    // Reject oversized pastes BEFORE any encoding work — otherwise a 40 MB clipboard
    // image would freeze the UI for seconds only to be refused on the Rust side.
    if (blob.size > 25 * 1024 * 1024) {
      toast('error', 'That image is too large to paste — save it as a file and drop it in.')
      return
    }
    try {
      // base64 via FileReader (native, off-thread encode) — a Uint8Array invoke arg
      // would serialize as a multi-million-element JSON array and beachball the app.
      const b64 = await new Promise<string>((resolve, reject) => {
        const r = new FileReader()
        r.onload = () => resolve(String(r.result).split(',')[1] ?? '')
        r.onerror = () => reject(r.error ?? new Error('Couldn’t read the pasted image.'))
        r.readAsDataURL(blob)
      })
      const ext = (blob.type.split('/')[1] || 'png').toLowerCase()
      const path = await api.savePastedImage(b64, ext)
      stageChatFiles([path])
    } catch (err) {
      toast('error', String(err))
    }
  }

  const pickGif = (g: { id: string; sendUrl: string; pageUrl: string; w: number; h: number }) => {
    setShowGif(false)
    void sendGif(friendId, {
      provider: 'giphy',
      id: g.id,
      url: g.sendUrl,
      page: g.pageUrl,
      w: g.w,
      h: g.h,
    })
    scrollToBottom()
  }

  const canSend = editing ? !!text.trim() : !!text.trim() || stagedFiles.length > 0

  return (
    <>
      {MOBILE_UI ? <header className="mobile-header-compact visible mobile-conversation-header"><button className="ios-button mobile-back" aria-label="Back to chats" onClick={() => useStore.getState().closeChat()}><ChevronLeft />Chats</button><span className="mobile-chat-avatar compact"><FriendAvatar friend={friend} /></span><div className="mobile-grow"><h1 className="ios-headline mobile-ellipsis">{friend.name}</h1><p className="ios-footnote">{typing ? 'typing…' : presenceText}</p></div><button className="ios-icon" aria-label="Search conversation" onClick={() => searchOpen ? closeSearch() : setSearchOpen(true)}><Search size={20} /></button></header> : (
        <div className="titlebar-drag chat-header">
          <span className="chat-avatar sm" style={{ background: avatarGradient(friend.id) }}>
            <FriendAvatar friend={friend} />
          </span>
          <div className="chat-header-text">
            <div className="chat-header-name truncate-1" title={displayName}>{displayName}</div>
            {/* Typing is a fresh peer signal, independent of the last-seen label. */}
            <div className={`chat-header-sub truncate-1${typing ? ' typing' : online ? ' online' : ''}`}>
              {typing ? 'typing…' : presenceText}
            </div>
          </div>
          <div className="chat-header-actions">
            {online && conn && <ConnInfo detail={conn} align="end" />}
            <IconButton
              label="Search this conversation"
              tooltip="Search"
              active={searchOpen}
              onClick={() => (searchOpen ? closeSearch() : setSearchOpen(true))}
            >
              <Search />
            </IconButton>
            {sharedFolder && (
              <IconButton
                label="Open shared folder"
                tooltip={`Open ${sharedFolder.folder.split(/[/\\]/).filter(Boolean).pop() || 'shared folder'}`}
                onClick={() => void api.openPath(sharedFolder.folder).catch(() => {})}
              >
                <FolderOpen />
              </IconButton>
            )}
            {!ownLabel && (
              <MenuButton
                label={`More options for ${friend.name}`}
                items={[
                  { label: 'Report…', icon: <Flag />, onSelect: () => openSafety({ kind: 'report', friendId: friend.id }) },
                  { label: 'Block…', icon: <Ban />, danger: true, onSelect: () => openSafety({ kind: 'block', friendId: friend.id }) },
                ]}
              />
            )}
          </div>
        </div>
      )}

      {searchOpen && (
        <div className="chat-searchbar">
          <label className="search-field chat-search-field">
            <Search />
            <input
              className="input"
              type="search"
              autoFocus
              value={searchQ}
              placeholder="Search messages and files"
              aria-label="Search messages and files"
              onChange={(e) => setSearchQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape') closeSearch()
                else if (e.key === 'Enter') jumpToMatch(searchIdx + (e.shiftKey ? -1 : 1))
              }}
            />
          </label>
          <span className="chat-search-count tnum" aria-live="polite">
            {searchQ.trim() ? (searchMatches.length ? `${searchIdx + 1} of ${searchMatches.length}` : 'No results') : ''}
          </span>
          <IconButton
            size="sm"
            label="Previous result"
            disabled={!searchMatches.length || searchIdx === 0}
            onClick={() => jumpToMatch(searchIdx - 1)}
          >
            <ChevronUp />
          </IconButton>
          <IconButton
            size="sm"
            label="Next result"
            disabled={!searchMatches.length || searchIdx >= searchMatches.length - 1}
            onClick={() => jumpToMatch(searchIdx + 1)}
          >
            <ChevronDown />
          </IconButton>
          <button className="btn btn-sm btn-plain" onClick={closeSearch}>Done</button>
        </div>
      )}

      <div className="chat-thread-wrap">
        <div ref={scrollRef} className="scroll-area chat-thread" onScroll={() => {
          atBottomRef.current = isAtBottom()
          if (atBottomRef.current && newCount) setNewCount(0)
        }}>
          {items.length === 0 ? (MOBILE_UI ? <div className="mobile-empty"><MessageCircle /><h2 className="ios-title2">Say hi to {friend.name}</h2><p className="ios-footnote">{online ? 'Start your conversation here.' : 'Messages deliver when they return.'}</p><button className="ios-button ios-primary" onClick={() => taRef.current?.focus({ preventScroll: true })}>Write a message</button></div> : (
            <div className="chat-thread-empty">
              <span className="chat-avatar lg" style={{ background: avatarGradient(friend.id) }}>
                <FriendAvatar friend={friend} />
              </span>
              <div className="chat-thread-empty-name truncate-1">{displayName}</div>
              <div className="chat-thread-empty-hint">
                {online ? 'No messages yet' : `Messages deliver when ${friend.name} is back online.`}
              </div>
            </div>
          )) : (
            <div className="chat-track" ref={trackRef}>
              {items.map((row) => {
                const hit = row.kind === 'msg' && searchMatches.includes(row.m.id)
                const current = row.kind === 'msg' && searchMatches[searchIdx] === row.m.id
                return (
                  <div
                    key={row.kind === 'msg' ? row.m.id : row.ev.id}
                    id={row.kind === 'msg' ? `msg-${row.m.id}` : undefined}
                    className={current ? 'chat-search-hit current' : hit ? 'chat-search-hit' : undefined}
                  >
                    {row.sep && (
                      <div className="chat-sep" role="separator">
                        {row.sep.day ? <><b>{row.sep.day}</b> {row.sep.time}</> : row.sep.time}
                      </div>
                    )}
                    {row.kind === 'activity' ? (
                      <FolderSyncRow
                        ev={row.ev}
                        folder={row.folder}
                        folderName={row.folderName}
                        friendName={friend.name}
                      />
                    ) : (
                      <MessageRow
                        m={row.m}
                        friend={friend}
                        firstOfRun={row.firstOfRun}
                        lastOfRun={row.lastOfRun}
                        meta={row.meta}
                        allById={messages}
                        onReply={beginReply}
                        onEdit={beginEdit}
                        onLightbox={setLightbox}
                        reportable={!ownLabel}
                      />
                    )}
                  </div>
                )
              })}
              {typing && !MOBILE_UI && (
                <div className="chat-line run-start run-end">
                  <div className="chat-line-body">
                    <div className="chat-typing" aria-label={`${displayName} is typing`}>
                      <span /><span /><span />
                    </div>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        <AnimatePresence>
          {newCount > 0 && (
            <motion.button
              className="chat-newpill"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 6 }}
              transition={{ duration: 0.16 }}
              onClick={() => scrollToBottom(true)}
            >
              <ArrowDown size={13} /> {newCount} new message{newCount > 1 ? 's' : ''}
            </motion.button>
          )}
        </AnimatePresence>
      </div>

      <div className={MOBILE_UI ? 'chat-composer mobile-composer' : 'chat-composer'}>
        {reply && !editing && (
          <div className="composer-context">
            <CornerUpLeft size={14} aria-hidden />
            <div className="composer-context-text truncate-1">
              <b>Replying to {reply.fromMe ? 'yourself' : displayName}</b>
              <span>{quoteText(reply)}</span>
            </div>
            <IconButton size="sm" label="Cancel reply" onClick={() => setReply(null)}>
              <X />
            </IconButton>
          </div>
        )}
        {editing && (
          <div className="composer-context">
            <Pencil size={14} aria-hidden />
            <div className="composer-context-text truncate-1">
              <b>Editing message</b>
            </div>
            <IconButton size="sm" label="Cancel editing" tooltip="Cancel (Esc)" onClick={cancelEdit}>
              <X />
            </IconButton>
          </div>
        )}

        {stagedFiles.length > 0 && (
          <div className="composer-staged">
            {stagedFiles.map((p) => (
              <StagedChip key={p} path={p} onRemove={() => unstageChatFile(p)} />
            ))}
          </div>
        )}

        <div className={`composer-field${editing ? ' editing' : ''}`}>
          <IconButton label="Attach files" onClick={attach} disabled={!!editing || picking}>
            <Paperclip />
          </IconButton>
          <textarea
            ref={taRef}
            className="composer-input"
            onFocus={() => {
              if (MOBILE_UI) scrollToBottom()
            }}
            value={text}
            placeholder={editing ? 'Edit message' : `Message ${displayName.split(/\s+/)[0] || displayName}`}
            aria-label={editing ? 'Edit message' : `Message ${displayName}`}
            rows={1}
            onChange={(e) => onType(e.target.value)}
            onPaste={(e) => void onPaste(e)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault()
                submit()
              } else if (e.key === 'Escape' && editing) {
                cancelEdit()
              }
            }}
          />
          {/* Settings promises "leave the key blank to hide" the GIF picker — honor
              it (an un-keyed picker only shows an error state anyway). */}
          {!MOBILE_UI && giphyKey.trim() !== '' && (
            <IconButton
              label="Send a GIF"
              active={showGif}
              disabled={!!editing}
              onClick={() => {
                setShowGif((v) => !v)
                setShowEmoji(false)
              }}
            >
              <span className="gif-glyph">GIF</span>
            </IconButton>
          )}
          {!MOBILE_UI && (
            <IconButton
              label="Emoji"
              active={showEmoji}
              onClick={() => {
                setShowEmoji((v) => !v)
                setShowGif(false)
              }}
            >
              <Smile />
            </IconButton>
          )}
          <button
            type="button"
            aria-label={editing ? 'Save message' : 'Send message'}
            title={editing ? 'Save' : 'Send'}
            className="composer-send"
            onClick={submit}
            disabled={!canSend}
          >
            <ArrowUp />
          </button>
        </div>

        {showGif && (
          <GifPicker
            apiKey={giphyKey}
            onPick={pickGif}
            onClose={() => setShowGif(false)}
            onSetup={() => {
              setShowGif(false)
              setView('settings')
            }}
          />
        )}
        {showEmoji && (
          <EmojiPicker
            onPick={(e) => {
              onType(text + e)
              taRef.current?.focus({ preventScroll: MOBILE_UI })
            }}
            onClose={() => setShowEmoji(false)}
          />
        )}
      </div>

      {lightbox && <Lightbox src={lightbox} onClose={() => setLightbox(null)} />}
    </>
  )
}

/** Whether a friend has any shared-folder activity to weave in (cheap check for
 *  the anchoring observer's dependency). */
function myPairsCount(
  pairs: { id: string; peerName?: string | null }[],
  name: string,
  activity: Record<string, FolderActivityEvent[]>,
): number {
  const n = name.trim().toLowerCase()
  let count = 0
  for (const p of pairs) if (p.peerName && p.peerName.trim().toLowerCase() === n) count += activity[p.id]?.length ?? 0
  return count
}

/** Composer emoji grid, styled as a system popover. Closes on outside click / Esc. */
function EmojiPicker({ onPick, onClose }: { onPick: (e: string) => void; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const down = (e: MouseEvent) => {
      const t = e.target as HTMLElement
      if (!ref.current?.contains(t) && !t.closest?.('[aria-label="Emoji"]')) onClose()
    }
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('mousedown', down)
    window.addEventListener('keydown', key)
    return () => {
      window.removeEventListener('mousedown', down)
      window.removeEventListener('keydown', key)
    }
  }, [onClose])
  return (
    <div ref={ref} className="emoji-pop" role="dialog" aria-label="Emoji">
      {EMOJIS.map((e) => (
        <button key={e} type="button" className="emoji-cell" aria-label={e} onClick={() => onPick(e)}>
          {e}
        </button>
      ))}
    </div>
  )
}

/** A file staged in the composer: thumbnail (images) or type icon, name, remove. */
function StagedChip({ path, onRemove }: { path: string; onRemove: () => void }) {
  const name = path.split(/[/\\]/).pop() || path
  const [broken, setBroken] = useState(false)
  const thumb = !broken && fileKind(name) === 'image' && (HAS_TAURI || path.startsWith('/mock-media/')) ? fileSrc(path) : null
  return (
    <span className="staged-chip" title={name}>
      {thumb ? (
        <img className="staged-thumb" src={thumb} alt="" onError={() => setBroken(true)} />
      ) : (
        <span className="staged-icon"><TypeIcon name={name} size={16} /></span>
      )}
      <span className="staged-name">{name}</span>
      <IconButton size="sm" label={`Remove ${name}`} tooltip="Remove" onClick={onRemove}>
        <X />
      </IconButton>
    </span>
  )
}

/** Click-to-enlarge image viewer. Click anywhere or press Esc to close. */
function Lightbox({ src, onClose }: { src: string; onClose: () => void }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [onClose])
  return createPortal(
    <div className="chat-lightbox" role="dialog" aria-label="Image" onClick={onClose}>
      <img src={src} alt="" />
      <IconButton label="Close" tooltip="Close (Esc)" className="chat-lightbox-close" onClick={onClose}>
        <X />
      </IconButton>
    </div>,
    document.body,
  )
}

/** A name inside a folder-activity line that opens/reveals it. */
function SyncLink({ children, onOpen, title }: { children: ReactNode; onOpen: () => void; title: string }) {
  if (MOBILE_UI) return <b>{children}</b>
  return <button type="button" className="sync-link" title={title} onClick={onOpen}>{children}</button>
}

/** A shared-folder sync woven into the timeline (GitHub #23): one quiet line,
 *  e.g. "You added cover-photo.png to Project". The file or folder name reveals it
 *  in Finder; the shared folder's name opens the folder. */
// Memoized: the thread re-renders on every composer keystroke; without memo every
// row (up to 2000, each ~30 elements) re-reconciled per keypress.
const FolderSyncRow = memo(function FolderSyncRow({
  ev,
  folder,
  folderName,
  friendName,
}: {
  ev: FolderActivityEvent
  folder: string
  folderName: string
  friendName: string
}) {
  const mine = ev.direction === 'send'
  const sep = folder.includes('\\') ? '\\' : '/'
  const full = (rel: string) => `${folder}${sep}${rel.split('/').join(sep)}`

  // Group the synced paths: a file directly in the shared root shows on its own,
  // but files inside a subfolder collapse into ONE folder entry — so creating a
  // folder and dropping files in it, or moving a folder in, reads as "added
  // <folder>" instead of a flat list of files (GitHub #23).
  type Entry = { kind: 'dir'; name: string; count: number } | { kind: 'file'; rel: string }
  const entries: Entry[] = (() => {
    const roots: string[] = []
    const dirs = new Map<string, number>()
    for (const raw of ev.files) {
      const rel = raw.split('\\').join('/')
      const slash = rel.indexOf('/')
      if (slash === -1) roots.push(rel)
      else {
        const top = rel.slice(0, slash)
        dirs.set(top, (dirs.get(top) ?? 0) + 1)
      }
    }
    const out: Entry[] = []
    for (const [name, count] of dirs) out.push({ kind: 'dir', name, count })
    for (const rel of roots) out.push({ kind: 'file', rel })
    return out
  })()

  const who = mine ? 'You' : (ev.from && ev.from.trim()) || friendName
  const baseOf = (p: string) => p.split('/').pop() || p
  const dirOf = (p: string) => p.split('/').slice(0, -1).join('/')
  const folderLink = (
    <SyncLink title={`Open ${folderName}`} onOpen={() => void api.openPath(folder).catch(() => {})}>{folderName}</SyncLink>
  )
  const reveal = (rel: string) => () => void api.revealPath(full(rel)).catch(() => {})

  let line: ReactNode
  if (ev.action === 'moved' && ev.moves?.length) {
    if (ev.moves.length === 1) {
      const mv = ev.moves[0]
      const to = mv.to.split('\\').join('/')
      const from = mv.from.split('\\').join('/')
      const item = <SyncLink title="Show in Finder" onOpen={reveal(to)}>{baseOf(to)}</SyncLink>
      line = dirOf(from) === dirOf(to)
        ? <>renamed {baseOf(from)} to {item}</>
        : <>moved {item} to {dirOf(to) ? baseOf(dirOf(to)) : folderName}</>
    } else {
      line = <>moved {ev.moves.length} items in {folderLink}</>
    }
  } else if (entries.length === 1) {
    const e = entries[0]
    line = e.kind === 'dir'
      ? <>added <SyncLink title={`Open ${e.name}`} onOpen={() => void api.openPath(full(e.name)).catch(() => {})}>{e.name}</SyncLink> ({e.count} item{e.count === 1 ? '' : 's'}) to {folderLink}</>
      : <>added <SyncLink title="Show in Finder" onOpen={reveal(e.rel)}>{baseOf(e.rel)}</SyncLink> to {folderLink}</>
  } else {
    line = <>added {ev.files.length} items to {folderLink}</>
  }
  const detail = ev.action !== 'moved' && ev.bytes > 0 ? formatBytes(ev.bytes) : undefined
  const listing = entries.length > 1 && entries.length <= 12
    ? entries.map((e) => (e.kind === 'dir' ? `${e.name}/` : baseOf(e.rel))).join('\n')
    : undefined

  return (
    <div className="sync-row" title={[listing, detail].filter(Boolean).join('\n') || undefined}>
      <FolderOpen size={12} aria-hidden />
      <span className="sync-text">
        {who} {line}
      </span>
    </div>
  )
})

/** Render message text with http(s) URLs as real clickable links (#17). The
 *  matching itself lives in lib/linkify — pure and unit-tested, http/https only
 *  (the only schemes the hardened `open_url` command will open), never a bare
 *  "www." it upgrades for you. Segments are React children, never HTML, so a
 *  message can't inject markup. The anchor NEVER navigates this webview: we
 *  preventDefault and hand the URL to the OS browser (window.open is the
 *  `vite dev` fallback, where there is no Tauri to invoke). */
function Linkified({ text }: { text: string }) {
  const parts = useMemo(() => linkify(text), [text])
  return (
    <>
      {parts.map((p, i) =>
        p.t === 'link' ? (
          <a
            key={i}
            href={p.v}
            className="chat-link"
            rel="noopener noreferrer"
            onClick={(e) => {
              e.preventDefault()
              if (HAS_TAURI) api.openUrl(p.v).catch(() => {})
              else window.open(p.v, '_blank', 'noopener,noreferrer')
            }}
          >
            {p.v}
          </a>
        ) : (
          <span key={i}>{p.v}</span>
        ),
      )}
    </>
  )
}

/** Copy a message to the clipboard (#26). navigator.clipboard is the happy path;
 *  it rejects (or is missing) in a non-secure context or when the webview denies
 *  the permission, so fall back to the old hidden-textarea + execCommand trick,
 *  which still works in WKWebView/WebView2. Rejects when neither route works, so
 *  the caller can say so instead of silently "succeeding". */
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    return
  } catch {
    /* fall through to the legacy path */
  }
  const ta = document.createElement('textarea')
  ta.value = text
  // Off-screen but still focusable — display:none would make the copy a no-op.
  ta.setAttribute('readonly', '')
  ta.style.cssText = 'position:fixed;top:-1000px;left:-1000px;opacity:0'
  document.body.appendChild(ta)
  try {
    ta.select()
    if (!document.execCommand('copy')) throw new Error('copy rejected')
  } finally {
    ta.remove()
  }
}

/** One line of text representing a message, for reply quotes. */
function quoteText(m: ChatMessage): string {
  if (m.deleted) return 'Unsent message'
  if (m.gif) return 'GIF'
  if (m.kind === 'file') {
    if (m.files.length !== 1) return `${m.files.length} attachments`
    const k = fileKind(m.files[0])
    return k === 'image' ? 'Photo' : k === 'video' ? 'Video' : m.files[0]
  }
  return m.text
}

/** Quick-reaction tray: a small system-style popover above the bubble. */
function ReactionTray({
  anchor,
  mine,
  onPick,
  onClose,
  trigger,
}: {
  anchor: DOMRect
  mine: boolean
  onPick: (emoji: string) => void
  onClose: () => void
  trigger?: HTMLElement | null
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    let top = anchor.top - h - 6
    if (top < 8) top = anchor.bottom + 6
    let left = mine ? anchor.right - w : anchor.left
    left = Math.min(Math.max(8, left), window.innerWidth - w - 8)
    setPos({ left, top })
    el.querySelector<HTMLButtonElement>('button')?.focus({ preventScroll: true })
  }, [anchor, mine])
  useEffect(() => {
    const down = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref.current?.contains(t) && !trigger?.contains(t)) onClose()
    }
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { e.stopPropagation(); onClose() }
      if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
        const list = [...(ref.current?.querySelectorAll<HTMLButtonElement>('button') ?? [])]
        const i = list.indexOf(document.activeElement as HTMLButtonElement)
        const next = e.key === 'ArrowRight' ? list[(i + 1) % list.length] : list[(i - 1 + list.length) % list.length]
        next?.focus()
      }
    }
    const away = (e: Event) => { if (!ref.current?.contains(e.target as Node)) onClose() }
    window.addEventListener('mousedown', down, true)
    window.addEventListener('keydown', key, true)
    window.addEventListener('scroll', away, true)
    window.addEventListener('resize', onClose)
    return () => {
      window.removeEventListener('mousedown', down, true)
      window.removeEventListener('keydown', key, true)
      window.removeEventListener('scroll', away, true)
      window.removeEventListener('resize', onClose)
    }
  }, [onClose, trigger])
  return createPortal(
    <div ref={ref} className="react-tray" role="menu" aria-label="React" style={pos ?? { left: -9999, top: -9999 }}>
      {QUICK.map((e) => (
        <button key={e} type="button" role="menuitem" className="react-tray-cell" aria-label={`React with ${e}`} onClick={() => onPick(e)}>
          {e}
        </button>
      ))}
    </div>,
    document.body,
  )
}

const MessageRow = memo(function MessageRow({
  m,
  friend,
  firstOfRun,
  lastOfRun,
  meta,
  allById,
  onReply,
  onEdit,
  onLightbox,
  reportable,
}: {
  m: ChatMessage
  friend: Friend
  firstOfRun: boolean
  lastOfRun: boolean
  meta: MetaKind
  allById: ChatMessage[]
  onReply: (m: ChatMessage) => void
  onEdit: (m: ChatMessage) => void
  onLightbox: (src: string) => void
  /** Their message in a friend's chat (not one of your own devices): Report… */
  reportable?: boolean
}) {
  const mine = m.fromMe
  const react = useStore((s) => s.reactToMessage)
  const del = useStore((s) => s.deleteChatMessage)
  const openSafety = useStore((s) => s.openSafety)
  const toast = useStore((s) => s.toast)
  // A file card whose bytes are still moving (or failed) speaks for itself — the
  // chat note's "Delivered" would contradict it.
  const xferState = useStore((s) => (m.fileXferId ? s.chatTransfers[m.fileXferId]?.state : undefined))
  const [tray, setTray] = useState<{ anchor: DOMRect; trigger: HTMLElement | null } | null>(null)
  const [menu, setMenu] = useState<{ anchor: DOMRect; trigger: HTMLElement | null; context: boolean } | null>(null)
  const mainRef = useRef<HTMLDivElement>(null)
  const hadSelectionRef = useRef(false)
  const closeTray = useCallback(() => setTray(null), [])
  const closeMenu = useCallback(() => setMenu(null), [])

  // Collapse reactions to one chip per emoji; mark the ones we added.
  const reactionChips = useMemo(() => {
    const map = new Map<string, { count: number; mine: boolean }>()
    for (const r of m.reactions ?? []) {
      const e = map.get(r.emoji) ?? { count: 0, mine: false }
      e.count += 1
      if (r.fromMe) e.mine = true
      map.set(r.emoji, e)
    }
    return [...map.entries()]
  }, [m.reactions])

  const replied = m.replyTo ? allById.find((x) => x.id === m.replyTo) : undefined
  // Prefer the LIVE original when it's on-device, so the quote reflects later edits
  // or an unsend instead of the snapshot taken at reply time. Fall back to the stored
  // preview only when we don't have the original (e.g. it predates our join).
  const quote = replied ? quoteText(replied) : m.replyPreview

  const doReact = (emoji: string) => {
    setTray(null)
    void react(friend.id, m.id, emoji)
  }

  // Copying a message out is a per-message action, not an owner action (#26), so
  // the "More" menu opens on THEIR bubbles too — with Edit/Unsend still yours
  // alone. Only real text is copyable; a GIF or file card has no text to take.
  const copyable = !m.deleted && m.kind === 'text' && !m.gif && m.text.trim().length > 0
  const canReport = !mine && !!reportable
  const doCopy = () => {
    void copyText(m.text).then(
      () => toast('success', 'Message copied'),
      () => toast('error', 'Could not copy to clipboard'),
    )
  }
  const menuItems = (context: boolean): MenuItem[] => {
    const top: MenuItem[] = []
    if (context) top.push({ label: 'Reply', icon: <CornerUpLeft />, onSelect: () => onReply(m) })
    if (copyable) top.push({ label: 'Copy', icon: <Copy />, onSelect: doCopy })
    if (mine && m.kind === 'text' && !m.gif) top.push({ label: 'Edit', icon: <Pencil />, onSelect: () => onEdit(m) })
    const bottom: MenuItem[] = []
    if (mine) bottom.push({ label: 'Unsend', icon: <Trash2 />, danger: true, onSelect: () => void del(friend.id, m.id) })
    if (canReport) bottom.push({ label: 'Report…', icon: <Flag />, danger: true, onSelect: () => openSafety({ kind: 'report', friendId: friend.id, messageId: m.id }) })
    return [...top, ...(top.length && bottom.length ? [{ separator: true } as const] : []), ...bottom]
  }
  const hasMenu = mine || copyable || canReport

  let content: ReactNode
  const jumbo = !m.deleted && !m.gif && m.kind === 'text' && !quote && isJumboEmoji(m.text)
  if (m.deleted) {
    content = <div className="chat-deleted">{mine ? 'You unsent a message' : `${friend.name} unsent a message`}</div>
  } else if (m.gif) {
    content = <GifBubble m={m} onLightbox={onLightbox} />
  } else if (m.kind === 'file') {
    content = <FileMessage m={m} mine={mine} onLightbox={onLightbox} />
  } else if (jumbo) {
    content = <div className="chat-jumbo" title={fullTime(m.ts)}>{m.text.trim()}</div>
  } else {
    content = (
      <div className={`chat-bubble${mine ? ' mine' : ''}`} title={fullTime(m.ts)}>
        {quote && (
          <div className="chat-quote">
            <span className="chat-quote-text">{quote}</span>
          </div>
        )}
        <span className="chat-text"><Linkified text={m.text} /></span>
      </div>
    )
  }

  const inFlight = !!xferState && xferState !== 'completed'
  const metaText = (() => {
    const parts: string[] = []
    if (m.edited && !m.deleted) parts.push('Edited')
    if (meta && !(m.kind === 'file' && (inFlight || m.fileXferFailed))) {
      const s = m.status
      if (meta === 'pending' || s === 'sending' || s === 'failed' || s == null) parts.push('Sending…')
      else parts.push(s === 'read' ? 'Read' : 'Delivered')
    }
    return parts.join(' · ')
  })()

  const pinned = !!tray || (!!menu && !menu.context)

  return (
    <div
      className={`chat-line${mine ? ' mine' : ''}${firstOfRun ? ' run-start' : ''}${lastOfRun ? ' run-end' : ''}${
        reactionChips.length ? ' has-reactions' : ''
      }`}
    >
      <div className="chat-line-body">
        <motion.div
          ref={mainRef}
          className="chat-main"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.16 }}
          onMouseDown={(e) => {
            // A right-click selects the word under it before `contextmenu` fires,
            // so decide here whether the reader had already selected text.
            if (e.button !== 2) return
            const sel = window.getSelection()
            hadSelectionRef.current = !!sel && !sel.isCollapsed && !!mainRef.current?.contains(sel.anchorNode)
          }}
          onContextMenu={(e) => {
            if (m.deleted) return
            // Keep the native menu on selected text so Copy/Look Up still work.
            if (hadSelectionRef.current) return
            e.preventDefault()
            window.getSelection()?.removeAllRanges()
            setTray(null)
            setMenu({ anchor: new DOMRect(e.clientX, e.clientY, 0, 0), trigger: null, context: true })
          }}
        >
          {content}

          {reactionChips.length > 0 && (
            <div className="chat-reactions">
              {reactionChips.map(([emoji, info]) => (
                <button
                  key={emoji}
                  type="button"
                  className={`react-chip${info.mine ? ' mine' : ''}`}
                  onClick={() => doReact(emoji)}
                  aria-label={`${emoji} ${info.count}${info.mine ? ', remove your reaction' : ''}`}
                  title={info.mine ? 'Remove reaction' : 'React'}
                >
                  {emoji}
                  {info.count > 1 && <span className="react-chip-n tnum">{info.count}</span>}
                </button>
              ))}
            </div>
          )}

          {!m.deleted && !MOBILE_UI && (
            <div className={`chat-actions${pinned ? ' pinned' : ''}`}>
              <IconButton
                size="sm"
                label="React"
                active={!!tray}
                onClick={(e) => {
                  const r = mainRef.current?.getBoundingClientRect()
                  const trigger = e.currentTarget
                  setMenu(null)
                  setTray((v) => (v || !r ? null : { anchor: r, trigger }))
                }}
              >
                <Smile />
              </IconButton>
              <IconButton size="sm" label="Reply" onClick={() => onReply(m)}>
                <CornerUpLeft />
              </IconButton>
              {hasMenu && (
                <IconButton
                  size="sm"
                  label="More"
                  aria-haspopup="menu"
                  aria-expanded={!!menu && !menu.context}
                  onClick={(e) => {
                    const trigger = e.currentTarget
                    const anchor = trigger.getBoundingClientRect()
                    setTray(null)
                    setMenu((v) => (v ? null : { anchor, trigger, context: false }))
                  }}
                >
                  <MoreHorizontal />
                </IconButton>
              )}
            </div>
          )}
        </motion.div>

        {metaText && <div className="chat-meta">{metaText}</div>}
      </div>
      {tray && <ReactionTray anchor={tray.anchor} trigger={tray.trigger} mine={mine} onPick={doReact} onClose={closeTray} />}
      {menu && (
        <MenuPopover
          anchor={menu.anchor}
          trigger={menu.trigger}
          items={menuItems(menu.context)}
          align={menu.context ? 'start' : mine ? 'end' : 'start'}
          onClose={closeMenu}
        />
      )}
    </div>
  )
})

/** A GIF: renders the LOCAL transferred copy only (animated GIFs autoplay
 *  natively in <img>), capped size, click to enlarge. We deliberately never load
 *  the Giphy CDN url — doing so would leak the receiver's IP to a third party for
 *  a P2P-private chat. Until the bytes arrive, show a sized placeholder. */
function GifBubble({ m, onLightbox }: { m: ChatMessage; onLightbox: (src: string) => void }) {
  const src = m.path && HAS_TAURI ? fileSrc(m.path) : null
  const [broken, setBroken] = useState(false)
  const ratio = m.gif && m.gif.w && m.gif.h ? { aspectRatio: `${m.gif.w} / ${m.gif.h}` } : undefined
  if (!src || broken) {
    return (
      <div className="att-placeholder" style={ratio} title={fullTime(m.ts)}>
        GIF
      </div>
    )
  }
  return (
    <button type="button" className="att-media" onClick={() => onLightbox(src)} title={fullTime(m.ts)}>
      <img className="att-img" src={src} alt="GIF" style={ratio} onError={() => setBroken(true)} />
    </button>
  )
}

/** An image attachment: the picture itself, rounded, no bubble chrome. Reserves
 *  a quiet placeholder until it decodes so the thread doesn't jump twice. */
function ImageAttachment({ src, name, detail, onOpen, onError }: {
  src: string; name: string; detail: string; onOpen: () => void; onError: () => void
}) {
  const [loaded, setLoaded] = useState(false)
  return (
    <button type="button" className={`att-media${loaded ? '' : ' loading'}`} onClick={onOpen} title={detail}>
      <img
        className="att-img"
        src={src}
        alt={name}
        onLoad={() => setLoaded(true)}
        onError={onError}
      />
    </button>
  )
}

/** First frames already captured, keyed by media URL, so a re-mounted or
 *  re-laid-out bubble shows its picture instantly instead of flashing blank. */
const posterCache = new Map<string, { poster: string; ratio: number }>()

/** A video attachment: first frame as a still with a play glyph; plays inline
 *  on click (native controls appear only then). WebKit can drop a paused
 *  <video>'s decoded frame (scrolling, memory pressure, window changes), which
 *  left a blank grey box — so the first frame is captured once to an image. */
function VideoAttachment({ src, name, detail, onError }: { src: string; name: string; detail: string; onError: () => void }) {
  const ref = useRef<HTMLVideoElement>(null)
  const cached = posterCache.get(src)
  const [playing, setPlaying] = useState(false)
  const [poster, setPoster] = useState<string | null>(cached?.poster ?? null)
  const [ratio, setRatio] = useState<number | null>(cached?.ratio ?? null)
  const box = ratio
    ? ratio >= 1 ? { width: 280, height: Math.round(280 / ratio) } : { width: Math.round(280 * ratio), height: 280 }
    : { width: 280, height: 158 }
  const capture = (v: HTMLVideoElement) => {
    if (posterCache.has(src) || !v.videoWidth || !v.videoHeight) return
    try {
      const scale = Math.min(1, 560 / v.videoWidth)
      const c = document.createElement('canvas')
      c.width = Math.round(v.videoWidth * scale)
      c.height = Math.round(v.videoHeight * scale)
      const ctx = c.getContext('2d')
      if (!ctx) return
      ctx.drawImage(v, 0, 0, c.width, c.height)
      // A frame grabbed before it's decoded is solid black; never cache that.
      const px = ctx.getImageData(0, 0, c.width, c.height).data
      let lum = 0, n = 0
      for (let i = 0; i < px.length; i += 4 * 97) { lum += px[i] + px[i + 1] + px[i + 2]; n++ }
      if (n && lum / (3 * n) < 6) return
      const url = c.toDataURL('image/jpeg', 0.82)
      const r = v.videoWidth / v.videoHeight
      posterCache.set(src, { poster: url, ratio: r })
      setPoster(url)
    } catch {
      // Canvas refused (cross-origin) — keep the live first frame instead.
    }
  }
  return (
    <div className={`att-video${playing ? ' playing' : ''}`} style={box} title={playing ? undefined : detail}>
      {poster && !playing && <img className="att-video-poster" src={poster} alt="" draggable={false} />}
      <video
        ref={ref}
        // A media fragment makes the webview paint the first frame instead of a
        // black box before the viewer presses play.
        src={`${src}#t=0.1`}
        crossOrigin="anonymous"
        preload={poster && !playing ? 'none' : 'metadata'}
        playsInline
        controls={playing}
        style={poster && !playing ? { visibility: 'hidden' } : undefined}
        onLoadedMetadata={(e) => {
          const v = e.currentTarget
          if (v.videoWidth && v.videoHeight) setRatio(v.videoWidth / v.videoHeight)
        }}
        onLoadedData={(e) => {
          // Force a real seek to the first frame; capture once it has landed.
          const v = e.currentTarget
          if (!posterCache.has(src) && !v.seeking) v.currentTime = Math.min(0.1, (v.duration || 1) / 2)
        }}
        onSeeked={(e) => capture(e.currentTarget)}
        onError={onError}
      />
      {!playing && (
        <button
          type="button"
          className="att-play"
          aria-label={`Play ${name}`}
          onClick={() => {
            setPlaying(true)
            requestAnimationFrame(() => void ref.current?.play().catch(() => {}))
          }}
        >
          <Play />
        </button>
      )}
    </div>
  )
}

/** A file/media message. Photos and videos render as media (no bubble); other
 *  files as a compact neutral row that opens the file; several files as a short
 *  list. Transfer progress or a failure sits quietly underneath. */
function FileMessage({
  m,
  mine,
  onLightbox,
}: {
  m: ChatMessage
  mine: boolean
  onLightbox: (src: string) => void
}) {
  const liveTransfer = useStore((s) => m.fileXferId ? s.chatTransfers[m.fileXferId] : undefined)
  const history = useStore((s) => s.history)
  const transfer = liveTransfer ?? restoredChatTransfer(m, history)
  const name = m.files[0]
  const completed = completedChatItems(transfer?.chatTransfer?.completedPaths)
  const landedPath = completed.find(item => item.name === name)?.path
  const path = landedPath ?? m.path
  const kind = fileKind(name)
  const [broken, setBroken] = useState(false)
  const [expanded, setExpanded] = useState(false)
  // The chat note can arrive before its separate file transfer finishes.
  const landedTransfer = useStore((s) => Object.values(s.transfers).reverse().find((t) =>
    !m.fromMe && t.direction === 'receive' && t.state === 'completed' &&
    !!t.outDir && t.fileNames.includes(name) &&
    `${t.outDir.replace(/\\/g, '/').replace(/\/$/, '')}/${name}` === path?.replace(/\\/g, '/')
  )?.id)
  useEffect(() => setBroken(false), [path, landedTransfer, transfer?.state])
  const available = mine || !!landedPath || !!m.path && (!transfer || transfer.state === 'completed')
  const canPreview = !!path && (HAS_TAURI || path.startsWith('/mock-media/')) && !broken && available
  const src = canPreview ? `${fileSrc(path!)}?landed=${landedTransfer ?? 'initial'}` : null
  const open = () => path && api.openPath(path).catch(() => {})
  const resendChatFile = useStore((s) => s.resendChatFile)
  // A file whose bytes never landed must not look openable.
  const openable = !!path && available

  const multi = m.files.length > 1

  if (MOBILE_UI) {
    const mobileTransfer = transfer ?? {
      id: m.fileXferId ?? m.id, direction: mine ? 'send' as const : 'receive' as const,
      state: m.fileXferFailed ? 'failed' as const : 'waitingForPeer' as const,
      fileNames: m.files, fileCount: m.files.length, bytesTotal: m.bytes, bytesDone: 0,
      percent: 0, speedBps: 0, etaSeconds: null, locality: 'unknown' as const,
      peer: null, friendName: null, code: null, error: null, outDir: null,
      detail: 'Waiting for confirmation…',
    }
    return <div className="chat-fileblock">
      {m.text && <Linkified text={m.text} />}
      {!multi && src && kind === 'image' && <img src={src} alt={name} className="chat-img" onClick={() => onLightbox(src)} onError={() => setBroken(true)} />}
      {!multi && src && kind === 'video' && <video className="chat-media" src={src} controls preload="metadata" onError={() => setBroken(true)} />}
      {!multi && src && kind === 'audio' && <audio className="chat-audio" src={src} controls preload="metadata" />}
      <TransferCard t={mobileTransfer} showAction={!!transfer || !!m.fileXferFailed && !!m.fileXferId} onRetry={() => void resendChatFile(m.peerId, m.id, mobileTransfer.id)} onShow={openable ? () => { void api.shareFiles(completed.length ? completed.map(item => item.path) : [path!]).catch(e => useStore.getState().toast('error', String(e))) } : undefined} />
      {transfer?.state !== 'completed' && completed.map(item => <button key={item.key} className="ios-button" onClick={() => void api.openPath(item.path).catch(() => {})}>{item.name} · Show</button>)}
    </div>
  }

  const size = m.bytes > 0 ? formatBytes(m.bytes) : ''
  const detail = [name, size].filter(Boolean).join(' · ')
  const retry = mine && m.fileXferId && (transfer ? transfer.state === 'failed' : m.fileXferFailed)
    ? () => void resendChatFile(m.peerId, m.id, transfer?.id ?? m.fileXferId!)
    : undefined
  // A failed card with no live/restored record still needs to say so.
  const shownTransfer: TransferUpdate | undefined = transfer ?? (m.fileXferFailed && m.fileXferId ? {
    id: m.fileXferId, direction: mine ? 'send' : 'receive', state: 'failed',
    fileNames: m.files, fileCount: m.files.length, bytesTotal: m.bytes, bytesDone: 0,
    percent: 0, speedBps: 0, etaSeconds: null, locality: 'unknown',
    peer: null, friendName: null, code: null, error: null, outDir: null,
  } : undefined)

  let body: ReactNode
  if (!multi && src && kind === 'image') {
    body = <ImageAttachment src={src} name={name} detail={detail} onOpen={() => onLightbox(src)} onError={() => setBroken(true)} />
  } else if (!multi && src && kind === 'video') {
    body = <VideoAttachment src={src} name={name} detail={detail} onError={() => setBroken(true)} />
  } else if (multi) {
    const pathFor = (n: string, i: number) =>
      completed.find((c) => c.name === n)?.path ?? (i === 0 && openable ? path ?? undefined : undefined)
    const LIMIT = 3
    const shown = expanded ? m.files : m.files.slice(0, LIMIT)
    const more = m.files.length - shown.length
    body = (
      <div className="att-card att-list">
        <div className="att-list-head tnum">{m.files.length} files{size ? ` · ${size}` : ''}</div>
        {shown.map((n, i) => {
          const p = pathFor(n, i)
          return p ? (
            <button type="button" key={`${i}:${n}`} className="att-list-row clickable" title={`Open ${n}`} onClick={() => void api.openPath(p).catch(() => {})}>
              <TypeIcon name={n} size={15} />
              <span className="truncate-1">{n}</span>
            </button>
          ) : (
            <div key={`${i}:${n}`} className="att-list-row" title={n}>
              <TypeIcon name={n} size={15} />
              <span className="truncate-1">{n}</span>
            </div>
          )
        })}
        {more > 0 && (
          <button type="button" className="att-list-more" onClick={() => setExpanded(true)}>
            and {more} more
          </button>
        )}
      </div>
    )
  } else {
    const row = (
      <>
        <span className="att-file-icon"><TypeIcon name={name} size={24} /></span>
        <span className="att-file-text">
          <span className="att-file-name truncate-1">{name}</span>
          {size && <span className="att-file-size tnum">{size}</span>}
        </span>
      </>
    )
    body = (
      <div className="att-card">
        {!multi && src && kind === 'text' && (m.bytes === 0 || m.bytes < 512 * 1024) && (
          <TextPreview src={src} onOpen={open} />
        )}
        {!multi && src && kind === 'audio' && (
          <audio className="att-audio" src={src} controls preload="metadata" />
        )}
        {openable ? (
          <button type="button" className="att-file clickable" onClick={open} title={`Open ${name}`}>{row}</button>
        ) : (
          <div className="att-file" title={name}>{row}</div>
        )}
      </div>
    )
  }

  return (
    <div className={`att${mine ? ' mine' : ''}`}>
      {body}
      {shownTransfer ? (
        <ChatTransferProgress t={shownTransfer} onRetry={retry} />
      ) : m.fileXferId ? (
        <div className="xfer-line">Waiting…</div>
      ) : null}
      {m.text && (
        <div className={`chat-bubble caption${mine ? ' mine' : ''}`} title={fullTime(m.ts)}>
          <span className="chat-text"><Linkified text={m.text} /></span>
        </div>
      )}
    </div>
  )
}

/** Lazily fetch a small text/markdown file and show the first lines as a preview. */
function TextPreview({ src, onOpen }: { src: string; onOpen: () => void }) {
  const [text, setText] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    let alive = true
    fetch(src)
      .then((r) => r.text())
      .then((t) => alive && setText(t.slice(0, 1400)))
      .catch(() => alive && setFailed(true))
    return () => {
      alive = false
    }
  }, [src])
  if (failed) return null
  return (
    <pre className="att-doc" onClick={onOpen} title="Open">
      {text === null ? 'Loading preview…' : text.length >= 1400 ? `${text}\n…` : text}
    </pre>
  )
}
