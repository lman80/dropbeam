import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { AnimatePresence, motion } from 'framer-motion'
import {
  ArrowDown,
  Check,
  CheckCheck,
  Clock,
  CornerUpLeft,
  File as FileIcon,
  FileText,
  FolderOpen,
  MessageCircle,
  MoreHorizontal,
  Music,
  Paperclip,
  Pencil,
  Send as SendIcon,
  Smile,
  Sparkles,
  Trash2,
  Users,
  Video,
  X,
} from 'lucide-react'
import { api, HAS_TAURI, type ChatMessage, type ConnDetail, type Friend } from '../lib/api'
import { useStore } from '../store'
import { EmptyState } from '../components/bits'
import { ConnInspector } from '../components/ConnInspector'
import { GifPicker } from '../components/GifPicker'
import { avatarGradient, initials } from '../lib/avatar'
import { formatBytes } from '../lib/format'
import { friendOnlineState, friendPresence, presenceLabel } from '../lib/presence'

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

/** A friend's avatar inner content: their received picture, or initials. */
function avatarContent(friend: Friend) {
  if (friend.avatar && HAS_TAURI) {
    return (
      <img
        className="avatar-img"
        src={convertFileSrc(friend.avatar)}
        alt=""
        onError={(e) => ((e.currentTarget as HTMLImageElement).style.display = 'none')}
      />
    )
  }
  return initials(friend.name)
}

