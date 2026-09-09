import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowDownToLine, ArrowUpFromLine, ChevronRight, File, Folder, FolderPlus, Home, Pencil, RefreshCw, Search, Trash2, X } from 'lucide-react'
import { api, locationsApi, onLocationsChanged, onTransferUpdate, type LocationEntry, type LocationPage, type SharedLocation } from '../lib/api'
import { formatBytes } from '../lib/format'
import { rememberLocationUpload, useStore } from '../store'
import './locations.css'

type Action = 'mkdir' | 'rename' | 'trash'
const join = (parent: string, name: string) => parent ? `${parent}/${name}` : name
export function FileBrowser({ friendId, location, online }: { friendId: string; location: SharedLocation; online: string }) {
  const toast = useStore(s => s.toast)
  const [path, setPath] = useState('')
  const [page, setPage] = useState(0)
  const cursors = useRef<(string | undefined)[]>([undefined])
  const [data, setData] = useState<LocationPage>({ entries: [], page: 0, hasMore: false })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [query, setQuery] = useState('')
  const [sort, setSort] = useState('name')
  const [selected, setSelected] = useState<string[]>([])
  const [dialog, setDialog] = useState<Action | null>(null)
  const [name, setName] = useState('')
  const [actionError, setActionError] = useState('')
  const [trashResults, setTrashResults] = useState<{ name: string; error?: string; trashPath?: string }[]>([])
  const [busy, setBusy] = useState(false)
  const [hover, setHover] = useState(false)
  const generation = useRef(0)
  const actionBusy = useRef(false)
  const request = useCallback(<T,>(kind: string, args: Record<string, unknown> = {}) =>
    locationsApi.request<T>(friendId, { kind: `locations.${kind}`, id: location.id, rel_path: path, ...args }), [friendId, location.id, path])
  const refresh = useCallback(async () => {
    const token = ++generation.current
    setLoading(true); setError('')
    try {
      const result = await request<LocationPage>('ls', { cursor: cursors.current[page], sort, query })
      if (token === generation.current) { cursors.current[page] = result.cursor; cursors.current[page + 1] = result.nextCursor; setData(result); setSelected(s => s.filter(name => result.entries.some(e => e.name === name))) }
    } catch(e) { if (token === generation.current) { setError(String(e)); setData({ entries: [], page, hasMore: false }); setSelected([]) } }
    finally { if (token === generation.current) setLoading(false) }
  }, [page, request, sort, query])
  useEffect(() => { void refresh(); return () => { ++generation.current } }, [refresh])
  useEffect(() => {
    let alive = true; const cleanup: (() => void)[] = []
    const add = (un: () => void) => { if (alive) cleanup.push(un); else un() }
    void onLocationsChanged(() => { cursors.current = [undefined]; if (page) setPage(0); else void refresh() }).then(add)
    void onTransferUpdate(u => { if (u.direction === 'send' && u.state === 'completed') { cursors.current = [undefined]; if (page) setPage(0); else void refresh() } }).then(add)
    return () => { alive = false; cleanup.forEach(un => un()) }
  }, [refresh, page])
  const restart = () => { cursors.current = [undefined]; if (page) setPage(0); else void refresh() }
  const navigate = (next: string) => { if (busy) return; cursors.current = [undefined]; setPath(next); setPage(0); setSelected([]); setQuery(''); setData({ entries: [], page: 0, hasMore: false }) }
  const upload = useCallback(async (paths: string[]) => {
    if (!paths.length) return
    if (!location.rights.upload) { toast('error', 'This location is read-only.'); return }
    if (actionBusy.current) return
    actionBusy.current = true; setBusy(true)
    try {
      const u = await locationsApi.upload(friendId, location.id, path, paths)
      rememberLocationUpload(u.id, friendId, location.id, path, paths)
      if (!useStore.getState().transfers[u.id]) useStore.getState().upsertTransfer(u)
      toast('success', 'Upload started. Follow progress in Send & Receive.')
    } catch(e) { toast('error', String(e)) }
    finally { actionBusy.current = false; setBusy(false) }
  }, [friendId, location.id, location.rights.upload, path, toast])
  useEffect(() => {
    const drop = (e: Event) => { setHover(false); void upload((e as CustomEvent<string[]>).detail) }
    const hover = (e: Event) => setHover(Boolean((e as CustomEvent<boolean>).detail))
    window.addEventListener('dropbeam:location-drop', drop); window.addEventListener('dropbeam:location-hover', hover)
    return () => { window.removeEventListener('dropbeam:location-drop', drop); window.removeEventListener('dropbeam:location-hover', hover) }
  }, [upload])
  const entries = data.entries
  const selectedEntries = data.entries.filter(e => selected.includes(e.name))
  const run = async () => {
    if (!dialog || actionBusy.current) return
    actionBusy.current = true; setBusy(true); setActionError('')
    try {
      if (dialog === 'mkdir') await request('mkdir', { rel_path: join(path, name.trim()) })
      if (dialog === 'rename') await request('rename', { rel_path: join(path, selected[0]), to: join(path, name.trim()) })
      if (dialog === 'trash') {
        const results: { name: string; error?: string; trashPath?: string }[] = []
        for (const item of selectedEntries) {
          try {
            const result = await request<{ trashPath: string }>('trash', { rel_path: join(path, item.name) })
            results.push({ name: item.name, trashPath: result.trashPath })
          } catch(e) { results.push({ name: item.name, error: String(e) }) }
          setTrashResults([...results])
        }
        const failed = results.filter(r => r.error)
        setSelected(failed.map(r => r.name))
        toast(failed.length ? 'info' : 'success', `${results.length - failed.length} moved to trash; ${failed.length} failed.`)
        if (failed.length) setActionError('Some items could not be moved. Review the results below; retry applies only to failed items.')
        return
      }
      toast('success', dialog === 'mkdir' ? 'Folder created.' : 'Renamed.')
      setDialog(null); setSelected([])
    } catch(e) { setActionError(String(e)) }
    finally { actionBusy.current = false; setBusy(false); restart() }
  }
  const pick = async (folder: boolean) => {
    try { const paths = folder ? [await api.pickDirectory()].filter((p): p is string => !!p) : await api.pickFiles(); await upload(paths) }
    catch(e) { toast('error', String(e)) }
  }
  const download = async () => {
    if (actionBusy.current) return
    actionBusy.current = true; setBusy(true)
    try {
      const result = await request<{ transferId: string | null; skipped?: string[] }>('download', { paths: selected.map(n => join(path, n)) })
      if (result.skipped?.length) { setError(`Skipped ${result.skipped.length} unsupported item(s): ${result.skipped.slice(0, 10).join('; ')}`) }
      if (result.transferId) toast('success', 'Download started. Follow progress in Send & Receive.')
      else toast('info', 'No transferable items in the selection.')
    }
    catch(e) { toast('error', String(e)) }
    finally { actionBusy.current = false; setBusy(false) }
  }
  const openDialog = (action: Action) => { setDialog(action); setName(action === 'rename' ? selected[0] : ''); setActionError(''); setTrashResults([]) }
  const validName = name.trim() && name.trim() !== '.' && name.trim() !== '..' && !/[\/\0]/.test(name) && !name.toLowerCase().startsWith('.dropbeam-')
  const parts = path.split('/').filter(Boolean)
  const toggle = (item: LocationEntry) => setSelected(s => s.includes(item.name) ? s.filter(n => n !== item.name) : [...s, item.name])
  return <section className={`card file-browser${hover && location.rights.upload ? ' location-drag' : ''}`} aria-label={`${location.name} file browser`}>
    <nav className="location-breadcrumbs" aria-label="Folder path"><button disabled={busy} onClick={() => navigate('')}><Home size={15} />{location.name}</button>{parts.map((part, i) => <span key={i}><ChevronRight size={14} /><button disabled={busy} onClick={() => navigate(parts.slice(0,i+1).join('/'))}>{part}</button></span>)}</nav>
    <div className="location-toolbar location-actions">
      <button className="btn btn-primary" disabled={!selected.length || busy || loading || !!error} onClick={() => { void download() }}><ArrowDownToLine size={15} />Download{selected.length > 0 ? ` (${selected.length})` : ''}</button>
      {location.rights.upload && <><button className="btn btn-ghost" disabled={busy || loading || !!error} onClick={() => { void pick(false) }}><ArrowUpFromLine size={15} />Upload files</button><button className="btn btn-ghost" disabled={busy || loading || !!error} onClick={() => { void pick(true) }}><Folder size={15} />Upload folder</button></>}
      {location.rights.manage && <><button className="btn btn-ghost" disabled={busy || loading || !!error} onClick={() => openDialog('mkdir')}><FolderPlus size={15} />New folder</button><button className="icon-btn" title="Rename selected item" aria-label="Rename selected item" disabled={selected.length !== 1 || busy || loading} onClick={() => openDialog('rename')}><Pencil size={16} /></button><button className="icon-btn" title="Move selected items to trash" aria-label="Move selected items to trash" disabled={!selected.length || busy || loading} onClick={() => openDialog('trash')}><Trash2 size={16} /></button></>}
      <button className="icon-btn" title="Refresh folder" aria-label="Refresh folder" disabled={loading || busy} onClick={restart}><RefreshCw size={16} /></button>
    </div>
    <div className="location-toolbar location-filter"><label><Search size={16} /><input aria-label="Filter folder" placeholder="Filter folder…" value={query} onChange={e => (() => { cursors.current = [undefined]; setPage(0); setQuery(e.target.value) })()} /></label><select aria-label="Sort files" value={sort} onChange={e => (() => { cursors.current = [undefined]; setPage(0); setSort(e.target.value) })()}><option value="name">Name · A–Z</option><option value="size">Size · Largest first</option><option value="modified">Modified · Newest first</option></select></div>
    {busy && <p className="location-status" role="status">Preparing… Large downloads are safely staged on the host first.</p>}
    {error && <div className="location-error" role="alert">{error}<br/><button className="btn btn-ghost" disabled={busy || loading} onClick={restart}>Try again</button></div>}
    <div className="location-table-wrap" aria-busy={loading}>
      <table className="location-table"><thead><tr><th><input type="checkbox" aria-label="Select all visible items" disabled={!entries.length || busy || loading} checked={entries.length > 0 && entries.every(e => selected.includes(e.name))} onChange={e => setSelected(e.target.checked ? entries.map(e => e.name) : [])} /></th><th>Name</th><th>Size</th><th>Modified</th></tr></thead><tbody>
        {entries.map(entry => <tr key={entry.name} className={selected.includes(entry.name) ? 'selected' : ''} onDoubleClick={() => entry.isDir && navigate(join(path, entry.name))}>
          <td><input type="checkbox" aria-label={`Select ${entry.name}`} disabled={busy || loading} checked={selected.includes(entry.name)} onChange={() => toggle(entry)} /></td>
          <td><button className="location-filename" disabled={busy || loading} onClick={() => entry.isDir ? navigate(join(path, entry.name)) : toggle(entry)}>{entry.isDir ? <Folder size={20} /> : <File size={19} />}<span>{entry.name}</span>{entry.isDir && <ChevronRight size={14} />}</button></td>
          <td>{entry.isDir ? '—' : formatBytes(entry.size)}</td><td>{entry.modified ? new Date(entry.modified).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '—'}</td></tr>)}
      </tbody></table>
      {loading && <p className="location-empty-small" role="status">Loading folder…</p>}
      {!loading && !error && !entries.length && <div className="location-empty-small"><FolderOpenIllustration /><h3>{query ? 'No matching items' : 'This folder is empty'}</h3><p>{query ? 'Try another name in this folder.' : location.rights.upload ? 'Upload files or drop them here.' : 'Files shared here will appear in this folder.'}</p></div>}
    </div>
    <footer className="location-browser-footer"><span>{online} · {data.total ?? data.entries.length} items · Page {page+1}{location.rights.upload ? ' · Drop files here to upload' : ' · Read only'}</span><div className="location-toolbar"><button className="btn btn-ghost" disabled={page === 0 || loading || busy} onClick={() => { setPage(p => p-1); setSelected([]) }}>Previous</button><button className="btn btn-ghost" disabled={!data.hasMore || loading || busy} onClick={() => { setPage(p => p+1); setSelected([]) }}>Next</button></div></footer>
    {dialog && <div className="location-modal"><form className="card dialog" role="dialog" aria-modal="true" aria-labelledby="location-action-title" onSubmit={e => { e.preventDefault(); void run() }} onKeyDown={e => {
        if (e.key === 'Escape' && !busy) setDialog(null)
        if (e.key === 'Tab') {
          const controls = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)'))
          const first = controls[0], last = controls[controls.length - 1]
          if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus() }
          if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus() }
        }
      }}>
      <div className="location-heading"><h2 id="location-action-title">{dialog === 'trash' ? 'Move to trash?' : dialog === 'rename' ? 'Rename item' : 'New folder'}</h2><button type="button" className="icon-btn" aria-label="Close dialog" disabled={busy} onClick={() => setDialog(null)}><X size={18} /></button></div>
      {dialog === 'trash' ? <><ul className="location-trash-items">{selectedEntries.map(e => <li key={e.name}>{e.name}</li>)}</ul><p>These items will move to <strong>{location.name}/.dropbeam-trash/&lt;timestamp&gt;/</strong>. They are not permanently deleted. The owner can restore them from that folder.</p></> : <label>{dialog === 'rename' ? `New name for “${selected[0]}”` : 'Folder name'}<input autoFocus required maxLength={200} value={name} onChange={e => setName(e.target.value)} /></label>}
      {actionError && <p className="location-error" role="alert">{actionError}</p>}
      {!!trashResults.length && <ul className="location-trash-items" aria-live="polite">{trashResults.map(r => <li key={r.name}><strong>{r.name}:</strong> {r.error ? `Failed — ${r.error}` : `Moved to ${r.trashPath}`}</li>)}</ul>}
      <div className="dialog-actions location-toolbar"><button type="button" autoFocus={dialog === 'trash'} className="btn btn-ghost" disabled={busy} onClick={() => setDialog(null)}>Cancel</button><button className="btn btn-primary" disabled={busy || (dialog !== 'trash' && !validName) || (dialog === 'trash' && !selectedEntries.length)}>{busy ? 'Working…' : dialog === 'trash' ? 'Move to trash' : dialog === 'mkdir' ? 'Create folder' : 'Rename'}</button></div>
    </form></div>}
  </section>
}
function FolderOpenIllustration() { return <Folder size={38} strokeWidth={1.2} style={{ color: 'var(--accent)', margin: '8px auto' }} /> }
