import { MOBILE_UI } from '../lib/platform'
import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ChevronRight, HardDrive, RotateCcw, Settings as SettingsIcon, Trash2 } from 'lucide-react'
import { api, onFolderHistoryChanged, type FolderHistorySummary, type HistoryItem } from '../lib/api'
import { useStore } from '../store'
import { FileIcon } from '../components/FileIcon'
import { Dialog } from '../components/Dialog'
import { EmptyState, IconButton, MenuButton, ProgressBar, SectionHeader, Spinner } from '../components/ui'
import { formatBytes, formatRelativeTime } from '../lib/format'

/** How many saved copies a folder section shows before "Show all". */
const PREVIEW_COUNT = 5

/** "Just now" / "Today 3:42 PM" read naturally mid-sentence ("Deleted today 3:42 PM"). */
const midSentence = (rel: string) => (/^(Just|Today|Yesterday)\b/.test(rel) ? rel[0].toLowerCase() + rel.slice(1) : rel)

type Confirm =
  | { kind: 'all' }
  | { kind: 'folder'; pairId: string; name: string; bytes: number }

/** The "Recoverable files" tab: every shared folder's saved copies of deleted /
 *  replaced files, with a storage summary and ways to free space. */
export function RecoverableFilesView() {
  const toast = useStore((s) => s.toast)
  const budget = useStore((s) => s.settings?.folderHistoryBudgetBytes ?? 2 * 1024 * 1024 * 1024)
  const setView = useStore((s) => s.setView)
  const focusPair = useStore((s) => s.historyFocusPair)
  const clearFocus = useStore((s) => s.clearHistoryFocus)
  const [summaries, setSummaries] = useState<FolderHistorySummary[] | null>(null)
  // Desktop: the folder deep-linked from Shared Folders (shown in full, scrolled to).
  const [focused, setFocused] = useState<string | null>(null)
  // Mobile: the one expanded folder.
  const [open, setOpen] = useState<string | null>(null)
  const [confirm, setConfirm] = useState<Confirm | null>(null)
  const [mobileConfirming, setMobileConfirming] = useState<string | null>(null)
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

  // Deep-link from a folder: open that folder once summaries arrive.
  useEffect(() => {
    if (focusPair && summaries) {
      if (summaries.some((s) => s.pairId === focusPair)) {
        setOpen(focusPair)
        setFocused(focusPair)
      }
      clearFocus()
    }
  }, [focusPair, summaries, clearFocus])

  const total = (summaries ?? []).reduce((s, f) => s + f.bytes, 0)

  const freeAll = async () => {
    setBusy(true)
    try {
      const freed = await api.clearAllFolderHistory()
      toast('success', `Freed ${formatBytes(freed)}`)
      setConfirm(null)
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
      setConfirm(null)
      setMobileConfirming(null)
      await load()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  if (summaries === null) {
    return (
      <div style={{ display: 'grid', placeItems: 'center', padding: 48 }}>
        <Spinner size={18} />
      </div>
    )
  }

  if (MOBILE_UI && summaries.length === 0) return <div className="mobile-empty"><HardDrive /><h2 className="ios-title2">Nothing to recover</h2><p className="ios-footnote">Saved copies of replaced files appear here.</p><button className="ios-button ios-primary" onClick={() => setView('locations')}>Browse locations</button></div>

  if (MOBILE_UI) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
        <div className="mobile-storage">
          <div className="ios-footnote">Saved copies ({budget > 0 ? `${formatBytes(budget)} limit per folder` : 'no storage limit'})</div>
          <div className="ios-title2">{formatBytes(total)}</div>
          {budget > 0 && <ProgressBar percent={(total / Math.max(total, budget * summaries.length, 1)) * 100} />}
          {mobileConfirming === 'all' ? (
            <div className="mobile-stack">
              <button className="ios-button ios-destructive" disabled={busy} onClick={freeAll}>Free {formatBytes(total)}</button>
              <button className="ios-button" disabled={busy} onClick={() => setMobileConfirming(null)}>Cancel</button>
            </div>
          ) : (
            <button className="ios-button" onClick={() => setMobileConfirming('all')}>Free up space</button>
          )}
        </div>
        {summaries.map((f) => (
          <MobileFolderRow
            key={f.pairId}
            summary={f}
            isOpen={open === f.pairId}
            onToggle={() => setOpen(open === f.pairId ? null : f.pairId)}
            confirming={mobileConfirming === f.pairId}
            onAskEmpty={() => setMobileConfirming(mobileConfirming === f.pairId ? null : f.pairId)}
            onEmpty={() => emptyFolder(f.pairId)}
            busy={busy}
            onChanged={load}
          />
        ))}
      </div>
    )
  }

  if (summaries.length === 0) {
    return (
      <EmptyState
        icon={<HardDrive />}
        title="Nothing to recover"
        hint="Files deleted from shared folders show up here."
      />
    )
  }

  const limit = budget > 0 ? budget * summaries.length : 0

  return (
    <div className="rf">
      <SectionHeader>Storage</SectionHeader>
      <div className="group rf-storage">
        <div className="row">
          <span className="rf-glyph" aria-hidden><HardDrive /></span>
          <div className="row-main">
            <div className="row-title tnum">{formatBytes(total)} used</div>
            <div className="row-sub">
              {budget > 0 ? `Up to ${formatBytes(budget)} per folder · old copies are removed automatically` : 'No storage limit'}
            </div>
            {limit > 0 && <ProgressBar percent={(total / Math.max(total, limit, 1)) * 100} label="Storage used by saved copies" />}
          </div>
          <div className="row-trailing">
            <IconButton label="Storage settings" tooltip="Change how long copies are kept" onClick={() => setView('settings')}>
              <SettingsIcon />
            </IconButton>
            <button className="btn btn-danger btn-sm" onClick={() => setConfirm({ kind: 'all' })} disabled={total === 0}>
              Free up space…
            </button>
          </div>
        </div>
      </div>

      {summaries.map((f) => (
        <FolderSection
          key={f.pairId}
          summary={f}
          focused={focused === f.pairId}
          onAskEmpty={() => setConfirm({ kind: 'folder', pairId: f.pairId, name: f.folderName, bytes: f.bytes })}
          onChanged={load}
        />
      ))}

      <AnimatePresence>
        {confirm && (
          <Dialog
            title={confirm.kind === 'all' ? 'Delete all saved copies?' : `Empty “${confirm.name}”?`}
            width={380}
            busy={busy}
            onClose={() => setConfirm(null)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setConfirm(null)} disabled={busy}>Cancel</button>
                <button
                  className="btn btn-destructive"
                  disabled={busy}
                  onClick={() => (confirm.kind === 'all' ? freeAll() : emptyFolder(confirm.pairId))}
                >
                  {busy ? <Spinner size={13} /> : null}
                  {confirm.kind === 'all' ? 'Free up space' : 'Empty'}
                </button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>
              {confirm.kind === 'all'
                ? `This frees ${formatBytes(total)}. Deleted files can’t be restored after this. Your live files aren’t touched.`
                : `This frees ${formatBytes(confirm.bytes)}. Deleted files from this folder can’t be restored after this.`}
            </p>
          </Dialog>
        )}
      </AnimatePresence>
    </div>
  )
}