/** A short wall-clock label, e.g. "3:42 PM". */
function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
}
/** A day/section divider label: Today / Yesterday / a date. */
function dayLabel(ms: number): string {
  const d = new Date(ms)
  const now = new Date()
  const sameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
  const y = new Date(now)
  y.setDate(now.getDate() - 1)
  if (sameDay(d, now)) return 'Today'
  if (sameDay(d, y)) return 'Yesterday'
  return d.toLocaleDateString([], {
    month: 'short',
    day: 'numeric',
    year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric',
  })
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

  const rows = useMemo(() => {
    const byId = new Map(friends.map((f) => [f.id, f]))
    const ordered: { friend: Friend; last?: string }[] = []
    const seen = new Set<string>()
    // 1) Every conversation in the overview — in recency order. If the friend
    //    record is missing, fall back to a placeholder so the thread STILL shows
    //    (the bug was: a lost friend record made the whole conversation invisible).
    for (const o of overview) {
      if (seen.has(o.peerId)) continue
      const f = byId.get(o.peerId) ?? placeholderFriend(o.peerId)
      ordered.push({ friend: f, last: o.lastText })
      seen.add(o.peerId)
    }
    // 2) Any thread that has messages but somehow isn't in the overview yet.
    for (const peerId of Object.keys(chats)) {
      if (seen.has(peerId) || !(chats[peerId]?.length)) continue
      const f = byId.get(peerId) ?? placeholderFriend(peerId)
      ordered.push({ friend: f })
      seen.add(peerId)
    }
    // 3) Friends with no conversation yet, so you can start one.
    for (const f of friends) if (!seen.has(f.id)) ordered.push({ friend: f })
    return ordered
  }, [friends, overview, chats])

  useEffect(() => {
    if (activeChatId) return
    const firstId = overview[0]?.peerId ?? friends[0]?.id
    if (firstId) void openChat(firstId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const online = (f: Friend) => friendOnlineState(f.name, friendSeen, folderStatuses) === true

  // Only show the "nobody to chat with" empty state when there are genuinely no
  // conversations AND no friends — never when a stored thread exists (otherwise a
  // lost friend record would hide a real conversation).
  if (rows.length === 0) {
    return (
      <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: '0 0 16px' }}>Chat</h1>
        <div className="card">
          <EmptyState
            icon={<MessageCircle size={24} />}
            title="No one to chat with yet"
            hint="Add a friend first — then you can message them and share files right inside the conversation."
          />
          <div style={{ display: 'flex', justifyContent: 'center', paddingBottom: 24 }}>
            <button className="btn btn-primary" onClick={() => setView('friends')}>
              <Users size={15} /> Go to Friends
            </button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', height: '100%', minHeight: 0 }}>
      <div
        style={{
          width: 232,
          flexShrink: 0,
          borderRight: '1px solid var(--border)',
          display: 'flex',
          flexDirection: 'column',
          minHeight: 0,
        }}
      >
        <div className="titlebar-drag" style={{ padding: '10px 16px 8px', fontWeight: 750, fontSize: 17 }}>
          Chat
        </div>
        <div className="scroll-area" style={{ flex: 1, padding: '0 8px 8px', minHeight: 0 }}>
          {rows.map(({ friend, last }) => {
            const active = friend.id === activeChatId
            const u = unread[friend.id] ?? 0
            return (
              <button
                key={friend.id}
                className={`chat-row${active ? ' active' : ''}`}
                onClick={() => void openChat(friend.id)}
              >
                <span className="chat-avatar" style={{ background: avatarGradient(friend.id) }}>
                  {avatarContent(friend)}
                  {online(friend) && <span className="chat-dot" />}
                </span>
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span className="chat-row-name">{friend.name}</span>
                  <span className="chat-row-last">{last ?? 'No messages yet'}</span>
                </span>
                {u > 0 && <span className="chat-unread">{u > 99 ? '99+' : u}</span>}
              </button>
            )
          })}
        </div>
      </div>

      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
        {activeChatId ? (
          <Conversation key={activeChatId} friendId={activeChatId} />
        ) : (
          <div style={{ flex: 1, display: 'grid', placeItems: 'center', color: 'var(--text-faint)' }}>
            Select a conversation
          </div>
        )}
      </div>
    </div>
  )
}

function Conversation({ friendId }: { friendId: string }) {
  const realFriend = useStore((s) => s.friends.find((f) => f.id === friendId))
  const messages = useStore((s) => s.chats[friendId] ?? EMPTY_MSGS)
  // Tolerate a missing friend record: render the conversation against a
  // placeholder so an open thread whose friend was lost (GitHub #18/#19) is never
  // a blank pane. The Rust receive path self-heals a real, replyable record; this
  // just keeps the UI alive until that lands (or for read-only history).
  const friend: Friend = realFriend ?? placeholderFriend(friendId)
  const pairs = useStore((s) => s.pairs)
  const sendChat = useStore((s) => s.sendChat)
  const sendGif = useStore((s) => s.sendGif)
  const editChat = useStore((s) => s.editChatMessage)
  const shareFilesInChat = useStore((s) => s.shareFilesInChat)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const typing = useStore((s) => !!s.chatTyping[friendId])
  const giphyKey = useStore((s) => s.settings?.giphyApiKey ?? '')
  const setView = useStore((s) => s.setView)

  const [text, setText] = useState('')
  const [reply, setReply] = useState<ChatMessage | null>(null)
  const [editing, setEditing] = useState<ChatMessage | null>(null)
  const [showEmoji, setShowEmoji] = useState(false)
  const [showGif, setShowGif] = useState(false)
  const [lightbox, setLightbox] = useState<string | null>(null)
  const [newCount, setNewCount] = useState(0)
  const [conn, setConn] = useState<ConnDetail | null>(null)

  const scrollRef = useRef<HTMLDivElement>(null)
  const taRef = useRef<HTMLTextAreaElement>(null)
  const atBottomRef = useRef(true)
  const prevLenRef = useRef(messages.length)
  const typingSentRef = useRef(false)
  const typingTimer = useRef<number | undefined>(undefined)

  // Live connection path to this friend, refreshed whenever they come online.
  const onlineNow = useMemo(
    () => (friend ? friendOnlineState(friend.name, friendSeen, folderStatuses) === true : false),
    [friend, friendSeen, folderStatuses],
  )
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
    scrollToBottom()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [friendId])

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

  const items = useMemo(() => {
    const GAP = 30 * 60 * 1000 // 30 min
    return messages.map((m, i) => {
      const prev = messages[i - 1]
      const next = messages[i + 1]
      const firstOfRun = !prev || prev.fromMe !== m.fromMe || m.ts - prev.ts > GAP
      const lastOfRun = !next || next.fromMe !== m.fromMe || next.ts - m.ts > GAP
      const divider = !prev || m.ts - prev.ts > GAP ? dayLabel(m.ts) : null
      return { m, firstOfRun, lastOfRun, divider }
    })
  }, [messages])

  const sharedFolder = pairs.find(
    (p) => !!p.peerName && p.peerName.trim().toLowerCase() === friend.name.trim().toLowerCase(),
  )
  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const online = presence.status === 'online'

  const onType = (v: string) => {
    setText(v)
    // Throttled typing beacon: send "on" once, refresh an "off" timer.
    if (!editing) {
      if (v.trim() && !typingSentRef.current) {
        typingSentRef.current = true
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
    if (!body) return
    if (editing) {
      void editChat(friendId, editing.id, body)
      setEditing(null)
    } else {
      void sendChat(friendId, body, reply)
      setReply(null)
    }
    setText('')
    stopTyping()
    scrollToBottom()
  }

  const beginEdit = (m: ChatMessage) => {
    setEditing(m)
    setReply(null)
    setText(m.text)
    setShowEmoji(false)
    setShowGif(false)
    setTimeout(() => taRef.current?.focus(), 0)
  }
  const cancelEdit = () => {
    setEditing(null)
    setText('')
  }

  const attach = async () => {
    const paths = await api.pickFiles()
    if (paths.length) void shareFilesInChat(friendId, paths)
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

  return (
    <>
      <div
        className="titlebar-drag"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 11,
          padding: '10px 16px',
          borderBottom: '1px solid var(--border)',
        }}
      >
        <span className="chat-avatar" style={{ background: avatarGradient(friend.id) }}>
          {avatarContent(friend)}
          {online && <span className="chat-dot" />}
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontWeight: 700, fontSize: 14.5 }}>{friend.name}</div>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              fontSize: 12,
              color: typing ? 'var(--accent)' : online ? 'var(--green)' : 'var(--text-faint)',
            }}
          >
            <span>{typing ? 'typing…' : presenceLabel(presence)}</span>
            {online && conn && !typing && (
              <>
                <span style={{ color: 'var(--border-strong)' }}>·</span>
                <ConnInspector detail={conn} compact />
              </>
            )}
          </div>
        </div>
        {sharedFolder && (
          <button
            className="btn btn-ghost"
            onClick={() => api.openPath(sharedFolder.folder)}
            title="Open the folder you share with this friend"
          >
            <FolderOpen size={15} /> Shared folder
          </button>
        )}
      </div>

      <div ref={scrollRef} className="scroll-area chat-thread" style={{ flex: 1, minHeight: 0 }} onScroll={() => {
        atBottomRef.current = isAtBottom()
        if (atBottomRef.current && newCount) setNewCount(0)
      }}>
        {messages.length === 0 ? (
          <div style={{ margin: 'auto', textAlign: 'center', color: 'var(--text-faint)', paddingTop: 40 }}>
            <MessageCircle size={30} style={{ opacity: 0.5 }} />
            <div style={{ marginTop: 8, fontSize: 13 }}>Say hi to {friend.name} 👋</div>
            {!online && (
              <div style={{ marginTop: 4, fontSize: 12, opacity: 0.75 }}>
                They’re offline — your message delivers when they’re back.
              </div>
            )}
          </div>
        ) : (
          <div className="chat-track">
            {items.map(({ m, firstOfRun, lastOfRun, divider }) => (
              <div key={m.id}>
                {divider && (
                  <div className="chat-divider">
                    <span>{divider}</span>
                  </div>
                )}
                <MessageRow
                  m={m}
                  friend={friend}
                  firstOfRun={firstOfRun}
                  lastOfRun={lastOfRun}
                  allById={messages}
                  onReply={() => {
                    setReply(m)
                    setEditing(null)
                    taRef.current?.focus()
                  }}
                  onEdit={() => beginEdit(m)}
                  onLightbox={setLightbox}
                />
              </div>
            ))}
          </div>
        )}
      </div>

      <AnimatePresence>
        {newCount > 0 && (
          <motion.button
            className="chat-newpill"
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            onClick={() => scrollToBottom(true)}
          >
            <ArrowDown size={13} /> {newCount} new message{newCount > 1 ? 's' : ''}
          </motion.button>
        )}
      </AnimatePresence>

      {reply && !editing && (
        <div className="chat-replybar">
          <CornerUpLeft size={14} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="chat-replybar-who">Replying to {reply.fromMe ? 'yourself' : friend.name}</div>
            <div className="chat-replybar-text">{quoteText(reply)}</div>
          </div>
          <button className="icon-btn" onClick={() => setReply(null)} title="Cancel reply">
            <X size={15} />
          </button>
        </div>
      )}
      {editing && (
        <div className="chat-replybar editing">
          <Pencil size={14} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="chat-replybar-who">Editing message</div>
          </div>
          <button className="icon-btn" onClick={cancelEdit} title="Cancel edit">
            <X size={15} />
          </button>
        </div>
      )}

      <div className="chat-composer">
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
          <div className="emoji-pop" onMouseDown={(e) => e.stopPropagation()}>
            {EMOJIS.map((e) => (
              <button
                key={e}
                className="emoji-cell"
                onClick={() => {
                  onType(text + e)
                  taRef.current?.focus()
                }}
              >
                {e}
              </button>
            ))}
          </div>
        )}
        <button className="icon-btn" title="Share a file" onClick={attach} disabled={!!editing}>
          <Paperclip size={18} />
        </button>
        <button
          className={`icon-btn${showGif ? ' on' : ''}`}
          title="Send a GIF"
          onClick={() => {
            setShowGif((v) => !v)
            setShowEmoji(false)
          }}
          disabled={!!editing}
        >
          <Sparkles size={18} />
        </button>
        <button
          className={`icon-btn${showEmoji ? ' on' : ''}`}
          title="Emoji"
          onClick={() => {
            setShowEmoji((v) => !v)
            setShowGif(false)
          }}
        >
          <Smile size={18} />
        </button>
        <textarea
          ref={taRef}
          className="chat-input"
          value={text}
          placeholder={editing ? 'Edit your message…' : `Message ${friend.name}…`}
          rows={1}
          onChange={(e) => onType(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              submit()
            } else if (e.key === 'Escape' && editing) {
              cancelEdit()
            }
          }}
        />
        <button className="btn btn-primary chat-send" onClick={submit} disabled={!text.trim()}>
          {editing ? <Check size={16} /> : <SendIcon size={16} />}
        </button>
      </div>

      {lightbox && (
        <div className="chat-lightbox" onClick={() => setLightbox(null)}>
          <img src={lightbox} alt="" />
        </div>
      )}
    </>
  )
}

