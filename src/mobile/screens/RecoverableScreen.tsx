import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Archive } from 'lucide-react'
import type { FolderHistorySummary, HistoryItem } from '../../lib/api'
import { formatBytes, formatRelativeTime } from '../../lib/format'
import { Spinner } from '../../components/bits'
import { ActionSheet, Button, EmptyState, Row, Section } from '../kit'
import { FileTile } from './BrowserControls'

export function RecoverableScreen({ summaries, total, budget, children }: { summaries: FolderHistorySummary[]; total: number; budget: number; children: ReactNode }) {
  const allowance = budget * Math.max(summaries.length, 1)
  return <div className="files-recovery"><Section footer={`Saved copies of deleted or replaced files are removed automatically. Your live files are never touched.${budget ? ` Each folder can keep up to ${formatBytes(budget)}.` : ' No storage limit is configured.'}`}>
    <Row title="Saved Copies" value={budget ? `${formatBytes(total)} of ${formatBytes(allowance)}` : formatBytes(total)} />
    <div className="files-storage" role="progressbar" aria-label="Saved copies storage" aria-valuemin={0} aria-valuemax={100} aria-valuenow={budget ? Math.min(100, total / allowance * 100) : 0}><i style={{ width: `${budget ? Math.min(100, total / allowance * 100) : 0}%` }} /></div>
  </Section>{summaries.some(f => f.itemCount > 0) ? children : <EmptyState icon={<Archive />} title="Nothing to Recover" />}</div>
}

export function RecoverableFolder({ summary, items, busy, itemBusy, confirming, askEmpty, empty, restore, forget, query, focused }: {
  summary: FolderHistorySummary; items: HistoryItem[] | null; busy: boolean; itemBusy: string | null; confirming: boolean
  askEmpty: () => void; empty: () => void; restore: (item: HistoryItem) => Promise<void>; forget: (item: HistoryItem) => Promise<void>; query: string; focused: boolean
}) {
  const [item, setItem] = useState<HistoryItem | null>(null)
  const section = useRef<HTMLDivElement>(null)
  useEffect(() => { if (focused) section.current?.scrollIntoView({ block: 'nearest' }) }, [focused])
  const shown = items?.filter(item => `${summary.folderName} ${item.relPath}`.toLowerCase().includes(query.trim().toLowerCase()))
  if (shown?.length === 0 && query) return null
  return <div className="files-folder-section" ref={section}>
    <Section title={summary.folderName}>
      {items === null ? <div className="files-loading" role="status" aria-label="Loading saved copies"><Spinner size={18} /></div> : shown?.map(item => <Row key={item.id} title={item.relPath.split('/').pop() || item.relPath} icon={<FileTile name={item.relPath} />} subtitle={`${formatBytes(item.size)} · ${formatRelativeTime(item.timestampMs)}`} accessory="chevron" disabled={busy || !!itemBusy} onPress={() => setItem(item)} />)}
    </Section>
    <Button className="files-empty-folder" disabled={busy || !!itemBusy || !summary.itemCount} onClick={askEmpty} aria-label={`Empty ${summary.folderName}`}>Empty</Button>
    {confirming && <ActionSheet title={`Empty ${summary.folderName}`} message="Delete all saved copies in this folder? This cannot be undone." onClose={askEmpty} dismissOnAction={false} actions={[{ label: 'Empty Saved Copies', destructive: true, disabled: busy, onPress: empty }]} />}
    {item && <ActionSheet title={item.relPath.split('/').pop()} message={item.relPath.split('/').pop()} onClose={() => setItem(null)} actions={[
      { label: 'Restore', onPress: () => { void restore(item) } },
      { label: 'Delete Forever', destructive: true, onPress: () => { void forget(item) } },
    ]} />}
  </div>
}
