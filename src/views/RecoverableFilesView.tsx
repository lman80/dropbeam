import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import {
  ChevronRight,
  HardDrive,
  RotateCcw,
  Settings as SettingsIcon,
  Trash2,
} from 'lucide-react'
import { api, onFolderHistoryChanged, type FolderHistorySummary, type HistoryItem } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, Spinner } from '../components/bits'
import { FileIcon } from '../components/FileIcon'
import { formatBytes, formatRelativeTime } from '../lib/format'

// A soft palette so each folder's share of the storage bar is distinguishable.
const BAR_COLORS = ['#5b9bf0', '#34c2a8', '#e0719a', '#d99b3f', '#b08cf0', '#3fae6e']

/** The "Recoverable files" tab: every shared folder's saved copies of deleted /
 *  replaced files, with a global storage gauge and one-tap ways to free space. */
export function RecoverableFilesView() {
  const toast = useStore((s) => s.toast)
  const setView = useStore((s) => s.setView)
  const focusPair = useStore((s) => s.historyFocusPair)
  const clearFocus = useStore((s) => s.clearHistoryFocus)
  const [summaries, setSummaries] = useState<FolderHistorySummary[] | null>(null)
  const [open, setOpen] = useState<string | null>(null)
  const [confirming, setConfirming] = useState<'all' | string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    try {
      setSummaries(await api.folderHistorySummary())
    } catch {
      setSummaries([])
    }
  }, [])

  useEffect(() => {
    void load()
    const un = onFolderHistoryChanged(() => void load())
    return () => {
      un.then((f) => f())
    }
  }, [load])

  // Deep-link from a folder: open that folder's drawer once summaries arrive.
  useEffect(() => {
    if (focusPair && summaries) {
      if (summaries.some((s) => s.pairId === focusPair)) setOpen(focusPair)
      clearFocus()
    }
  }, [focusPair, summaries, clearFocus])

  const total = (summaries ?? []).reduce((s, f) => s + f.bytes, 0)

  const freeAll = async () => {
    setBusy(true)
    try {
      const freed = await api.clearAllFolderHistory()
      toast('success', `Freed ${formatBytes(freed)}`)
      setConfirming(null)
      await load()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const emptyFolder = async (pairId: string) => {
    setBusy(true)
    try {
      const freed = await api.clearFolderHistory(pairId)
      toast('success', `Freed ${formatBytes(freed)}`)
      setConfirming(null)
      await load()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  if (summaries === null) {
    return (
      <div style={{ display: 'grid', placeItems: 'center', padding: 50 }}>
        <Spinner size={22} />
      </div>
    )
  }

  if (summaries.length === 0) {
    return (
      <div className="card">
        <EmptyState
          icon={<HardDrive size={24} />}
          title="Nothing to recover"
          hint="When a file is deleted or replaced in a shared folder, a copy is kept here so you can get it back — and it's cleaned up automatically."
        />
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      {/* Storage gauge */}
      <div className="card" style={{ padding: 18 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 10 }}>
          <div>
            <div style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>Saved copies are using</div>
            <div style={{ fontSize: 26, fontWeight: 780, letterSpacing: '-0.02em', lineHeight: 1.15 }}>
              {formatBytes(total)}
            </div>
          </div>
          {confirming === 'all' ? (
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>Free {formatBytes(total)}?</span>
              <button className="btn btn-ghost" onClick={() => setConfirming(null)} disabled={busy}>
                Cancel
              </button>
              <button className="btn btn-danger" onClick={freeAll} disabled={busy}>
                {busy ? <Spinner size={13} /> : <Trash2 size={14} />} Free up space
              </button>
            </div>
          ) : (
            <button className="btn btn-primary" onClick={() => setConfirming('all')}>
              <Trash2 size={14} /> Free up space
            </button>
          )}
        </div>
        {/* stacked per-folder share bar */}
        <div
          style={{
            display: 'flex',
            height: 9,
            borderRadius: 999,
            overflow: 'hidden',
            background: 'var(--surface-2)',
            marginTop: 14,
          }}
        >
          {total > 0 &&
            summaries.map((f, i) => (
              <div
                key={f.pairId}
                title={`${f.folderName} — ${formatBytes(f.bytes)}`}
                style={{ width: `${(f.bytes / total) * 100}%`, background: BAR_COLORS[i % BAR_COLORS.length] }}
              />
            ))}
        </div>
        <div style={{ fontSize: 11.5, color: 'var(--text-faint)', marginTop: 11, lineHeight: 1.5 }}>
          Old copies are removed automatically to keep this small. Your live files are never touched.{' '}
          <button
            onClick={() => setView('settings')}
            style={{
              background: 'none',
              border: 'none',
              padding: 0,
              cursor: 'pointer',
              color: 'var(--accent)',
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              gap: 3,
            }}
          >
            <SettingsIcon size={11} /> Change how long
          </button>
        </div>
      </div>

      {/* Per-folder list */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        {summaries.map((f) => (
          <FolderRow
            key={f.pairId}
            summary={f}
            isOpen={open === f.pairId}
            onToggle={() => setOpen(open === f.pairId ? null : f.pairId)}
            confirming={confirming === f.pairId}
            onAskEmpty={() => setConfirming(confirming === f.pairId ? null : f.pairId)}
            onEmpty={() => emptyFolder(f.pairId)}
            busy={busy}
            onChanged={load}
          />
        ))}
      </div>
    </div>
  )
}

function FolderRow({
  summary,
  isOpen,
  onToggle,
  confirming,
  onAskEmpty,
  onEmpty,
  busy,
  onChanged,
}: {
  summary: FolderHistorySummary
  isOpen: boolean
  onToggle: () => void
  confirming: boolean
  onAskEmpty: () => void
  onEmpty: () => void
  busy: boolean
  onChanged: () => Promise<void>
}) {
  const toast = useStore((s) => s.toast)
  const [items, setItems] = useState<HistoryItem[] | null>(null)
  const [itemBusy, setItemBusy] = useState<string | null>(null)
  const loadedFor = useRef<string | null>(null)

  useEffect(() => {
    if (!isOpen) return
    // Reload whenever opened or the underlying summary count changes.
    const key = `${summary.pairId}:${summary.itemCount}`
    if (loadedFor.current === key) return
    loadedFor.current = key
    void api
      .listFolderHistory(summary.pairId)
      .then(setItems)
      .catch(() => setItems([]))
  }, [isOpen, summary.pairId, summary.itemCount])

  const restore = async (item: HistoryItem) => {
    setItemBusy(item.id)
    try {
      await api.restoreFolderItem(summary.pairId, item.id)
      toast('success', `Restored ${item.relPath.split('/').pop()}`)
      await onChanged()
      setItems(await api.listFolderHistory(summary.pairId))
    } catch (e) {
      toast('error', String(e))
    } finally {
      setItemBusy(null)
    }
  }

  const forget = async (item: HistoryItem) => {
    setItemBusy(item.id)
    try {
      await api.forgetFolderItem(summary.pairId, item.id)
      await onChanged()
      setItems(await api.listFolderHistory(summary.pairId))
    } finally {
      setItemBusy(null)
    }
  }

  return (
    <div className="card" style={{ padding: 0, overflow: 'hidden' }}>
      <div
        className="row-hover"
        onClick={onToggle}
        style={{ display: 'flex', alignItems: 'center', gap: 11, padding: '13px 15px', cursor: 'pointer' }}
      >
        <motion.div animate={{ rotate: isOpen ? 90 : 0 }} style={{ display: 'grid', placeItems: 'center', color: 'var(--text-faint)' }}>
          <ChevronRight size={17} />
        </motion.div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 14, fontWeight: 650, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {summary.folderName}
          </div>
          <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
            {formatBytes(summary.bytes)} · {summary.itemCount} {summary.itemCount === 1 ? 'item' : 'items'}
          </div>
        </div>
        {confirming ? (
          <div style={{ display: 'flex', gap: 8 }} onClick={(e) => e.stopPropagation()}>
            <button className="btn btn-ghost" onClick={onAskEmpty} disabled={busy}>
              Cancel
            </button>
            <button className="btn btn-danger" onClick={onEmpty} disabled={busy}>
              {busy ? <Spinner size={13} /> : <Trash2 size={14} />} Empty
            </button>
          </div>
        ) : (
          <button
            className="icon-btn"
            title="Empty this folder's saved copies"
            onClick={(e) => {
              e.stopPropagation()
              onAskEmpty()
            }}
          >
            <Trash2 size={15} />
          </button>
        )}
      </div>

      <AnimatePresence initial={false}>
        {isOpen && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            style={{ overflow: 'hidden' }}
          >
            <div style={{ borderTop: '1px solid var(--border)', padding: 6 }}>
              {items === null ? (
                <div style={{ display: 'grid', placeItems: 'center', padding: 22 }}>
                  <Spinner size={18} />
                </div>
              ) : items.length === 0 ? (
                <div style={{ padding: '14px 12px', fontSize: 12.5, color: 'var(--text-faint)' }}>
                  Nothing saved here.
                </div>
              ) : (
                items.map((item) => {
                  const name = item.relPath.split('/').pop() ?? item.relPath
                  const dir = item.relPath.includes('/') ? item.relPath.slice(0, item.relPath.lastIndexOf('/')) : ''
                  return (
                    <div
                      key={item.id}
                      className="row-hover"
                      style={{ display: 'flex', alignItems: 'center', gap: 11, padding: '9px 10px', borderRadius: 9 }}
                    >
                      <FileIcon name={name} />
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 13.5, fontWeight: 550, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                          {name}
                        </div>
                        <div style={{ fontSize: 11.5, color: 'var(--text-faint)', marginTop: 1 }}>
                          {item.reason === 'replaced' ? 'Replaced' : 'Deleted'} · {formatRelativeTime(item.timestampMs)} ·{' '}
                          {formatBytes(item.size)}
                          {dir ? ` · ${dir}` : ''}
                        </div>
                      </div>
                      <button
                        className="btn btn-ghost"
                        style={{ padding: '5px 11px' }}
                        onClick={() => restore(item)}
                        disabled={itemBusy === item.id}
                        title="Put it back (re-syncs to everyone)"
                      >
                        {itemBusy === item.id ? <Spinner size={13} /> : <RotateCcw size={14} />} Restore
                      </button>
                      <button
                        className="icon-btn"
                        title="Remove this saved copy"
                        onClick={() => forget(item)}
                        disabled={itemBusy === item.id}
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  )
                })
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