/** Matches http(s) URLs in a message. Conservative on purpose: only http/https
 *  (the only schemes the hardened `open_url` command will open), and we trim a
 *  trailing ), ., , ! ? : that's almost always sentence punctuation, not the URL. */
const URL_RE = /\bhttps?:\/\/[^\s<]+/gi
function trimTrailingPunct(url: string): { url: string; trailing: string } {
  const m = url.match(/[).,!?:;'"]+$/)
  if (!m) return { url, trailing: '' }
  // Keep a closing ) when it balances an opening ( inside the URL (e.g. Wikipedia).
  let cut = m[0]
  if (cut.endsWith(')') && (url.match(/\(/g)?.length ?? 0) > (url.match(/\)/g)?.length ?? 0)) {
    cut = cut.slice(0, -1)
  }
  return { url: url.slice(0, url.length - cut.length), trailing: cut }
}

/** Render message text with http(s) URLs as real clickable links. Clicking opens
 *  the URL externally via the hardened `open_url` command (http/https only — it
 *  rejects file://, custom schemes, etc.). Preserves plain text + whitespace. */
function Linkified({ text }: { text: string }) {
  const parts = useMemo(() => {
    const out: Array<{ t: 'text'; v: string } | { t: 'link'; v: string }> = []
    let last = 0
    for (const match of text.matchAll(URL_RE)) {
      const start = match.index ?? 0
      if (start > last) out.push({ t: 'text', v: text.slice(last, start) })
      const { url, trailing } = trimTrailingPunct(match[0])
      out.push({ t: 'link', v: url })
      if (trailing) out.push({ t: 'text', v: trailing })
      last = start + match[0].length
    }
    if (last < text.length) out.push({ t: 'text', v: text.slice(last) })
    return out
  }, [text])
  return (
    <>
      {parts.map((p, i) =>
        p.t === 'link' ? (
          <a
            key={i}
            href={p.v}
            className="chat-link"
            onClick={(e) => {
              e.preventDefault()
              api.openUrl(p.v).catch(() => {})
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

/** One line of text representing a message, for reply quotes. */
function quoteText(m: ChatMessage): string {
  if (m.deleted) return 'Deleted message'
  if (m.gif) return 'GIF'
  if (m.kind === 'file') return m.files.length === 1 ? `📎 ${m.files[0]}` : `📎 ${m.files.length} files`
  return m.text
}

function MessageRow({
  m,
  friend,
  firstOfRun,
  lastOfRun,
  allById,
  onReply,
  onEdit,
  onLightbox,
}: {
  m: ChatMessage
  friend: Friend
  firstOfRun: boolean
  lastOfRun: boolean
  allById: ChatMessage[]
  onReply: () => void
  onEdit: () => void
  onLightbox: (src: string) => void
}) {
  const mine = m.fromMe
  const react = useStore((s) => s.reactToMessage)
  const del = useStore((s) => s.deleteChatMessage)
  const [tray, setTray] = useState(false)
  const [menu, setMenu] = useState(false)

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
  const quote = m.replyPreview ?? (replied ? quoteText(replied) : undefined)

  const doReact = (emoji: string) => {
    setTray(false)
    void react(friend.id, m.id, emoji)
  }

  return (
    <div
      className={`chat-line${mine ? ' mine' : ''}${lastOfRun ? ' run-end' : ''}`}
      onMouseLeave={() => {
        setTray(false)
        setMenu(false)
      }}
    >
      {!mine && (
        <span
          className="chat-line-avatar"
          style={{ visibility: lastOfRun ? 'visible' : 'hidden', background: avatarGradient(friend.id) }}
        >
          {avatarContent(friend)}
        </span>
      )}
      <div className="chat-line-body">
        {quote && (
          <div className={`chat-quote${mine ? ' mine' : ''}`}>
            <span className="chat-quote-bar" />
            <span className="chat-quote-text">{quote}</span>
          </div>
        )}

        <div className="chat-bubble-wrap">
          <motion.div
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            className={`chat-bubble${mine ? ' mine' : ''}${firstOfRun ? ' first' : ''}${m.deleted ? ' deleted' : ''}${
              m.gif ? ' media' : ''
            }`}
          >
            {m.deleted ? (
              <span className="chat-deleted">This message was deleted</span>
            ) : m.gif ? (
              <GifBubble m={m} onLightbox={onLightbox} />
            ) : m.kind === 'file' ? (
              <FileMessage m={m} mine={mine} onLightbox={onLightbox} />
            ) : (
              <span style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                <Linkified text={m.text} />
                {m.edited && <span className="chat-edited"> (edited)</span>}
              </span>
            )}
          </motion.div>

          {!m.deleted && (
            <div className="chat-actions">
              <button className="chat-act" title="React" onClick={() => setTray((v) => !v)}>
                <Smile size={15} />
              </button>
              <button className="chat-act" title="Reply" onClick={onReply}>
                <CornerUpLeft size={15} />
              </button>
              {mine && (
                <button className="chat-act" title="More" onClick={() => setMenu((v) => !v)}>
                  <MoreHorizontal size={15} />
                </button>
              )}
              {tray && (
                <div className="react-tray" onMouseDown={(e) => e.stopPropagation()}>
                  {QUICK.map((e) => (
                    <button key={e} className="react-tray-cell" onClick={() => doReact(e)}>
                      {e}
                    </button>
                  ))}
                </div>
              )}
              {menu && mine && (
                <div className="chat-menu" onMouseDown={(e) => e.stopPropagation()}>
                  {m.kind === 'text' && !m.gif && (
                    <button
                      onClick={() => {
                        setMenu(false)
                        onEdit()
                      }}
                    >
                      <Pencil size={14} /> Edit
                    </button>
                  )}
                  <button
                    className="danger"
                    onClick={() => {
                      setMenu(false)
                      void del(friend.id, m.id)
                    }}
                  >
                    <Trash2 size={14} /> Unsend
                  </button>
                </div>
              )}
            </div>
          )}
        </div>

        {reactionChips.length > 0 && (
          <div className={`chat-reactions${mine ? ' mine' : ''}`}>
            {reactionChips.map(([emoji, info]) => (
              <button
                key={emoji}
                className={`react-chip${info.mine ? ' mine' : ''}`}
                onClick={() => doReact(emoji)}
                title={info.mine ? 'Remove reaction' : 'React'}
              >
                {emoji}
                {info.count > 1 && <span className="react-chip-n">{info.count}</span>}
              </button>
            ))}
          </div>
        )}

        {mine && lastOfRun && <DeliveryState status={m.status} />}
      </div>
      <span className="chat-time-gutter">{clock(m.ts)}</span>
    </div>
  )
}

/** The subtle delivery line under your own last bubble. Offline/queued shows a
 *  calm clock (not a scary "failed"); delivery + read mirror iMessage. */
function DeliveryState({ status }: { status: ChatMessage['status'] }) {
  if (status === 'read')
    return (
      <span className="chat-state read">
        <CheckCheck size={12} /> Read
      </span>
    )
  if (status === 'delivered' || status === 'sent')
    return (
      <span className="chat-state">
        <Check size={12} /> Delivered
      </span>
    )
  // sending / failed / null → still on its way (the outbox keeps retrying).
  return (
    <span className="chat-state" title="Sending — delivers when they’re online">
      <Clock size={11} /> Sending
    </span>
  )
}

/** A GIF bubble: renders the LOCAL transferred copy only (animated GIFs autoplay
 *  natively in <img>), capped size, click to enlarge. We deliberately never load
 *  the Giphy CDN url — doing so would leak the receiver's IP to a third party for
 *  a P2P-private chat. Until the bytes arrive, show a sized placeholder. */
function GifBubble({ m, onLightbox }: { m: ChatMessage; onLightbox: (src: string) => void }) {
  const src = m.path && HAS_TAURI ? convertFileSrc(m.path) : null
  const [broken, setBroken] = useState(false)
  const ratio = m.gif && m.gif.w && m.gif.h ? { aspectRatio: `${m.gif.w} / ${m.gif.h}` } : undefined
  if (!src || broken) {
    return (
      <div className="chat-gif-loading" style={ratio}>
        GIF…
      </div>
    )
  }
  return (
    <img
      className="chat-gif"
      src={src}
      alt="GIF"
      style={ratio}
      onClick={() => onLightbox(src)}
      onError={() => setBroken(true)}
    />
  )
}

/** A file/media message: an inline preview (when we can render one) plus a
 *  clickable header that opens the file in its default app. */
function FileMessage({
  m,
  mine,
  onLightbox,
}: {
  m: ChatMessage
  mine: boolean
  onLightbox: (src: string) => void
}) {
  const name = m.files[0]
  const kind = fileKind(name)
  const [broken, setBroken] = useState(false)
  const canPreview = !!m.path && HAS_TAURI && !broken
  const src = canPreview ? convertFileSrc(m.path!) : null
  const open = () => m.path && api.openPath(m.path).catch(() => {})

  const multi = m.files.length > 1
  const GlyphIcon = kind === 'video' ? Video : kind === 'audio' ? Music : kind === 'text' ? FileText : FileIcon

  return (
    <div className="chat-fileblock">
      {!multi && src && kind === 'image' && (
        <img
          src={src}
          alt={name}
          className="chat-img"
          onClick={() => onLightbox(src)}
          onError={() => setBroken(true)}
        />
      )}
      {!multi && src && kind === 'video' && (
        <video className="chat-media" src={src} controls preload="metadata" onError={() => setBroken(true)} />
      )}
      {!multi && src && kind === 'audio' && (
        <audio className="chat-audio" src={src} controls preload="metadata" />
      )}
      {!multi && src && kind === 'text' && (m.bytes === 0 || m.bytes < 512 * 1024) && (
        <TextPreview src={src} onOpen={open} />
      )}

      <div
        className={`chat-file${m.path ? ' clickable' : ''}`}
        onClick={m.path ? open : undefined}
        title={m.path ? 'Open' : undefined}
      >
        <span className={`chat-file-ic${mine ? ' mine' : ''}`}>
          <GlyphIcon size={16} />
        </span>
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="chat-file-name">{multi ? `${m.files.length} files` : name}</div>
          {m.bytes > 0 && <div className="chat-file-size">{formatBytes(m.bytes)}</div>}
        </div>
      </div>
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
    <pre className="chat-doc" onClick={onOpen} title="Open">
      {text === null ? 'Loading preview…' : text.length >= 1400 ? `${text}\n…` : text}
    </pre>
  )
}
