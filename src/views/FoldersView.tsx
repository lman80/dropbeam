import { useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  ArrowLeftRight,
  ArrowRight,
  Check,
  Copy,
  FolderOpen,
  FolderSync,
  Plus,
  QrCode,
  Settings2,
  Unlink,
  X,
} from 'lucide-react'
import { api, type FolderStatus, type Pair } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, ProgressBar, Spinner } from '../components/bits'
import { formatBytes, formatEta, formatSpeed } from '../lib/format'
import { PairingModal } from '../components/PairingModal'

export function FoldersView() {
  const pairs = useStore((s) => s.pairs)
  const statuses = useStore((s) => s.folderStatuses)
  const [modal, setModal] = useState<'create' | 'accept' | null>(null)
  const [invite, setInvite] = useState<{ code: string; name: string } | null>(null)

  return (
    <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 18,
          gap: 12,
        }}
      >
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>Shared Folders</h1>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => setModal('accept')}>
            <Plus size={15} /> Accept invite
          </button>
          <button className="btn btn-primary" onClick={() => setModal('create')}>
            <FolderSync size={15} /> New folder
          </button>
        </div>
      </div>

      {pairs.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<FolderSync size={24} />}
            title="No shared folders yet"
            hint="Pair a folder with a friend so anything you drop in is automatically beamed to them — even across the internet. Optionally, files vanish from your side once delivered."
          />
          <div style={{ display: 'flex', gap: 10, justifyContent: 'center', paddingBottom: 24 }}>
            <button className="btn btn-ghost" onClick={() => setModal('accept')}>
              Accept an invite
            </button>
            <button className="btn btn-primary" onClick={() => setModal('create')}>
              <FolderSync size={15} /> Create one
            </button>
          </div>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <AnimatePresence initial={false}>
            {pairs.map((pair) => (
              <FolderCard
                key={pair.id}
                pair={pair}
                status={statuses[pair.id]}
                onShowInvite={(code) => setInvite({ code, name: pair.folder.split('/').pop() || '' })}
              />
            ))}
          </AnimatePresence>
        </div>
      )}

      {modal && <PairingModal mode={modal} onClose={() => setModal(null)} />}
      {invite && <InviteModal code={invite.code} folderName={invite.name} onClose={() => setInvite(null)} />}
    </div>
  )
}

function statusInfo(pair: Pair, status?: FolderStatus): { color: string; label: string } {
  if (pair.role === 'a' && !pair.peerName) {
    return { color: 'var(--amber)', label: 'Waiting for someone to accept the invite' }
  }
  const st = status?.state ?? 'idle'
  switch (st) {
    case 'sending':
      return {
        color: 'var(--accent)',
        label: status?.sendingFile ? `Sending ${status.sendingFile}` : 'Sending…',
      }
    case 'receiving':
      return { color: 'var(--accent)', label: 'Receiving…' }
    case 'waiting':
      return {
        color: 'var(--amber)',
        label:
          (status?.detail ?? 'Waiting for the other device') +
          (status && status.queued > 0 ? ` · ${status.queued} queued` : ''),
      }
    case 'error':
      return { color: 'var(--red)', label: status?.detail ?? 'Something went wrong' }
    default:
      return { color: 'var(--green)', label: 'In sync' }
  }
}

