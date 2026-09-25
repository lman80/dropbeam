import { Fragment, useCallback, useEffect, useRef, useState } from 'react'
import { ArrowDownToLine, ArrowUpFromLine, ChevronDown, ChevronLeft, ChevronRight, File, Folder, FolderPlus, Pencil, Plus, RefreshCw, Search, Trash2, X } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'
import { api, locationsApi, onLocationsChanged, onTransferUpdate, type LocationEntry, type LocationPage, type SharedLocation } from '../lib/api'
import { formatBytes } from '../lib/format'
import { rememberLocationUpload, useStore } from '../store'
import { Dialog } from './Dialog'
import { IconButton, MenuPopover, Spinner } from './ui'

type Action = 'mkdir' | 'rename' | 'trash'
const join = (parent: string, name: string) => parent ? `${parent}/${name}` : name
/** Browse a friend's Location. `host` is the friend's display name, `online` their presence in words. */
export function FileBrowser({ friendId, location, online, host, onBack }: { friendId: string; location: SharedLocation; online: string; host?: string; onBack?: () => void }) {
  const toast = useStore(s => s.toast)
  const [selecting, setSelecting] = useState(false)
  const [actionsOpen, setActionsOpen] = useState(false)
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
    if (!location.rights.upload) { toast('error', 'This location is view only.'); return }
    if (actionBusy.current) return
    actionBusy.current = true; setBusy(true)
    try {
      const u = await locationsApi.upload(friendId, location.id, path, paths)
      rememberLocationUpload(u.id, friendId, location.id, path, paths)
      if (!useStore.getState().transfers[u.id]) useStore.getState().upsertTransfer(u)
      toast('success', 'Uploading · follow it in Send & Receive')
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
        const moved = results.length - failed.length
        setSelected(failed.map(r => r.name))
        toast(failed.length ? 'info' : 'success', failed.length
          ? `Moved ${moved} of ${results.length} to Trash · ${failed.length} couldn’t be moved`
          : `Moved ${moved === 1 ? `“${results[0].name}”` : `${moved} items`} to Trash`)
        if (failed.length) setActionError(`${failed.length === 1 ? 'This item' : 'These items'} couldn’t be moved. Try again to retry just ${failed.length === 1 ? 'it' : 'them'}.`)
        else setDialog(null)
        return
      }
      toast('success', dialog === 'mkdir' ? 'Folder created' : 'Renamed')
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
      if (result.skipped?.length) toast('info', `Skipped ${result.skipped.length === 1 ? '1 item' : `${result.skipped.length} items`} that can’t be downloaded: ${result.skipped.slice(0, 3).map(p => p.split('/').pop()).join(', ')}${result.skipped.length > 3 ? '…' : ''}`)
      if (result.transferId) toast('success', 'Downloading · follow it in Send & Receive')
      else toast('info', 'Nothing in the selection can be downloaded')
    }
    catch(e) { toast('error', String(e)) }
    finally { actionBusy.current = false; setBusy(false) }
  }
  const openDialog = (action: Action) => { setDialog(action); setName(action === 'rename' ? selected[0] : ''); setActionError(''); setTrashResults([]) }
  const validName = name.trim() && name.trim() !== '.' && name.trim() !== '..' && !/[/\0]/.test(name) && !name.toLowerCase().startsWith('.dropbeam-')
  const parts = path.split('/').filter(Boolean)
  const toggle = (item: LocationEntry) => setSelected(s => s.includes(item.name) ? s.filter(n => n !== item.name) : [...s, item.name])
  const trashCount = selectedEntries.length
  const trashTitle = trashCount === 1 ? `Move “${selectedEntries[0].name}” to Trash?` : `Move ${trashCount} items to Trash?`
  const failures = trashResults.filter(r => r.error)
  const actionSheet = dialog && <Dialog width={400} busy={busy} onClose={() => setDialog(null)} className="location-dialog"
    title={dialog === 'trash' ? (trashCount ? trashTitle : 'Move to Trash') : dialog === 'rename' ? 'Rename' : 'New folder'}
    footer={<>
      <button type="button" className="btn btn-secondary" disabled={busy} autoFocus={dialog === 'trash'} onClick={() => setDialog(null)}>{failures.length ? 'Close' : 'Cancel'}</button>
      <button type="submit" form="location-action-form" className={dialog === 'trash' ? 'btn btn-destructive' : 'btn btn-primary'}
        disabled={busy || (dialog !== 'trash' && !validName) || (dialog === 'trash' && !trashCount)}>
        {busy ? 'Working…' : dialog === 'trash' ? (failures.length ? 'Try again' : 'Move to Trash') : dialog === 'mkdir' ? 'Create' : 'Rename'}</button>
    </>}>
    <form id="location-action-form" onSubmit={e => { e.preventDefault(); void run() }}>
      {dialog === 'trash'
        ? <>
          <p className="dialog-text">{trashCount === 1 ? 'It moves' : 'They move'} to this location’s Trash folder, where the owner can restore {trashCount === 1 ? 'it' : 'them'}.</p>
          {trashCount > 1 && <ul className="location-name-list">
            {selectedEntries.slice(0, 5).map(e => <li key={e.name} className="truncate-1" title={e.name}>{e.name}</li>)}
            {trashCount > 5 && <li className="faint">and {trashCount - 5} more</li>}
          </ul>}
        </>
        : <label className="location-field">
          <span className="field-label">{dialog === 'rename' ? 'New name' : 'Name'}</span>
          <input className="input" autoFocus required maxLength={200} value={name} placeholder={dialog === 'mkdir' ? 'Untitled folder' : undefined}
            onFocus={e => { if (dialog === 'rename') { const dot = e.target.value.lastIndexOf('.'); e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length) } }}
            onChange={e => setName(e.target.value)} />
        </label>}
      {actionError && <p className="form-error" role="alert">{actionError}</p>}
      {failures.length > 0 && <ul className="location-name-list" aria-live="polite">
        {failures.map(r => <li key={r.name}><span className="truncate-1" title={r.name}>{r.name}</span><span className="form-error">{r.error}</span></li>)}
      </ul>}
    </form>
  </Dialog>
  if (MOBILE_UI) return <section className="mobile-browser" aria-label={`${location.name} file browser`}>
    <header className="mobile-header-compact visible mobile-browser-header"><button className="ios-button mobile-back" aria-label="Back" disabled={busy} onClick={() => path ? navigate(parts.slice(0, -1).join('/')) : onBack?.()}><ChevronLeft />Back</button><h1 className="ios-headline mobile-grow mobile-ellipsis">{parts.at(-1) ?? location.name}</h1><button className="ios-button" onClick={() => { setSelecting(!selecting); setSelected([]) }}>{selecting ? 'Done' : 'Select'}</button><button className="ios-icon" aria-label="Folder actions" aria-expanded={actionsOpen} onClick={() => setActionsOpen(!actionsOpen)}><Plus /></button></header>
    <div className="mobile-inset"><input className="mobile-search" aria-label="Search folder" placeholder="Search folder" value={query} onChange={e => { cursors.current = [undefined]; setPage(0); setQuery(e.target.value) }} /></div>
    {error && <div className="mobile-inset ios-footnote" role="alert">{error}<button className="ios-button" onClick={restart}>Try again</button></div>}
    {busy && <p className="mobile-inset ios-footnote" role="status">Preparing…</p>}
    <div className="ios-list" aria-busy={loading}>{entries.map(entry => <div className="ios-row" key={entry.name}><button className="mobile-entry" disabled={busy || loading} onClick={() => selecting || !entry.isDir ? (setSelecting(true), toggle(entry)) : navigate(join(path, entry.name))}><span className="mobile-tinted-icon">{entry.isDir ? <Folder /> : <File />}</span><span className="mobile-grow"><span className="ios-headline mobile-ellipsis">{entry.name}</span><span className="ios-footnote">{entry.isDir ? 'Folder' : formatBytes(entry.size)} · {entry.modified ? new Date(entry.modified).toLocaleDateString() : '—'}</span></span></button>{selecting ? <input type="checkbox" aria-label={`Select ${entry.name}`} disabled={busy || loading} checked={selected.includes(entry.name)} onChange={() => toggle(entry)} /> : entry.isDir && <ChevronRight size={18} />}</div>)}</div>
    {loading && <p className="mobile-inset ios-footnote" role="status">Loading folder…</p>}
    {!loading && !error && !entries.length && <div className="mobile-empty"><Folder /><h2 className="ios-title2">{query ? 'No matching items' : 'This folder is empty'}</h2><p className="ios-footnote">{query ? 'Try another file name.' : 'Files shared here appear in this folder.'}</p><button className="ios-button ios-primary" onClick={() => query ? setQuery('') : setActionsOpen(true)}>{query ? 'Clear search' : 'Folder actions'}</button></div>}
    <footer className="mobile-inset mobile-browser-footer"><p className="ios-footnote">{online} · {data.total ?? entries.length} items · Page {page + 1}</p><div className="ios-row"><button className="ios-button" disabled={!page || loading || busy} onClick={() => { setPage(p => p - 1); setSelected([]) }}>Previous</button><button className="ios-button" disabled={!data.hasMore || loading || busy} onClick={() => { setPage(p => p + 1); setSelected([]) }}>Next</button></div></footer>
    {(selected.length > 0 || actionsOpen) && <div className="mobile-browser-actions"><button className="ios-button" disabled={!selected.length || busy || loading || !!error} onClick={() => void download()}><ArrowDownToLine size={18} />Download</button>{location.rights.upload && <><button className="ios-button" disabled={busy || loading || !!error} onClick={() => void pick(false)}>Upload files</button><button className="ios-button" disabled={busy || loading || !!error} onClick={() => void pick(true)}>Upload folder</button></>}{location.rights.manage && <><button className="ios-button" disabled={busy || loading || !!error} onClick={() => openDialog('mkdir')}>New folder</button><button className="ios-button" disabled={selected.length !== 1 || busy || loading} onClick={() => openDialog('rename')}>Rename</button><button className="ios-button ios-destructive" disabled={!selected.length || busy || loading} onClick={() => openDialog('trash')}>Trash</button></>}<button className="ios-button" disabled={busy} onClick={() => { setActionsOpen(false); setSelected([]); setSelecting(false) }}>Done</button></div>}
    {actionSheet}
  </section>
  const canAct = !busy && !loading && !error
  const pageable = page > 0 || data.hasMore
  const here = parts.at(-1) ?? location.name
  return <section className="location-browser" aria-label={`${location.name} file browser`}>
    <div className="page-header titlebar-drag location-browser-head">
      <div className="location-crumbs">
        <IconButton label="All locations" onClick={onBack} disabled={!onBack}><ChevronLeft /></IconButton>
        <nav aria-label="Folder path" className="location-breadcrumbs">
          <h1 className="page-title location-crumb-list">
            {[location.name, ...parts].map((part, i, all) => {
              const last = i === all.length - 1
              return <Fragment key={i}>
                {i > 0 && <ChevronRight className="location-crumb-sep" aria-hidden />}
                {last
                  ? <span className="location-crumb current truncate-1" title={part} aria-current="page">{part}</span>
                  : <button type="button" className="location-crumb truncate-1" title={part} disabled={busy}
                    onClick={() => navigate(parts.slice(0, i).join('/'))}>{part}</button>}
              </Fragment>
            })}
          </h1>
        </nav>
      </div>
      <div className="page-actions">
        <IconButton label="Refresh" disabled={loading || busy} onClick={restart}><RefreshCw /></IconButton>
        {location.rights.manage && <IconButton label="New folder" disabled={!canAct} onClick={() => openDialog('mkdir')}><FolderPlus /></IconButton>}
        {location.rights.upload && <UploadButton disabled={!canAct} onPick={folder => void pick(folder)} />}
      </div>
    </div>

    <div className="location-toolbar-row">
      <label className="search-field location-search">
        <Search aria-hidden />
        <input className="input" type="search" aria-label={`Filter ${here}`} placeholder="Filter" value={query}
          onChange={e => { cursors.current = [undefined]; setPage(0); setQuery(e.target.value) }} />
      </label>
      <select className="input location-sort" aria-label="Sort by" value={sort}
        onChange={e => { cursors.current = [undefined]; setPage(0); setSort(e.target.value) }}>
        <option value="name">Name</option><option value="size">Size</option><option value="modified">Date modified</option>
      </select>
      <div className="location-selection" aria-live="polite">{selected.length > 0 && <>
        <span className="muted tnum truncate-1">{selected.length} selected</span>
        <button type="button" className="btn btn-secondary btn-sm" disabled={!canAct} onClick={() => { void download() }}><ArrowDownToLine />Download</button>
        {location.rights.manage && <>
          <IconButton size="sm" label="Rename" disabled={selected.length !== 1 || busy || loading} onClick={() => openDialog('rename')}><Pencil /></IconButton>
          <IconButton size="sm" label="Move to Trash" danger disabled={busy || loading} onClick={() => openDialog('trash')}><Trash2 /></IconButton>
        </>}
        <IconButton size="sm" label="Clear selection" onClick={() => setSelected([])}><X /></IconButton>
      </>}</div>
    </div>

    <div className={`group location-table-group${hover && location.rights.upload ? ' location-drop' : ''}${loading && entries.length ? ' location-loading' : ''}`} aria-busy={loading}>
      <table className="location-table">
        <thead><tr>
          <th className="location-col-check"><input type="checkbox" aria-label="Select all" disabled={!entries.length || busy || loading}
            checked={entries.length > 0 && entries.every(e => selected.includes(e.name))}
            onChange={e => setSelected(e.target.checked ? entries.map(e => e.name) : [])} /></th>
          <th>Name</th><th className="location-col-size">Size</th><th className="location-col-date">Date modified</th>
        </tr></thead>
        <tbody>{entries.map(entry => {
          const on = selected.includes(entry.name)
          return <tr key={entry.name} className={on ? 'selected' : ''} aria-selected={on}
            onClick={e => { if ((e.target as HTMLElement).closest('input,button')) return; toggle(entry) }}
            onDoubleClick={() => entry.isDir && navigate(join(path, entry.name))}>
            <td className="location-col-check"><input type="checkbox" aria-label={`Select ${entry.name}`} disabled={busy || loading} checked={on} onChange={() => toggle(entry)} /></td>
            <td className="location-col-name">
              {entry.isDir
                ? <button type="button" className="location-filename" disabled={busy || loading} onClick={() => navigate(join(path, entry.name))}>
                  <Folder className="location-icon-folder" aria-hidden /><span className="truncate-1" title={entry.name}>{entry.name}</span></button>
                : <span className="location-filename"><File className="location-icon-file" aria-hidden /><span className="truncate-1" title={entry.name}>{entry.name}</span></span>}
            </td>
            <td className="location-col-size tnum">{entry.isDir ? '—' : formatBytes(entry.size)}</td>
            <td className="location-col-date tnum">{entry.modified ? new Date(entry.modified).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '—'}</td>
          </tr>
        })}</tbody>
      </table>
      {error
        ? <div className="location-table-note" role="alert"><span className="truncate-1" title={error}>Couldn’t open this folder</span>
          <button type="button" className="btn btn-plain btn-sm" disabled={busy || loading} onClick={restart}>Try again</button></div>
        : loading && !entries.length
          ? <div className="location-table-note" role="status"><Spinner /> Loading…</div>
          : !entries.length && <div className="location-table-note">{query ? 'No matches' : location.rights.upload ? 'Empty folder · drop files here to upload' : 'Empty folder'}</div>}
    </div>

    <footer className="location-browser-foot">
      <span className="truncate-1 muted" role="status">
        {busy
          ? <><Spinner size={11} /> Preparing…</>
          : hover && location.rights.upload
            ? <>Drop to upload to {here}</>
            : <span className="truncate-1">{host ? `${host} · ` : ''}{online} · <span className="tnum">{data.total ?? entries.length} item{(data.total ?? entries.length) === 1 ? '' : 's'}</span>{location.rights.upload ? '' : ' · View only'}</span>}
      </span>
      {pageable && <div className="location-pager">
        <span className="faint tnum">Page {page + 1}</span>
        <IconButton size="sm" label="Previous page" disabled={page === 0 || loading || busy} onClick={() => { setPage(p => p - 1); setSelected([]) }}><ChevronLeft /></IconButton>
        <IconButton size="sm" label="Next page" disabled={!data.hasMore || loading || busy} onClick={() => { setPage(p => p + 1); setSelected([]) }}><ChevronRight /></IconButton>
      </div>}
    </footer>
    {actionSheet}
  </section>
}

/** "Upload" with a small menu: files, or a whole folder. */
function UploadButton({ disabled, onPick }: { disabled: boolean; onPick: (folder: boolean) => void }) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null)
  const [trigger, setTrigger] = useState<HTMLElement | null>(null)
  const close = useCallback(() => setAnchor(null), [])
  return <>
    <button type="button" className="btn btn-primary location-upload" disabled={disabled} aria-haspopup="menu" aria-expanded={!!anchor}
      onClick={e => { setTrigger(e.currentTarget); setAnchor(anchor ? null : e.currentTarget.getBoundingClientRect()) }}>
      <ArrowUpFromLine />Upload<ChevronDown className="location-upload-caret" />
    </button>
    {anchor && <MenuPopover anchor={anchor} trigger={trigger} onClose={close} items={[
      { label: 'Files…', icon: <File />, onSelect: () => onPick(false) },
      { label: 'Folder…', icon: <Folder />, onSelect: () => onPick(true) },
    ]} />}
  </>
}