/** Load (and reload when its count changes) one folder's saved copies. */
function useFolderItems(summary: FolderHistorySummary, enabled: boolean) {
  const [items, setItems] = useState<HistoryItem[] | null>(null)
  const loadedFor = useRef<string | null>(null)
  useEffect(() => {
    if (!enabled) return
    const key = `${summary.pairId}:${summary.itemCount}`
    if (loadedFor.current === key) return
    loadedFor.current = key
    void api
      .listFolderHistory(summary.pairId)
      .then(setItems)
      .catch(() => setItems([]))
  }, [enabled, summary.pairId, summary.itemCount])
  const reload = async () => setItems(await api.listFolderHistory(summary.pairId))
  return [items, reload] as const
}

function FolderSection({
  summary,
  focused,
  onAskEmpty,
  onChanged,
}: {
  summary: FolderHistorySummary
  focused: boolean
  onAskEmpty: () => void
  onChanged: () => Promise<void>
}) {
  const toast = useStore((s) => s.toast)
  const [items, reloadItems] = useFolderItems(summary, true)
  const [itemBusy, setItemBusy] = useState<string | null>(null)
  const [showAll, setShowAll] = useState(false)
  const [forgetting, setForgetting] = useState<HistoryItem | null>(null)
  const ref = useRef<HTMLElement>(null)

  // Deep-linked from its folder: show everything and bring it into view.
  useEffect(() => {
    if (!focused) return
    ref.current?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }, [focused])

  const restore = async (item: HistoryItem) => {
    setItemBusy(item.id)
    try {
      await api.restoreFolderItem(summary.pairId, item.id)
      toast('success', `Restored ${item.relPath.split('/').pop()}`)
      await onChanged()
      await reloadItems()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setItemBusy(null)
    }
  }

  // This deletes the ONLY saved copy of a file, so it always goes through a confirm.
  const forget = async (item: HistoryItem) => {
    setForgetting(null)
    setItemBusy(item.id)
    try {
      await api.forgetFolderItem(summary.pairId, item.id)
      await onChanged()
      await reloadItems()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setItemBusy(null)
    }
  }

  const visible = items ? (showAll || focused ? items : items.slice(0, PREVIEW_COUNT)) : []
  const hidden = items ? items.length - visible.length : 0

  return (
    <section ref={ref}>
      <SectionHeader
        action={
          <MenuButton
            size="sm"
            label={`${summary.folderName} options`}
            items={[{ label: 'Empty saved copies…', icon: <Trash2 />, danger: true, onSelect: onAskEmpty }]}
          />
        }
      >
        <span className="truncate-1" title={summary.folder}>{summary.folderName}</span>
        <span className="rf-section-meta tnum">
          {formatBytes(summary.bytes)} · {summary.itemCount} {summary.itemCount === 1 ? 'item' : 'items'}
        </span>
      </SectionHeader>
      <div className="group rf-items">
        {items === null ? (
          <div className="row" style={{ justifyContent: 'center' }}><Spinner size={14} /></div>
        ) : items.length === 0 ? (
          <div className="row"><span className="row-sub">Nothing saved here.</span></div>
        ) : (
          <>
            {visible.map((item) => {
              const name = item.relPath.split('/').pop() ?? item.relPath
              const dir = item.relPath.includes('/') ? item.relPath.slice(0, item.relPath.lastIndexOf('/')) : ''
              const meta = [
                `${item.reason === 'replaced' ? 'Replaced' : 'Deleted'} ${midSentence(formatRelativeTime(item.timestampMs))}`,
                formatBytes(item.size),
                dir ? `${dir}/` : null,
              ].filter(Boolean).join(' · ')
              return (
                <div key={item.id} className="row">
                  <span className="history-icon" aria-hidden><FileIcon name={name} size={20} /></span>
                  <div className="row-main">
                    <div className="row-title truncate-1" title={item.relPath}>{name}</div>
                    <div className="row-sub truncate-1" title={meta}>{meta}</div>
                  </div>
                  <div className="row-trailing">
                    <button
                      className="btn btn-secondary btn-sm"
                      onClick={() => restore(item)}
                      disabled={itemBusy === item.id}
                      title="Put it back — it syncs to everyone again"
                    >
                      {itemBusy === item.id ? <Spinner size={12} /> : <RotateCcw />} Restore
                    </button>
                    <MenuButton
                      size="sm"
                      label={`More for ${name}`}
                      items={[{ label: 'Delete forever…', icon: <Trash2 />, danger: true, onSelect: () => setForgetting(item) }]}
                    />
                  </div>
                </div>
              )
            })}
            {hidden > 0 && (
              <button type="button" className="row rf-more" onClick={() => setShowAll(true)}>
                Show {hidden} more
              </button>
            )}
          </>
        )}
      </div>

      <AnimatePresence>
        {forgetting && (
          <Dialog
            title={`Delete “${forgetting.relPath.split('/').pop()}” forever?`}
            width={380}
            onClose={() => setForgetting(null)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setForgetting(null)}>Cancel</button>
                <button className="btn btn-destructive" onClick={() => void forget(forgetting)}>Delete</button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>This is the only saved copy. It can’t be restored after this.</p>
          </Dialog>
        )}
      </AnimatePresence>
    </section>
  )
}

