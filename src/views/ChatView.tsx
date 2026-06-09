import { useEffect, useMemo, useRef, useState } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { motion } from 'framer-motion'
import {
  Check,
  Clock,
  FolderOpen,
  MessageCircle,
  Paperclip,
  Send as SendIcon,
  Users,
} from 'lucide-react'
import { api, type ChatMessage, type Friend } from '../lib/api'
import { useStore } from '../store'
import { EmptyState } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { formatBytes, formatRelativeTime } from '../lib/format'
import { friendOnlineState, friendPresence, presenceLabel } from '../lib/presence'

/** Stable empty array so the messages selector doesn't return a fresh ref each render. */
const EMPTY_MSGS: ChatMessage[] = []

const IMG_RE = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i
function isImageName(name: string | undefined): boolean {
  return !!name && IMG_RE.test(name)
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

  // Conversations with history first (most recent on top), then the remaining
  // friends so you can start a new chat with anyone.
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

  // On entering Chat with nothing selected, open the most recent conversation.
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
      {/* conversation list */}
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
        <div
          className="titlebar-drag"
          style={{ padding: '10px 16px 8px', fontWeight: 750, fontSize: 17 }}
        >
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
                <span
                  className="chat-avatar"
                  style={{ background: avatarGradient(friend.name) }}
                >
                  {initials(friend.name)}
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

      {/* active conversation */}
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

  // Keep the thread pinned to the newest message.
  useEffect(() => {
    const el = scrollRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [messages.length, friendId])

  if (!friend) return null

  const sharedFolder = pairs.find(
    (p) =>
      !!p.peerName &&
      p.peerName.trim().toLowerCase() === friend.name.trim().toLowerCase(),
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
        <span className="chat-avatar" style={{ background: avatarGradient(friend.name) }}>
          {initials(friend.name)}
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

      <div
        ref={scrollRef}
        className="scroll-area"
        style={{
          flex: 1,
          padding: 16,
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
          minHeight: 0,
        }}
      >
        {messages.length === 0 ? (
          <div style={{ margin: 'auto', textAlign: 'center', color: 'var(--text-faint)' }}>
            <MessageCircle size={30} style={{ opacity: 0.5 }} />
            <div style={{ marginTop: 8, fontSize: 13 }}>Say hi to {friend.name}.</div>
          </div>
        ) : (
          messages.map((m) => <Bubble key={m.id} m={m} />)
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

function Bubble({ m }: { m: ChatMessage }) {
  const mine = m.fromMe
  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      style={{ alignSelf: mine ? 'flex-end' : 'flex-start', maxWidth: '78%' }}
    >
      <div className={`chat-bubble${mine ? ' mine' : ''}`}>
        {m.kind === 'file' ? (
          <div
            className={`chat-file${m.path ? ' clickable' : ''}`}
            onClick={m.path ? () => api.openPath(m.path!).catch(() => {}) : undefined}
            title={m.path ? 'Open' : undefined}
          >
            {m.path && isImageName(m.files[0]) && (
              <img
                src={convertFileSrc(m.path)}
                alt={m.files[0]}
                className="chat-img"
                onError={(e) => {
                  ;(e.currentTarget as HTMLImageElement).style.display = 'none'
                }}
              />
            )}
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <Paperclip size={16} style={{ flexShrink: 0 }} />
              <div style={{ minWidth: 0, flex: 1 }}>
                <div
                  style={{
                    fontWeight: 600,
                    fontSize: 13.5,
                    whiteSpace: 'nowrap',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                  }}
                >
                  {m.files.length === 1 ? m.files[0] : `${m.files.length} files`}
                </div>
                {m.bytes > 0 && (
                  <div style={{ fontSize: 11.5, opacity: 0.7 }}>{formatBytes(m.bytes)}</div>
                )}
              </div>
            </div>
          </div>
        ) : (
          <span style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>{m.text}</span>
        )}
      </div>
      <div
        style={{
          fontSize: 10.5,
          color: m.status === 'failed' ? 'var(--red)' : 'var(--text-faint)',
          marginTop: 3,
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          justifyContent: mine ? 'flex-end' : 'flex-start',
        }}
      >
        <span>{formatRelativeTime(m.ts)}</span>
        {mine && m.status === 'sending' && <Clock size={11} aria-label="Sending" />}
        {mine && m.status === 'sent' && <Check size={11} aria-label="Sent" />}
        {mine && m.status === 'failed' && <span>· not delivered, retrying…</span>}
      </div>
    </motion.div>
  )
}
