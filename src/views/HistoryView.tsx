import { MobileHeader } from '../components/MobileHeader'
import { integrityLabel } from '../lib/integrity'
import { ChevronRight } from 'lucide-react'
import { ShareFilesButton } from '../components/ShareFilesButton'
import { MOBILE_UI } from '../lib/platform'
import { useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import {
  ArrowDownToLine,
  CheckCircle2,
  FolderOpen,
  History as HistoryIcon,
  Search,
  Send,
  Trash2,
  XCircle,
  X,
} from 'lucide-react'
import { api, type HistoryEntry } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, LocalityBadge } from '../components/bits'
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
        <div>
          <h1 className="page-title">History</h1>
          <p className="page-subtitle">Everything you’ve sent and received, plus files you can bring back.</p>
        </div>
        {tab === 'recents' && history.length > 0 && (
          <div className="page-actions">
            <button className="btn btn-ghost" onClick={clearAll} title="Clears this list — your files aren't touched">
              <Trash2 size={15} /> Clear list
            </button>
          </div>
        )}
      </div>

      {/* segmented tabs */}
      <div className="seg" role="tablist" aria-label="History" style={{ display: 'flex', width: '100%', marginBottom: 16, boxSizing: 'border-box' }}>
        <button
          role="tab"
          aria-selected={tab === 'recents'}
          className={tab === 'recents' ? 'active' : ''}
          style={{ flex: 1, justifyContent: 'center' }}
          onClick={() => setTab('recents')}
        >
          Recents
        </button>
        <button
          role="tab"
          aria-selected={tab === 'recoverable'}
          className={tab === 'recoverable' ? 'active' : ''}
          style={{ flex: 1, justifyContent: 'center' }}
          onClick={() => setTab('recoverable')}
        >
          Recoverable files
        </button>
      </div>

      {tab === 'recents' ? (
        <Recents history={history} query={query} setQuery={setQuery} />
      ) : (
        <RecoverableFilesView />
      )}
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
    return (
      <div className="card">
        <EmptyState
          icon={<HistoryIcon size={24} />}
          title="No transfers yet"
          hint="Files you send and receive will show up here."
        />
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <label className="search-field">
        <Search size={15} />
        <input
          className="input"
          type="search"
          aria-label="Search history"
          placeholder="Search files & people"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Escape' && query) { e.preventDefault(); setQuery('') } }}
        />
      </label>

      {groups.length === 0 ? (
        <div className="card">
          <EmptyState icon={<Search size={22} />} title="No matches" hint="Try a different file name or person." />
        </div>
      ) : (
        groups.map((g) => (
          <div key={g.label}>
            <h2 className="section-title" style={{ margin: '2px 4px 7px' }}>{g.label}</h2>
            <div className="card" style={{ padding: 6 }}>
              <AnimatePresence initial={false}>
                {g.entries.map((e) => (
                  <RecentRow key={e.id} e={e} />
                ))}
              </AnimatePresence>
            </div>
          </div>
        ))
      )}
    </div>
  )
}

function RecentRow({ e }: { e: HistoryEntry }) {
  const ok = e.state === 'completed'
  const failed = e.state === 'failed'
  const DirIcon = e.direction === 'send' ? Send : ArrowDownToLine

  if (MOBILE_UI) return <div className="ios-row"><span className="mobile-tinted-icon"><FileIcon name={e.fileNames[0] ?? ''} size={22} /></span><div className="mobile-grow"><h3 className="ios-headline mobile-ellipsis">{entryTitle(e)}</h3><p className="ios-footnote">{e.peer ?? 'Peer'} · {formatBytes(e.bytesTotal)}{e.locality !== 'unknown' && ` · ${e.locality === 'internet' ? 'Relay' : 'Direct'}`} · {ok ? integrityLabel(e.integrity ?? [], e.bytesTotal, true) : e.state}</p></div>{ok && e.outDir && <button className="ios-button" aria-label={`Show ${entryTitle(e)}`} onClick={() => void api.shareFiles(e.fileNames.map(n => `${e.outDir}/${n}`)).catch(error => useStore.getState().toast('error', String(error)))}>Show<ChevronRight size={16} /></button>}</div>

  return (
    <motion.div
      layout
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="row-hover"
      style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '10px 12px' }}
    >
      <div style={{ position: 'relative', flexShrink: 0, width: 34, height: 34 }}>
        <div
          style={{
            width: 34,
            height: 34,
            borderRadius: 9,
            display: 'grid',
            placeItems: 'center',
            background: 'var(--surface-2)',
          }}
        >
          <FileIcon name={e.fileNames[0] ?? ''} size={18} />
        </div>
        {/* tiny direction chip */}
        <div
          style={{
            position: 'absolute',
            right: -4,
            bottom: -4,
            width: 16,
            height: 16,
            borderRadius: 999,
            display: 'grid',
            placeItems: 'center',
            background: 'var(--surface)',
            border: '1.5px solid var(--surface)',
            color: e.direction === 'send' ? 'var(--accent)' : 'var(--green)',
          }}
        >
          <DirIcon size={10} />
        </div>
      </div>

      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontWeight: 600, fontSize: 'var(--font-base)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }} title={e.fileNames.join('\n')}>
          {entryTitle(e)}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: '2px 7px', marginTop: 2, fontSize: 'var(--font-xs)', color: 'var(--text-muted)', flexWrap: 'wrap', minWidth: 0 }}>
          <span className="truncate-1" style={{ maxWidth: '100%' }}>
            {e.direction === 'send' ? 'Sent' : 'Received'}
            {e.peer ? ` ${e.direction === 'send' ? 'to' : 'from'} ${e.peer}` : ''}
          </span>
          {e.bytesTotal > 0 && <span>· {formatBytes(e.bytesTotal)}</span>}
          <span>· {timeOfDay(e.timestampMs)}</span>
          <LocalityBadge locality={e.locality} />
        </div>
        <IntegrityDetails rows={e.integrity} total={e.bytesTotal} completed={ok} />
      </div>

      {ok ? (
        <CheckCircle2 size={16} color="var(--green)" style={{ flexShrink: 0 }} />
      ) : failed ? (
        <XCircle size={16} color="var(--red)" style={{ flexShrink: 0 }} />
      ) : null}
      {MOBILE_UI && e.direction === 'receive' && e.outDir && ok && (
            <ShareFilesButton outDir={e.outDir} fileNames={e.fileNames} />
          )}
          {!MOBILE_UI && e.direction === 'receive' && e.outDir && ok && (
        <button
          className="icon-btn"
          title={e.fileNames.length === 1 ? 'Show in folder' : 'Open folder'}
          aria-label={e.fileNames.length === 1 ? 'Show in folder' : 'Open folder'}
          onClick={() => {
            const sep = e.outDir!.includes('\\') ? '\\' : '/'
            if (e.fileNames.length === 1) {
              api.revealPath(`${e.outDir}${sep}${e.fileNames[0]}`).catch(() => {})
            } else {
              api.openPath(e.outDir!).catch(() => {})
            }
          }}
        >
          <FolderOpen size={15} />
        </button>
      )}
      {!MOBILE_UI && (
        <button
          className="icon-btn icon-btn-danger history-row-remove"
          title="Remove from this list — the files stay where they are"
          aria-label={`Remove ${entryTitle(e)} from history`}
          onClick={() => {
            void api.removeHistoryEntry(e.id)
              .then(() => useStore.getState().reloadHistory())
              .catch((err) => useStore.getState().toast('error', String(err)))
          }}
        >
          <X size={15} />
        </button>
      )}
    </motion.div>
  )
}
