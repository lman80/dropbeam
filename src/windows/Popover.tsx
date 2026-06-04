import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDownToLine, Check, Copy, Maximize2, Send, UserPlus, X } from 'lucide-react'
import { api, isActive, type TransferUpdate } from '../lib/api'
import { useStore } from '../store'
import { BeamLogo, Spinner } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { formatSpeed } from '../lib/format'

const openMain = () => invoke('open_main_window').catch(() => {})
const hideSelf = () => invoke('hide_popover').catch(() => {})

export function Popover() {
  const init = useStore((s) => s.init)
  const ready = useStore((s) => s.ready)
  const friends = useStore((s) => s.friends)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const sendPaths = useStore((s) => s.sendPaths)
  const sendToFriend = useStore((s) => s.sendToFriend)
  const receiveCode = useStore((s) => s.receiveCode)
  const [pickingFor, setPickingFor] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const [showReceive, setShowReceive] = useState(false)

  useEffect(() => {
    init()
  }, [init])

  const active = useMemo(
    () =>
      order
        .map((id) => transfers[id])
        .filter(Boolean)
        .filter((t) => isActive(t.state) || t.state === 'completed')
        .reverse()
        .slice(0, 6),
    [order, transfers],
  )

  const pickAndSend = async () => {
    setPickingFor('__quick__')
    try {
      const paths = await api.pickFiles()
      if (paths.length) sendPaths(paths)
    } finally {
      setPickingFor(null)
    }
  }

  const beamToFriend = async (id: string) => {
    setPickingFor(id)
    try {
      const paths = await api.pickFiles()
      if (paths.length) await sendToFriend(id, paths)
    } finally {
      setPickingFor(null)
    }
  }

  const submitReceive = (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    receiveCode(code)
    setCode('')
    setShowReceive(false)
  }

  return (
    <div className="popover-root">
      <div className="popover-panel">
        <header className="popover-head">
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <BeamLogo size={20} />
            <span style={{ fontWeight: 750, fontSize: 14 }}>DropBeam</span>
          </div>
          <div style={{ display: 'flex', gap: 2 }}>
            <button className="icon-btn" title="Open DropBeam" onClick={openMain}>
              <Maximize2 size={15} />
            </button>
            <button className="icon-btn" title="Close" onClick={hideSelf}>
              <X size={16} />
            </button>
          </div>
        </header>

        <div className="popover-body">
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              className="btn btn-primary"
              style={{ flex: 1, justifyContent: 'center' }}
              onClick={pickAndSend}
              disabled={pickingFor === '__quick__'}
            >
              {pickingFor === '__quick__' ? <Spinner size={15} /> : <Send size={15} />} Send a file
            </button>
            <button
              className="btn btn-ghost"
              title="Receive with a code"
              onClick={() => setShowReceive((v) => !v)}
              style={{ color: showReceive ? 'var(--accent)' : undefined }}
            >
              <ArrowDownToLine size={16} />
            </button>
          </div>

          <AnimatePresence initial={false}>
            {showReceive && (
              <motion.form
                onSubmit={submitReceive}
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                style={{ overflow: 'hidden', display: 'flex', gap: 8, marginTop: 8 }}
              >
                <input
                  className="input"
                  placeholder="Enter a code"
                  value={code}
                  autoFocus
                  spellCheck={false}
                  onChange={(e) => setCode(e.target.value)}
                  style={{ fontFamily: 'var(--font-mono)', fontSize: 13 }}
                />
                <button className="btn btn-primary" type="submit" disabled={!code.trim()}>
                  <ArrowDownToLine size={15} />
                </button>
              </motion.form>
            )}
          </AnimatePresence>

          {/* Friends */}
          <div className="popover-section-label">Beam to a friend</div>
          {friends.length ? (
            <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
              {friends.map((f) => (
                <button
                  key={f.id}
                  className="popover-friend"
                  onClick={() => beamToFriend(f.id)}
                  disabled={pickingFor !== null}
                  title={`Send files to ${f.name}`}
                >
                  <span
                    className="popover-friend-av"
                    style={{ background: avatarGradient(f.id) }}
                  >
                    {pickingFor === f.id ? <Spinner size={12} /> : initials(f.name)}
                  </span>
                  <span style={{ fontSize: 12.5, fontWeight: 600 }}>{f.name}</span>
                </button>
              ))}
            </div>
          ) : (
            <button className="btn btn-ghost" style={{ width: '100%', fontSize: 12.5 }} onClick={openMain}>
              <UserPlus size={14} /> Add a friend in the app
            </button>
          )}

          {/* Active transfers */}
          {active.length > 0 && (
            <>
              <div className="popover-section-label">Transfers</div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 7 }}>
                {active.map((t) => (
                  <PopoverTransfer key={t.id} t={t} />
                ))}
              </div>
            </>
          )}

          {!ready && (
            <div style={{ display: 'grid', placeItems: 'center', padding: 16 }}>
              <Spinner size={18} />
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function PopoverTransfer({ t }: { t: TransferUpdate }) {
  const [copied, setCopied] = useState(false)
  const name = t.fileNames[0] ?? (t.direction === 'receive' ? 'Incoming' : 'Files')
  const isSendWaiting =
    t.direction === 'send' && t.state === 'waitingForPeer' && !!t.code && !t.friendName

  const label =
    t.state === 'completed'
      ? t.direction === 'send'
        ? 'Sent'
        : 'Received'
      : t.state === 'transferring'
        ? `${Math.round(t.percent)}% · ${formatSpeed(t.speedBps)}`
        : t.friendName
          ? `to ${t.friendName}`
          : t.direction === 'receive'
            ? 'Receiving…'
            : 'Waiting…'

  const copy = async () => {
    if (!t.code) return
    try {
      await navigator.clipboard.writeText(t.code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1400)
    } catch {
      /* ignore */
    }
  }

  return (
    <div className="popover-xfer">
      <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
        <span
          style={{
            width: 7,
            height: 7,
            borderRadius: 999,
            flexShrink: 0,
            background:
              t.state === 'completed'
                ? 'var(--green)'
                : t.state === 'failed'
                  ? 'var(--red)'
                  : 'var(--accent)',
          }}
        />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 12.5, fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {name}
          </div>
          <div style={{ fontSize: 11, color: 'var(--text-faint)' }}>{label}</div>
        </div>
        {isSendWaiting && (
          <button className="icon-btn" style={{ width: 26, height: 26 }} onClick={copy} title="Copy code">
            {copied ? <Check size={13} /> : <Copy size={13} />}
          </button>
        )}
      </div>
      {isSendWaiting && (
        <code className="popover-code selectable">{t.code}</code>
      )}
      {t.state === 'transferring' && (
        <div className="popover-progress">
          <div className="popover-progress-fill" style={{ width: `${Math.max(3, t.percent)}%` }} />
        </div>
      )}
    </div>
  )
}
