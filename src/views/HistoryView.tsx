import { AnimatePresence, motion } from 'framer-motion'
import {
  ArrowDownToLine,
  CheckCircle2,
  FolderOpen,
  History as HistoryIcon,
  Send,
  Trash2,
  XCircle,
} from 'lucide-react'
import { api, type HistoryEntry } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, LocalityBadge } from '../components/bits'
import { formatBytes, formatRelativeTime } from '../lib/format'

function entryTitle(e: HistoryEntry): string {
  if (e.fileNames.length === 1) return e.fileNames[0]
  if (e.fileNames.length > 1) return `${e.fileNames[0]} + ${e.fileNames.length - 1} more`
  return e.direction === 'receive' ? 'Received files' : 'Files'
}

export function HistoryView() {
  const history = useStore((s) => s.history)
  const reload = useStore((s) => s.reloadHistory)

  const clearAll = async () => {
    await api.clearHistory()
    reload()
  }

  return (
    <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
      <div
        className="titlebar-drag"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 16,
        }}
      >
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>History</h1>
        {history.length > 0 && (
          <button className="btn btn-ghost" onClick={clearAll}>
            <Trash2 size={15} /> Clear
          </button>
        )}
      </div>

      {history.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<HistoryIcon size={24} />}
            title="No transfers yet"
            hint="Files you send and receive will show up here."
          />
        </div>
      ) : (
        <div className="card" style={{ padding: 6 }}>
          <AnimatePresence initial={false}>
            {history.map((e) => {
              const ok = e.state === 'completed'
              const failed = e.state === 'failed'
              const color = ok ? 'var(--green)' : failed ? 'var(--red)' : 'var(--text-faint)'
              const DirIcon = e.direction === 'send' ? Send : ArrowDownToLine
              return (
                <motion.div
                  key={e.id}
                  layout
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  className="row-hover"
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 12,
                    padding: '11px 12px',
                  }}
                >
                  <div
                    style={{
                      width: 36,
                      height: 36,
                      borderRadius: 10,
                      display: 'grid',
                      placeItems: 'center',
                      flexShrink: 0,
                      color,
                      background: `color-mix(in srgb, ${color} 13%, transparent)`,
                    }}
                  >
                    <DirIcon size={17} />
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div
                      style={{
                        fontWeight: 600,
                        fontSize: 14,
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {entryTitle(e)}
                    </div>
                    <div
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        marginTop: 2,
                        fontSize: 12,
                        color: 'var(--text-muted)',
                      }}
                    >
                      <span>{e.direction === 'send' ? 'Sent' : 'Received'}</span>
                      {e.bytesTotal > 0 && <span>· {formatBytes(e.bytesTotal)}</span>}
                      <span>· {formatRelativeTime(e.timestampMs)}</span>
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
                      title="Open folder"
                      onClick={() => api.openPath(e.outDir!)}
                    >
                      <FolderOpen size={15} />
                    </button>
                  )}
                </motion.div>
              )
            })}
          </AnimatePresence>
        </div>
      )}
    </div>
  )
}
