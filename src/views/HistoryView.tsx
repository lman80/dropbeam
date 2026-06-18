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
} from 'lucide-react'
import { api, type HistoryEntry } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, LocalityBadge } from '../components/bits'
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

  return (
    <div style={{ maxWidth: 680, margin: '0 auto', padding: '8px 28px 36px' }}>
      <div
        className="titlebar-drag"
        style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14 }}
      >
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>History</h1>
        {tab === 'recents' && history.length > 0 && (
          <button className="btn btn-ghost" onClick={clearAll} title="Clears this list — your files aren't touched">
            <Trash2 size={15} /> Clear list
          </button>
        )}
      </div>

      {/* segmented tabs */}
      <div className="seg" style={{ display: 'flex', width: '100%', marginBottom: 16 }}>
        <button
          className={tab === 'recents' ? 'active' : ''}
          style={{ flex: 1, justifyContent: 'center' }}
          onClick={() => setTab('recents')}
        >
          Recents
        </button>
        <button
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
      <div style={{ position: 'relative' }}>
        <Search
          size={15}
          style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', color: 'var(--text-faint)' }}
        />
        <input
          className="input"
          placeholder="Search files & people"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          style={{ padding: '9px 12px 9px 34px', fontSize: 13.5, width: '100%' }}
        />
      </div>

      {groups.length === 0 ? (
        <div className="card">
          <EmptyState icon={<Search size={22} />} title="No matches" hint="Try a different file name or person." />
        </div>
      ) : (
        groups.map((g) => (
          <div key={g.label}>
            <div
              style={{
                fontSize: 11.5,
                fontWeight: 700,
                letterSpacing: '0.04em',
                textTransform: 'uppercase',
                color: 'var(--text-faint)',
                margin: '2px 4px 7px',
              }}
            >
              {g.label}
            </div>
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

  return (
    <motion.div
      layout
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="row-hover"
      style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '10px 12px', borderRadius: 9 }}
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
        <div style={{ fontWeight: 600, fontSize: 14, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
          {entryTitle(e)}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 7, marginTop: 2, fontSize: 12, color: 'var(--text-muted)' }}>
          <span>
            {e.direction === 'send' ? 'Sent' : 'Received'}
            {e.peer ? ` ${e.direction === 'send' ? 'to' : 'from'} ${e.peer}` : ''}
          </span>
          {e.bytesTotal > 0 && <span>· {formatBytes(e.bytesTotal)}</span>}
          <span>· {timeOfDay(e.timestampMs)}</span>
          <LocalityBadge locality={e.locality} />
        </div>
      </div>

      {ok ? (
        <CheckCircle2 size={16} color="var(--green)" style={{ flexShrink: 0 }} />
      ) : failed ? (
        <XCircle size={16} color="var(--red)" style={{ flexShrink: 0 }} />
      ) : null}
      {e.direction === 'receive' && e.outDir && ok && (
        <button
          className="icon-btn"
          title={e.fileNames.length === 1 ? 'Show in folder' : 'Open folder'}
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
    </motion.div>
  )
}