function FolderCard({
  pair,
  status,
  onShowInvite,
}: {
  pair: Pair
  status?: FolderStatus
  onShowInvite: (code: string) => void
}) {
  const updatePair = useStore((s) => s.updatePair)
  const removePair = useStore((s) => s.removePair)
  const toast = useStore((s) => s.toast)
  const [open, setOpen] = useState(false)
  const [confirmUnpair, setConfirmUnpair] = useState(false)
  const [loadingInvite, setLoadingInvite] = useState(false)

  const info = statusInfo(pair, status)
  const folderName = pair.folder.split('/').pop() || pair.folder
  const peer = pair.peerName || 'Pending peer'

  const showInvite = async () => {
    setLoadingInvite(true)
    try {
      const code = await api.pairInvite(pair.id)
      onShowInvite(code)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setLoadingInvite(false)
    }
  }

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.97 }}
      className="card"
      style={{ padding: 16, overflow: 'hidden' }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: 12,
            display: 'grid',
            placeItems: 'center',
            flexShrink: 0,
            color: 'white',
            background: 'linear-gradient(135deg, var(--accent), var(--accent-2))',
          }}
        >
          <FolderSync size={19} />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontWeight: 700, fontSize: 14.5 }}>{peer}</span>
            <span
              className="chip"
              style={{ background: 'var(--surface-2)', color: 'var(--text-muted)' }}
            >
              {pair.twoWay ? <ArrowLeftRight size={11} /> : <ArrowRight size={11} />}
              {pair.twoWay ? 'Two-way' : 'One-way'}
            </span>
            {pair.autoDelete && (
              <span className="chip" style={{ background: 'var(--amber-soft)', color: 'var(--amber)' }}>
                Auto-delete
              </span>
            )}
          </div>
          <div
            style={{
              fontSize: 12,
              color: 'var(--text-faint)',
              marginTop: 2,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {folderName}
          </div>
        </div>
        <button className="icon-btn" title="Open folder" onClick={() => api.revealPath(pair.folder)}>
          <FolderOpen size={16} />
        </button>
        <button
          className="icon-btn"
          title="Folder settings"
          onClick={() => setOpen((o) => !o)}
          style={{ color: open ? 'var(--accent)' : undefined }}
        >
          <Settings2 size={16} />
        </button>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 12 }}>
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: 999,
            background: info.color,
            flexShrink: 0,
            boxShadow: `0 0 0 3px color-mix(in srgb, ${info.color} 22%, transparent)`,
          }}
        />
        <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>{info.label}</span>
        {pair.role === 'a' && !pair.peerName && (
          <button
            className="btn btn-ghost"
            style={{ marginLeft: 'auto', padding: '5px 10px', fontSize: 12.5 }}
            onClick={showInvite}
            disabled={loadingInvite}
          >
            {loadingInvite ? <Spinner size={13} /> : <QrCode size={13} />} Show invite
          </button>
        )}
      </div>

      {/* Live transfer progress while sending or receiving */}
      {(status?.state === 'sending' || status?.state === 'receiving') && (
        <motion.div
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: 'auto' }}
          style={{ marginTop: 12, overflow: 'hidden' }}
        >
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'baseline',
              marginBottom: 6,
            }}
          >
            <span style={{ fontSize: 15, fontWeight: 750 }} className="gradient-text">
              {Math.round(status.percent)}%
            </span>
            <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              {formatBytes(status.bytesDone)}
              {status.bytesTotal > 0 ? ` / ${formatBytes(status.bytesTotal)}` : ''}
            </span>
          </div>
          <ProgressBar percent={status.percent} />
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              fontSize: 11.5,
              color: 'var(--text-faint)',
              marginTop: 6,
            }}
          >
            <span>{formatSpeed(status.speedBps)}</span>
            <span>{formatEta(status.etaSeconds)} left</span>
          </div>
        </motion.div>
      )}

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2 }}
            style={{ overflow: 'hidden' }}
          >
            <div style={{ borderTop: '1px solid var(--border)', marginTop: 14, paddingTop: 6 }}>
              <SettingRow
                title="Two-way sync"
                desc="Receive the peer's files too, not just send."
              >
                <button
                  className={`toggle${pair.twoWay ? ' on' : ''}`}
                  onClick={() => updatePair({ id: pair.id, twoWay: !pair.twoWay })}
                />
              </SettingRow>
              <SettingRow
                title="Delete after delivery"
                desc="Remove the local copy once the peer confirms receipt — a self-emptying outbox."
              >
                <button
                  className={`toggle${pair.autoDelete ? ' on' : ''}`}
                  onClick={() => updatePair({ id: pair.id, autoDelete: !pair.autoDelete })}
                />
              </SettingRow>
              {pair.autoDelete && (
                <SettingRow title="When deleting" desc="Trash is recoverable; permanent is not.">
                  <div className="seg">
                    {(['trash', 'permanent'] as const).map((m) => (
                      <button
                        key={m}
                        className={pair.deleteMode === m ? 'active' : ''}
                        style={{ textTransform: 'capitalize', padding: '5px 12px' }}
                        onClick={() => updatePair({ id: pair.id, deleteMode: m })}
                      >
                        {m === 'trash' ? 'Trash' : 'Permanent'}
                      </button>
                    ))}
                  </div>
                </SettingRow>
              )}
              <div
                style={{
                  display: 'flex',
                  gap: 8,
                  justifyContent: 'flex-end',
                  marginTop: 12,
                  paddingBottom: 2,
                }}
              >
                {pair.role === 'a' && (
                  <button className="btn btn-ghost" onClick={showInvite} disabled={loadingInvite}>
                    <QrCode size={14} /> Show invite
                  </button>
                )}
                {confirmUnpair ? (
                  <>
                    <button className="btn btn-ghost" onClick={() => setConfirmUnpair(false)}>
                      Cancel
                    </button>
                    <button className="btn btn-danger" onClick={() => removePair(pair.id)}>
                      <Unlink size={14} /> Confirm unpair
                    </button>
                  </>
                ) : (
                  <button className="btn btn-danger" onClick={() => setConfirmUnpair(true)}>
                    <Unlink size={14} /> Unpair
                  </button>
                )}
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  )
}

