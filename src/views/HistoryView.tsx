import { MobileHeader } from '../components/MobileHeader'
import { integrityLabel } from '../lib/integrity'
import { ChevronRight } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'
import { useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { FolderOpen, History as HistoryIcon, Search, Trash2, X } from 'lucide-react'
import { api, type HistoryEntry } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, IconButton, MenuButton, SectionHeader, Segmented, Tooltip } from '../components/ui'
import { Dialog } from '../components/Dialog'
import { peerLabel } from '../lib/humanize'
import { IntegrityDetails } from '../components/IntegrityDetails'
import { FileIcon } from '../components/FileIcon'
import { RecoverableFilesView } from './RecoverableFilesView'
import { formatBytes } from '../lib/format'

type Tab = 'recents' | 'recoverable'

function entryTitle(e: HistoryEntry): string {
  if (e.fileNames.length === 1) return e.fileNames[0]
  if (e.fileNames.length > 1) return `${e.fileNames[0]} + ${e.fileNames.length - 1} more`
  return e.direction === 'receive' ? 'Received files' : 'Files'
}

function timeOfDay(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
}

/** Time for today/yesterday (the section says which day), a short date before that. */
function whenLabel(ms: number): string {
  const group = dayGroup(ms)
  if (group === 'Today' || group === 'Yesterday') return timeOfDay(ms)
  const d = new Date(ms)
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
  })
}

/** Files-app style date buckets: Today / Yesterday / Last 7 days / month. */
function dayGroup(ms: number): string {
  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const dayMs = 86_400_000
  if (ms >= startOfToday) return 'Today'
  if (ms >= startOfToday - dayMs) return 'Yesterday'
  if (ms >= startOfToday - 7 * dayMs) return 'Last 7 days'
  const d = new Date(ms)
  return d.toLocaleDateString(undefined, {
    month: 'long',
    year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric',
  })
}

export function HistoryView() {
  const history = useStore((s) => s.history)
  const reload = useStore((s) => s.reloadHistory)
  const focusPair = useStore((s) => s.historyFocusPair)
  // A deep-link from a folder lands on the Recoverable tab.
  const [tab, setTab] = useState<Tab>(focusPair ? 'recoverable' : 'recents')
  const [query, setQuery] = useState('')
  const [confirmClear, setConfirmClear] = useState(false)

  const clearAll = async () => {
    await api.clearHistory()
    reload()
  }

  if (MOBILE_UI) return <div className="mobile-page mobile-history"><MobileHeader title="History" actions={tab === 'recents' && history.length > 0 ? <button className="ios-icon ios-destructive" aria-label="Clear history list" onClick={clearAll}><Trash2 size={20} /></button> : undefined} />
    <div className="mobile-inset mobile-segmented">{(['recents', 'recoverable'] as const).map(value => <button key={value} aria-pressed={tab === value} onClick={() => setTab(value)}>{value === 'recents' ? 'Recents' : 'Recoverable'}</button>)}</div>
    {tab === 'recents' ? <Recents history={history} query={query} setQuery={setQuery} /> : <div className="mobile-inset"><RecoverableFilesView /></div>}
  </div>

  return (
    <div className="page">
      <div className="page-header titlebar-drag">
        <h1 className="page-title">History</h1>
        {tab === 'recents' && history.length > 0 && (
          <div className="page-actions">
            <MenuButton
              label="More"
              items={[{ label: 'Clear list…', icon: <Trash2 />, onSelect: () => setConfirmClear(true) }]}
            />
          </div>
        )}
      </div>

      <div className="history-toolbar">
        <Segmented
          role="tablist"
          label="History"
          value={tab}
          onChange={setTab}
          options={[
            { value: 'recents', label: 'Recents' },
            { value: 'recoverable', label: 'Recoverable files' },
          ]}
        />
        {tab === 'recents' && history.length > 0 && (
          <label className="search-field history-search">
            <Search />
            <input
              className="input"
              type="search"
              aria-label="Search history"
              placeholder="Search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Escape' && query) { e.preventDefault(); setQuery('') } }}
            />
          </label>
        )}
      </div>

      {tab === 'recents' ? (
        <Recents history={history} query={query} setQuery={setQuery} />
      ) : (
        <RecoverableFilesView />
      )}

      <AnimatePresence>
        {confirmClear && (
          <Dialog
            title="Clear the history list?"
            width={380}
            onClose={() => setConfirmClear(false)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setConfirmClear(false)}>Cancel</button>
                <button className="btn btn-destructive" onClick={() => { setConfirmClear(false); void clearAll() }}>Clear list</button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>The files themselves aren’t touched.</p>
          </Dialog>
        )}
      </AnimatePresence>
    </div>
  )
}

