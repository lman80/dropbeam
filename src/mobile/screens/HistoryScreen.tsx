import { useRef, useState, type ReactNode } from 'react'
import { History as HistoryIcon } from 'lucide-react'
import type { HistoryEntry } from '../../lib/api'
import { api } from '../../lib/api'
import { formatBytes } from '../../lib/format'
import { useStore } from '../../store'
import { Alert, ContextMenu, EmptyState, Row, Screen, SearchField, Section, SegmentedControl } from '../kit'
import { FileTile, FilesMenu } from './BrowserControls'
import { groupHistoryDays } from './HistorySections'

export function HistoryScreen({ history, tab, setTab, query, setQuery, clearAll, recoverable }: {
  history: HistoryEntry[]; tab: 'recents' | 'recoverable'; setTab: (tab: 'recents' | 'recoverable') => void
  query: string; setQuery: (query: string) => void; clearAll: () => Promise<void>; recoverable: ReactNode
}) {
  const [confirm, setConfirm] = useState(false)
  const [busy, setBusy] = useState(false)
  // The API exposes only clear-all. Keep individual dismissals local to this device.
  const [hidden, setHidden] = useState<string[]>(() => {
    try { const ids = JSON.parse(localStorage.getItem('dropbeam.hidden-recents') || '[]'); return Array.isArray(ids) ? ids.filter(id => typeof id === 'string') : [] } catch { return [] }
  })
  const toast = useStore(s => s.toast)
  const q = query.trim().toLowerCase()
  const groups = groupHistoryDays(history.filter(e => !hidden.includes(e.id) && (!q || [...e.fileNames, e.peer ?? ''].some(n => n.toLowerCase().includes(q)))))
  const remove = (id: string) => {
    const next = [...hidden, id]; setHidden(next)
    try { localStorage.setItem('dropbeam.hidden-recents', JSON.stringify(next)) } catch { toast('info', 'Removed for this session. Device storage is unavailable.') }
  }
  return <div className="files-screen"><Screen title="History" trailing={tab === 'recents' ? <FilesMenu label="History actions" actions={[{ label: 'Clear History', destructive: true, disabled: busy || !history.length, onPress: () => setConfirm(true) }]} /> : undefined}>
    <div className="files-segments"><SegmentedControl label="History view" options={[{ value: 'recents', label: 'Recents' }, { value: 'recoverable', label: 'Recoverable' }]} value={tab} onChange={setTab} /></div>
    <SearchField value={query} onChange={setQuery} />
    {tab === 'recoverable' ? recoverable : groups.length ? groups.map(group => <Section key={group.label} title={group.label}>{group.entries.map(entry => <HistoryRecent key={entry.id} entry={entry} remove={() => remove(entry.id)} />)}</Section>) : <EmptyState icon={<HistoryIcon />} title={query ? 'No Results' : 'No Recents'} body={query ? 'Try another file name or person.' : 'Files you send and receive will appear here.'} />}
    {confirm && <Alert title="Clear History?" message="This removes your recent transfers. Your files will stay where they are." onClose={() => { if (!busy) setConfirm(false) }} actions={[
      { label: 'Cancel', disabled: busy, onPress: () => setConfirm(false) },
      { label: 'Clear History', destructive: true, disabled: busy, onPress: () => { setBusy(true); void clearAll().then(() => setConfirm(false)).catch(e => toast('error', String(e))).finally(() => setBusy(false)) } },
    ]} />}
  </Screen></div>
}

function HistoryRecent({ entry: e, remove }: { entry: HistoryEntry; remove: () => void }) {
  const toast = useStore(s => s.toast)
  const pointer = useRef({ at: 0, x: 0, y: 0, moved: false })
  const title = e.fileNames.length === 1 ? e.fileNames[0] : `${e.fileNames.length} files`
  const open = () => { if (e.outDir) void api.shareFiles(e.fileNames.length ? e.fileNames.map(n => `${e.outDir}/${n}`) : [e.outDir]).catch(error => toast('error', String(error))) }
  const route = e.locality === 'internet' ? 'Relay' : e.locality === 'local' || e.locality === 'direct' ? 'Direct' : ''
  return <div className="files-recent" onPointerDownCapture={event => { pointer.current = { at: Date.now(), x: event.clientX, y: event.clientY, moved: false } }} onPointerMoveCapture={event => { if (Math.hypot(event.clientX - pointer.current.x, event.clientY - pointer.current.y) > 8) pointer.current.moved = true }} onClickCapture={event => {
    if ((event.target as HTMLElement).closest('.mk-modal')) return
    event.stopPropagation()
    if (!pointer.current.moved && (event.detail === 0 || Date.now() - pointer.current.at < 400)) open()
  }} onKeyDownCapture={event => { if ((event.target as HTMLElement).closest('.mk-modal')) return; if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.stopPropagation(); open() } }}>
    <ContextMenu label={`${title}, actions`} actions={[
      { label: 'Show in Files', disabled: !e.outDir, onPress: open },
      { label: 'Copy name', onPress: () => { void navigator.clipboard.writeText(e.fileNames.join('\n')).catch(error => toast('error', String(error))) } },
      { label: 'Remove from History', destructive: true, onPress: remove },
    ]}><Row title={title} emphasized icon={<FileTile name={e.fileNames[0] ?? ''} />} subtitle={<>{formatBytes(e.bytesTotal)} · {e.direction === 'receive' ? 'From' : 'To'} {e.peer || 'Unknown'}{route && ` · ${route}`}{e.state === 'failed' && <span className="mk-destructive"> · Failed</span>}</>} accessory={e.outDir ? 'chevron' : 'none'} /></ContextMenu>
  </div>
}