/** The phone layout's folder row (unchanged behaviour; the web phone UI is retired). */
function MobileFolderRow({
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
  const [items, reloadItems] = useFolderItems(summary, isOpen)
  const [itemBusy, setItemBusy] = useState<string | null>(null)
  /** Item id whose Forget button is ARMED (two-tap confirm; auto-disarms in 4s). */
  const [confirmForget, setConfirmForget] = useState<string | null>(null)

  const restore = async (item: HistoryItem) => {
    setItemBusy(item.id)
    try {
      await api.restoreFolderItem(summary.pairId, item.id)
      toast('success', `Restored ${item.relPath.split('/').pop()}`)
      await onChanged()
      await reloadItems()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setItemBusy(null)
    }
  }

  const forget = async (item: HistoryItem) => {
    if (confirmForget !== item.id) {
      setConfirmForget(item.id)
      window.setTimeout(() => setConfirmForget((v) => (v === item.id ? null : v)), 4000)
      return
    }
    setConfirmForget(null)
    setItemBusy(item.id)
    try {
      await api.forgetFolderItem(summary.pairId, item.id)
      await onChanged()
      await reloadItems()
    } finally {
      setItemBusy(null)
    }
  }

  return <section className="ios-list">
    <div className="ios-row"><button className="mobile-entry" aria-expanded={isOpen} onClick={onToggle}><span className="mobile-tinted-icon"><HardDrive size={22} /></span><span className="mobile-grow"><span className="ios-headline mobile-ellipsis">{summary.folderName}</span><span className="ios-footnote">{formatBytes(summary.bytes)} · {summary.itemCount} items</span></span><ChevronRight size={18} /></button><button className="ios-icon ios-destructive" aria-label="Empty saved copies" disabled={busy} onClick={onAskEmpty}><Trash2 size={18} /></button></div>
    {confirming && <div className="mobile-inset mobile-stack"><p className="ios-footnote">Delete this folder’s saved copies?</p><button className="ios-button ios-destructive" disabled={busy} onClick={onEmpty}>Empty saved copies</button><button className="ios-button" disabled={busy} onClick={onAskEmpty}>Cancel</button></div>}
    {isOpen && <div aria-busy={items === null}>
      {items === null ? <p className="mobile-inset ios-footnote">Loading saved copies…</p> : !items.length ? <p className="mobile-inset ios-footnote">Nothing saved here.</p> : items.map(item => <div className="ios-row" key={item.id}><span className="mobile-tinted-icon"><FileIcon name={item.relPath} size={22} /></span><div className="mobile-grow"><h3 className="ios-headline mobile-ellipsis">{item.relPath.split('/').pop()}</h3><p className="ios-footnote">{formatBytes(item.size)} · {formatRelativeTime(item.timestampMs)}</p><div className="mobile-recovery-actions"><button className="ios-button" disabled={itemBusy === item.id} onClick={() => void restore(item)}>Restore</button><button className="ios-button ios-destructive" disabled={itemBusy === item.id} onClick={() => void forget(item)}>{confirmForget === item.id ? 'Delete forever?' : 'Forget'}</button></div></div></div>)}
    </div>}
  </section>
}