function SettingRow({
  title,
  desc,
  children,
}: {
  title: string
  desc: string
  children: React.ReactNode
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 14, padding: '11px 2px' }}>
      <div style={{ flex: 1 }}>
        <div style={{ fontSize: 13.5, fontWeight: 600 }}>{title}</div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2, lineHeight: 1.45 }}>
          {desc}
        </div>
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  )
}

function InviteModal({
  code,
  folderName,
  onClose,
}: {
  code: string
  folderName: string
  onClose: () => void
}) {
  const toast = useStore((s) => s.toast)
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy')
    }
  }
  return (
    <div
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(8, 9, 14, 0.5)',
        backdropFilter: 'blur(4px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 200,
        padding: 20,
      }}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        onClick={(e) => e.stopPropagation()}
        className="card"
        style={{ width: 420, maxWidth: '100%', padding: 22, borderRadius: 20 }}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 14 }}>
          <div style={{ fontWeight: 750, fontSize: 16 }}>Invite for {folderName}</div>
          <button className="icon-btn" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <p style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 0, lineHeight: 1.5 }}>
          Send this to the other person. They open DropBeam → <b>Accept invite</b>, paste it, and
          choose a folder.
        </p>
        <div style={{ display: 'grid', placeItems: 'center', margin: '6px 0 14px' }}>
          <div style={{ background: '#fff', padding: 12, borderRadius: 14, border: '1px solid var(--border)' }}>
            <QRCodeSVG value={code} size={150} level="M" fgColor="#15161d" bgColor="#fff" />
          </div>
        </div>
        <div
          className="selectable"
          style={{
            fontFamily: 'var(--font-mono)',
            fontSize: 11,
            background: 'var(--surface-2)',
            border: '1px solid var(--border)',
            borderRadius: 11,
            padding: '10px 12px',
            wordBreak: 'break-all',
            maxHeight: 80,
            overflowY: 'auto',
            color: 'var(--text-muted)',
          }}
        >
          {code}
        </div>
        <button className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} style={{ width: '100%', marginTop: 12 }} onClick={copy}>
          {copied ? <Check size={15} /> : <Copy size={15} />}
          {copied ? 'Copied' : 'Copy invite'}
        </button>
      </motion.div>
    </div>
  )
}
