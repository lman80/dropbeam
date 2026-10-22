import { useState } from 'react'
import { Check, Folder } from 'lucide-react'
import type { LocationEntry, SharedLocation } from '../../lib/api'
import { formatBytes } from '../../lib/format'
import { Spinner } from '../../components/bits'
import { ActionSheet, Alert, Button, EmptyState, Row, Screen, SearchField, Section, TextField, type MenuAction } from '../kit'
import { FileTile, FilesMenu } from './BrowserControls'

type Action = 'mkdir' | 'rename' | 'trash'
interface BrowserProps {
  location: SharedLocation; path: string; entries: LocationEntry[]; selected: string[]; selecting: boolean; query: string
  loading: boolean; busy: boolean; error: string; hasMore: boolean; dialog: Action | null; name: string; validName: boolean
  actionError: string; trashResults: { name: string; error?: string; trashPath?: string }[]
  back: () => void; navigate: (name: string) => void; toggle: (entry: LocationEntry) => void; selectFile: (name: string) => void
  setSelecting: () => void; search: (query: string) => void; sort: (sort: string) => void; showMore: () => void; refresh: () => void
  download: () => Promise<void>; pick: (folder: boolean) => Promise<void>; openDialog: (action: Action) => void
  closeDialog: () => void; setName: (name: string) => void; run: () => Promise<void>
}
export function BrowserScreen(p: BrowserProps) {
  const [file, setFile] = useState<string | null>(null)
  const parts = p.path.split('/').filter(Boolean)
  const blocked = p.busy || p.loading || !!p.error
  const menu: MenuAction[] = [
    ...(p.location.rights.manage ? [{ label: 'New Folder', disabled: blocked, onPress: () => p.openDialog('mkdir') }] : []),
    ...(p.location.rights.upload ? [{ label: 'Upload Files', disabled: blocked, onPress: () => { void p.pick(false) } }, { label: 'Upload Folder', disabled: blocked, onPress: () => { void p.pick(true) } }] : []),
    { label: 'Sort by Name', disabled: p.busy, onPress: () => p.sort('name') },
    { label: 'Sort by Date', disabled: p.busy, onPress: () => p.sort('modified') },
  ]
  return <div className={`files-screen${p.selecting ? ' files-browser-selecting' : ''}`}><Screen title={p.selecting ? `${p.selected.length} Selected` : parts.at(-1) || p.location.name}
    leading={{ title: parts.length ? parts.at(-2) || p.location.name : 'Locations', onPress: () => { if (!p.busy) p.back() } }}
    trailing={<><Button disabled={p.busy} onClick={p.setSelecting}>{p.selecting ? 'Done' : 'Select'}</Button><FilesMenu label="Folder actions" actions={menu} /></>}>
    <SearchField value={p.query} onChange={p.search} placeholder="Search" disabled={p.busy} />
    {p.error && <Section footer={<span role="alert">{p.error}</span>}><Row title="Try Again" tint onPress={p.refresh} disabled={p.busy || p.loading} /></Section>}
    {p.busy && <Section footer={<span role="status">Working…</span>} />}
    {!!p.entries.length && <Section>{p.entries.map(entry => <div className="files-browser-row" key={entry.name}>
      {p.selecting && <Button className="files-select-target" role="checkbox" aria-checked={p.selected.includes(entry.name)} aria-label={`Select ${entry.name}`} disabled={p.busy || p.loading} onClick={() => p.toggle(entry)}><span className={`files-check${p.selected.includes(entry.name) ? ' is-selected' : ''}`} aria-hidden="true">{p.selected.includes(entry.name) && <Check size={16} />}</span></Button>}
      <Row title={entry.name} icon={<>{entry.isDir ? <span className="files-tile files-tile-folder"><Folder size={22} /></span> : <FileTile name={entry.name} />}
    </>} subtitle={entry.isDir ? 'Folder' : `${formatBytes(entry.size)}${entry.modified ? ` · ${new Date(entry.modified).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })}` : ''}`} accessory={!p.selecting && entry.isDir ? 'chevron' : 'none'} disabled={p.busy || p.loading} onPress={() => {
      if (p.selecting) p.toggle(entry)
      else if (entry.isDir) p.navigate(entry.name)
      else { p.selectFile(entry.name); setFile(entry.name) }
    }} /></div>)}{p.hasMore && <Row title="Show more" tint disabled={p.loading || p.busy} onPress={p.showMore} />}</Section>}
    {p.loading && <div className="files-loading" role="status" aria-label="Loading folder"><Spinner size={18} /></div>}
    {!p.loading && !p.error && !p.entries.length && <EmptyState icon={<Folder />} title={p.query ? 'No Results' : 'Folder is Empty'} />}
    {!!p.trashResults.length && <Section title="Trash Results">{p.trashResults.map(result => <Row key={result.name} title={result.name} subtitle={result.error ? <span className="mk-destructive">{result.error}</span> : 'Moved to Trash'} icon={<FileTile name={result.name} />} />)}</Section>}
    {p.actionError && !p.dialog && <Section footer={<span className="mk-destructive" role="alert">{p.actionError}</span>} />}
  </Screen>
    {p.selecting && <div className="files-toolbar" role="toolbar" aria-label="Selected files"><Button disabled={!p.selected.length || blocked} onClick={() => void p.download()}>Download</Button>{p.location.rights.manage && <>{p.selected.length === 1 && <Button disabled={blocked} onClick={() => p.openDialog('rename')}>Rename</Button>}<Button destructive disabled={!p.selected.length || blocked} onClick={() => p.openDialog('trash')}>Trash</Button></>}</div>}
    {file && <ActionSheet title={file} message={file} onClose={() => setFile(null)} actions={[
      { label: 'Download', onPress: () => { void p.download() } },
      ...(p.location.rights.manage ? [{ label: 'Rename', onPress: () => p.openDialog('rename') }, { label: 'Move to Trash', destructive: true, onPress: () => p.openDialog('trash') }] : []),
    ]} />}
    {p.dialog === 'trash' && <ActionSheet title="Move to Trash" message="These items can be restored by the folder owner." onClose={() => { if (!p.busy) p.closeDialog() }} dismissOnAction={false} actions={[{ label: `Move ${p.selected.length} ${p.selected.length === 1 ? 'item' : 'items'} to Trash`, destructive: true, disabled: p.busy || !p.selected.length, onPress: () => { void p.run().then(p.closeDialog) } }]} />}
    {(p.dialog === 'mkdir' || p.dialog === 'rename') && <Alert title={p.dialog === 'mkdir' ? 'New Folder' : 'Rename'} message={p.actionError || undefined} onClose={() => { if (!p.busy) p.closeDialog() }} actions={[
      { label: 'Cancel', disabled: p.busy, onPress: p.closeDialog },
      { label: p.dialog === 'mkdir' ? 'Create' : 'Save', disabled: p.busy || !p.validName, onPress: () => { void p.run() } },
    ]}><form onSubmit={event => { event.preventDefault(); if (!p.busy && p.validName) void p.run() }}><TextField label={p.dialog === 'mkdir' ? 'Folder name' : 'New name'} autoFocus maxLength={200} value={p.name} onChange={event => p.setName(event.target.value)} disabled={p.busy} /></form></Alert>}
  </div>
}