function Recents({
  history,
  query,
  setQuery,
}: {
  history: HistoryEntry[]
  query: string
  setQuery: (v: string) => void
}) {
  const groups = useMemo(() => {
    const q = query.trim().toLowerCase()
    const filtered = q
      ? history.filter(
          (e) =>
            e.fileNames.some((n) => n.toLowerCase().includes(q)) ||
            (e.peer ?? '').toLowerCase().includes(q),
        )
      : history
    // history is newest-first → groups appear in chronological-bucket order.
    const out: { label: string; entries: HistoryEntry[] }[] = []
    for (const e of filtered) {
      const label = dayGroup(e.timestampMs)
      const last = out[out.length - 1]
      if (last && last.label === label) last.entries.push(e)
      else out.push({ label, entries: [e] })
    }
    return out
  }, [history, query])

  if (MOBILE_UI) return <>
    {history.length > 0 && <div className="mobile-inset"><input className="mobile-search" aria-label="Search history" placeholder="Search files & people" value={query} onChange={e => setQuery(e.target.value)} /></div>}
    {groups.map(g => <section key={g.label}><h2 className="ios-section-title">{g.label}</h2><div className="ios-list">{g.entries.map(e => <RecentRow key={e.id} e={e} />)}</div></section>)}
    {!groups.length && <div className="mobile-empty"><HistoryIcon /><h2 className="ios-title2">{query ? 'No matches' : 'No transfers yet'}</h2><p className="ios-footnote">{query ? 'Try another name.' : 'Your transfers appear here.'}</p><button className="ios-button ios-primary" onClick={() => query ? setQuery('') : useStore.getState().setView('send')}>{query ? 'Clear search' : 'Send files'}</button></div>}
  </>

  if (history.length === 0) {
    return <EmptyState icon={<HistoryIcon />} title="No transfers yet" hint="Files you send and receive show up here." />
  }

  if (groups.length === 0) {
    return <EmptyState icon={<Search />} title="No matches" hint="Try another file name or person." />
  }

  return (
    <>
      {groups.map((g) => (
        <section key={g.label}>
          <SectionHeader>{g.label}</SectionHeader>
          <div className="group history-group">
            {g.entries.map((e) => (
              <RecentRow key={e.id} e={e} />
            ))}
          </div>
        </section>
      ))}
    </>
  )
}

function RecentRow({ e }: { e: HistoryEntry }) {
  const ok = e.state === 'completed'
  const failed = e.state === 'failed'

  if (MOBILE_UI) return <div className="ios-row"><span className="mobile-tinted-icon"><FileIcon name={e.fileNames[0] ?? ''} size={22} /></span><div className="mobile-grow"><h3 className="ios-headline mobile-ellipsis">{entryTitle(e)}</h3><p className="ios-footnote">{e.peer ?? 'Peer'} · {formatBytes(e.bytesTotal)}{e.locality !== 'unknown' && ` · ${e.locality === 'internet' ? 'Relay' : 'Direct'}`} · {ok ? integrityLabel(e.integrity ?? [], e.bytesTotal, true) : e.state}</p></div>{ok && e.outDir && <button className="ios-button" aria-label={`Show ${entryTitle(e)}`} onClick={() => void api.shareFiles(e.fileNames.map(n => `${e.outDir}/${n}`)).catch(error => useStore.getState().toast('error', String(error)))}>Show<ChevronRight size={16} /></button>}</div>

  const who = peerLabel(e.peer)
  const verb = e.direction === 'send' ? 'Sent' : 'Received'
  const meta = [
    who ? `${verb} ${e.direction === 'send' ? 'to' : 'from'} ${who}` : verb,
    e.bytesTotal > 0 ? formatBytes(e.bytesTotal) : null,
    whenLabel(e.timestampMs),
  ].filter(Boolean).join(' · ')
  const canReveal = e.direction === 'receive' && !!e.outDir && ok
  const canRetry = failed && e.direction === 'send' && hasRetryPayload(e.id)
  const remove = () => {
    void api.removeHistoryEntry(e.id)
      .then(() => useStore.getState().reloadHistory())
      .catch((err) => useStore.getState().toast('error', String(err)))
  }

  return (
    <div className="row history-row">
      <span className="history-icon" aria-hidden><FileIcon name={e.fileNames[0] ?? ''} size={20} /></span>
      <div className="row-main">
        <div className="row-title truncate-1" title={e.fileNames.join('\n')}>{entryTitle(e)}</div>
        <div className="row-sub truncate-1 tnum" title={meta}>{meta}</div>
        <IntegrityDetails rows={e.integrity} total={e.bytesTotal} completed={ok} />
      </div>
      <div className="row-trailing history-trailing">
        {failed && (
          <>
            <Tooltip label={e.error || undefined}>
              <span className="history-state failed" tabIndex={e.error ? 0 : undefined}>
                {e.direction === 'send' ? 'Couldn’t send' : 'Couldn’t receive'}
              </span>
            </Tooltip>
            {canRetry && (
              <button className="btn btn-plain btn-sm" onClick={() => void useStore.getState().retryTransfer(e.id)}>
                Retry
              </button>
            )}
          </>
        )}
        {e.state === 'canceled' && <span className="history-state">Canceled</span>}
        {canReveal && (
          <IconButton
            label={e.fileNames.length === 1 ? 'Show in Finder' : 'Open folder'}
            onClick={() => {
              const sep = e.outDir!.includes('\\') ? '\\' : '/'
              if (e.fileNames.length === 1) {
                api.revealPath(`${e.outDir}${sep}${e.fileNames[0]}`).catch(() => {})
              } else {
                api.openPath(e.outDir!).catch(() => {})
              }
            }}
          >
            <FolderOpen />
          </IconButton>
        )}
        <IconButton
          className="history-row-remove"
          label="Remove from list"
          tooltip="Remove from list — the files stay put"
          onClick={remove}
        >
          <X />
        </IconButton>
      </div>
    </div>
  )
}

/** A failed send can be replayed only while the store still remembers its files
 *  (the same cache store.retryTransfer reads); otherwise there is nothing to retry. */
function hasRetryPayload(id: string): boolean {
  try {
    const all = JSON.parse(localStorage.getItem('dropbeam-retry-payloads') || '{}') as Record<string, unknown>
    return !!all[id]
  } catch {
    return false
  }
}
