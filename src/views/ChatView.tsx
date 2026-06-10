import { useEffect, useMemo, useRef, useState } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { motion } from 'framer-motion'
import {
  Check,
  Clock,
  File as FileIcon,
  FileText,
  FolderOpen,
  MessageCircle,
  Music,
  Paperclip,
  Send as SendIcon,
  Users,
  Video,
} from 'lucide-react'
import { api, HAS_TAURI, type ChatMessage, type Friend } from '../lib/api'
import { useStore } from '../store'
import { EmptyState } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { formatBytes } from '../lib/format'
import { friendOnlineState, friendPresence, presenceLabel } from '../lib/presence'

/** Stable empty array so the messages selector doesn't return a fresh ref each render. */
const EMPTY_MSGS: ChatMessage[] = []

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
  return d.toLocaleDateString([], { month: 'short', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' })
}

export function ChatView() {
  const friends = useStore((s) => s.friends)
  const overview = useStore((s) => s.chatOverview)
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
    for (const o of overview) {
      const f = byId.get(o.peerId)
      if (f) {
        ordered.push({ friend: f, last: o.lastText })
        seen.add(f.id)
      }
    }
    for (const f of friends) if (!seen.has(f.id)) ordered.push({ friend: f })
    return ordered
  }, [friends, overview])

  useEffect(() => {
    if (activeChatId) return
    const firstId = overview[0]?.peerId ?? friends[0]?.id
    if (firstId) void openChat(firstId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const online = (f: Friend) => friendOnlineState(f.name, friendSeen, folderStatuses) === true

  if (friends.length === 0) {
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
  const friend = useStore((s) => s.friends.find((f) => f.id === friendId))
  const messages = useStore((s) => s.chats[friendId] ?? EMPTY_MSGS)
  const pairs = useStore((s) => s.pairs)
  const sendChat = useStore((s) => s.sendChat)
  const shareFilesInChat = useStore((s) => s.shareFilesInChat)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const [text, setText] = useState('')
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = scrollRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [messages.length, friendId])

  // Build per-message render hints: group runs from the same sender, drop an
  // avatar only on the last bubble of an incoming run, and insert a day/time
  // divider when there's a sizeable gap.
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

  if (!friend) return null

  const sharedFolder = pairs.find(
    (p) => !!p.peerName && p.peerName.trim().toLowerCase() === friend.name.trim().toLowerCase(),
  )
  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const online = presence.status === 'online'

  const submit = () => {
    const body = text.trim()
    if (!body) return
    setText('')
    void sendChat(friendId, body)
  }

  const attach = async () => {
    const paths = await api.pickFiles()
    if (paths.length) void shareFilesInChat(friendId, paths)
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
          <div style={{ fontSize: 12, color: online ? 'var(--green)' : 'var(--text-faint)' }}>
            {presenceLabel(presence)}
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

      <div ref={scrollRef} className="scroll-area chat-thread" style={{ flex: 1, minHeight: 0 }}>
        {messages.length === 0 ? (
          <div style={{ margin: 'auto', textAlign: 'center', color: 'var(--text-faint)', paddingTop: 40 }}>
            <MessageCircle size={30} style={{ opacity: 0.5 }} />
            <div style={{ marginTop: 8, fontSize: 13 }}>Say hi to {friend.name}.</div>
          </div>
        ) : (
          <motion.div
            className="chat-track"
            drag="x"
            dragConstraints={{ left: -68, right: 0 }}
            dragElastic={0.05}
            dragMomentum={false}
            whileDrag={{ cursor: 'grabbing' }}
          >
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
                />
              </div>
            ))}
          </motion.div>
        )}
      </div>

      <div
        style={{
          display: 'flex',
          alignItems: 'flex-end',
          gap: 8,
          padding: '10px 14px',
          borderTop: '1px solid var(--border)',
        }}
      >
        <button className="icon-btn" title="Share a file" onClick={attach}>
          <Paperclip size={18} />
        </button>
        <textarea
          className="chat-input"
          value={text}
          placeholder={`Message ${friend.name}…`}
          rows={1}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              submit()
            }
          }}
        />
        <button className="btn btn-primary" onClick={submit} disabled={!text.trim()}>
          <SendIcon size={16} />
        </button>
      </div>
    </>
  )
}

function MessageRow({
  m,
  friend,
  firstOfRun,
  lastOfRun,
}: {
  m: ChatMessage
  friend: Friend
  firstOfRun: boolean
  lastOfRun: boolean
}) {
  const mine = m.fromMe
  return (
    <div className={`chat-line${mine ? ' mine' : ''}${lastOfRun ? ' run-end' : ''}`}>
      {/* incoming sender avatar (only on the last bubble of a run) */}
      {!mine && (
        <span className="chat-line-avatar" style={{ visibility: lastOfRun ? 'visible' : 'hidden', background: avatarGradient(friend.id) }}>
          {avatarContent(friend)}
        </span>
      )}
      <div className="chat-line-body">
        <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} className={`chat-bubble${mine ? ' mine' : ''}${firstOfRun ? ' first' : ''}`}>
          {m.kind === 'file' ? <FileMessage m={m} mine={mine} /> : (
            <span style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>{m.text}</span>
          )}
        </motion.div>
        {/* delivery state for our own messages, shown subtly under the run */}
        {mine && lastOfRun && m.status && m.status !== 'sent' && (
          <span className={`chat-state${m.status === 'failed' ? ' failed' : ''}`}>
            {m.status === 'sending' ? <Clock size={11} /> : null}
            {m.status === 'failed' ? 'not delivered — retrying…' : ''}
          </span>
        )}
        {mine && lastOfRun && m.status === 'sent' && (
          <span className="chat-state">
            <Check size={11} /> Sent
          </span>
        )}
      </div>
      {/* timestamp revealed by swiping the thread left */}
      <span className="chat-time-gutter">{clock(m.ts)}</span>
    </div>
  )
}

/** A file/media message: an inline preview (when we can render one) plus a
 *  clickable header that opens the file in its default app. */
function FileMessage({ m, mine }: { m: ChatMessage; mine: boolean }) {
  const name = m.files[0]
  const kind = fileKind(name)
  const canPreview = !!m.path && HAS_TAURI
  const src = canPreview ? convertFileSrc(m.path!) : null
  const open = () => m.path && api.openPath(m.path).catch(() => {})

  const multi = m.files.length > 1
  const GlyphIcon = kind === 'video' ? Video : kind === 'audio' ? Music : kind === 'text' ? FileText : FileIcon

  return (
    <div className="chat-fileblock">
      {/* preview */}
      {!multi && src && kind === 'image' && (
        <img src={src} alt={name} className="chat-img" onClick={open} onError={(e) => ((e.currentTarget as HTMLImageElement).style.display = 'none')} />
      )}
      {!multi && src && kind === 'video' && (
        <video className="chat-media" src={src} controls preload="metadata" onError={(e) => ((e.currentTarget as HTMLVideoElement).style.display = 'none')} />
      )}
      {!multi && src && kind === 'audio' && (
        <audio className="chat-audio" src={src} controls preload="metadata" />
      )}
      {!multi && src && kind === 'text' && (m.bytes === 0 || m.bytes < 512 * 1024) && (
        <TextPreview src={src} onOpen={open} />
      )}

      {/* clickable header (always present) */}
      <div className={`chat-file${m.path ? ' clickable' : ''}`} onClick={m.path ? open : undefined} title={m.path ? 'Open' : undefined}>
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
