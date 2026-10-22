import { useEffect, useRef, useState } from 'react'
import { ArrowDownToLine, ExternalLink, FolderOpen } from 'lucide-react'
import { Dialog } from './Dialog'
import { Spinner } from './ui'
import { errorText } from '../lib/errors'
import { formatBytes } from '../lib/format'
import { IS_MAC, IS_WINDOWS } from '../lib/platform'
import { driveApi, pathLabel, streamApi, type ReadUrl, type StreamStatus } from '../lib/streamApi'

const DRIVE_LABEL = IS_MAC ? 'Show in Finder' : IS_WINDOWS ? 'Show in File Explorer' : 'Show in Files'
const KINDS = new Set(['image', 'video', 'audio', 'pdf', 'text'])

/** Open a Location file in place: it streams from the friend's device as it
 *  plays or scrolls, nothing is copied first. Shows which path it uses and how
 *  fast it's coming in. */
export function FilePreview({ friendId, locationId, relPath, name, onClose, onDownload, toast }: {
  friendId: string; locationId: string; relPath: string; name: string
  onClose: () => void; onDownload: () => void; toast: (kind: 'error' | 'info' | 'success', text: string) => void
}) {
  const [read, setRead] = useState<ReadUrl | null>(null)
  const [error, setError] = useState('')
  const [text, setText] = useState<string | null>(null)
  const [status, setStatus] = useState<StreamStatus | null>(null)
  const [opening, setOpening] = useState(false)
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    streamApi.readUrl(friendId, locationId, relPath).then(r => { if (alive.current) setRead(r) }).catch(e => { if (alive.current) setError(errorText(e)) })
    const tick = () => streamApi.status(friendId).then(s => { if (alive.current) setStatus(s) }).catch(() => {})
    void tick()
    const timer = window.setInterval(() => void tick(), 1000)
    return () => { alive.current = false; window.clearInterval(timer) }
  }, [friendId, locationId, relPath])
  useEffect(() => {
    if (read?.kind !== 'text') return
    // The first 256 KB is plenty to read; the rest streams on "Open".
    fetch(read.url, { headers: { Range: 'bytes=0-262143' } }).then(r => r.text()).then(t => { if (alive.current) setText(t) })
      .catch(e => { if (alive.current) setError(errorText(e)) })
  }, [read])
  const openWith = async () => {
    setOpening(true)
    try { await streamApi.open(friendId, locationId, relPath) } catch (e) { toast('error', errorText(e)) } finally { if (alive.current) setOpening(false) }
  }
  const reveal = async () => { try { await driveApi.open(friendId, locationId, relPath) } catch (e) { toast('error', errorText(e)) } }
  const badge = status && <span className={`stream-badge stream-${status.path}`} title={status.rttMs != null ? `${status.rttMs} ms round trip` : undefined}>
    {pathLabel(status.path)}{status.bytesPerSec > 0 ? ` · ${formatBytes(status.bytesPerSec)}/s` : ''}
  </span>
  const body = error
    ? <p className="form-error" role="alert">{error}</p>
    : !read
      ? <div className="stream-loading" role="status"><Spinner /> Opening…</div>
      : read.kind === 'image' ? <img className="stream-media" src={read.url} alt={name} />
      : read.kind === 'video' ? <video className="stream-media" src={read.url} controls autoPlay playsInline preload="metadata" />
      : read.kind === 'audio' ? <audio className="stream-audio" src={read.url} controls autoPlay preload="metadata" />
      : read.kind === 'pdf' ? <iframe className="stream-pdf" src={read.url} title={name} />
      : read.kind === 'text' ? (text === null ? <div className="stream-loading"><Spinner /> Opening…</div> : <pre className="stream-text">{text}{read.size > 262144 ? '\n…' : ''}</pre>)
      : <p className="dialog-text">DropBeam can’t show this kind of file here. Open it with an app on this computer, or download a copy.</p>
  return <Dialog width={read && KINDS.has(read.kind) && read.kind !== 'audio' ? 880 : 460} onClose={onClose} className="stream-preview"
    title={name} subtitle={<span className="stream-sub">{read ? formatBytes(read.size) : ''}{badge}</span>}
    footer={<>
      <button type="button" className="btn btn-secondary" onClick={() => void reveal()}><FolderOpen />{DRIVE_LABEL}</button>
      <button type="button" className="btn btn-secondary" onClick={onDownload}><ArrowDownToLine />Download</button>
      <button type="button" className="btn btn-primary" disabled={opening} onClick={() => void openWith()}>{opening ? <Spinner size={12} /> : <ExternalLink />}Open with Default App</button>
    </>}>
    <div className="stream-body">{body}</div>
  </Dialog>
}

/** A small picture for image rows, made on the friend's device and cached
 *  here. Asked for only when the row scrolls into view, a few at a time. */
const queue: (() => Promise<void>)[] = []
let running = 0
function schedule(job: () => Promise<void>) {
  queue.push(job)
  const pump = () => {
    while (running < 3 && queue.length) {
      const next = queue.shift()!
      running++
      void next().finally(() => { running--; pump() })
    }
  }
  pump()
}
export function Thumb({ friendId, locationId, relPath, size, modified, fallback }: {
  friendId: string; locationId: string; relPath: string; size: number; modified: number; fallback: React.ReactNode
}) {
  const [src, setSrc] = useState<string | null>(null)
  const ref = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    let alive = true
    const el = ref.current
    if (!el) return
    const seen = new IntersectionObserver(items => {
      if (!items.some(i => i.isIntersecting)) return
      seen.disconnect()
      schedule(() => streamApi.thumb(friendId, locationId, relPath, size, modified).then(s => { if (alive && s) setSrc(s) }).catch(() => {}))
    })
    seen.observe(el)
    return () => { alive = false; seen.disconnect() }
  }, [friendId, locationId, relPath, size, modified])
  return <span ref={ref} className="location-thumb">{src ? <img src={src} alt="" /> : fallback}</span>
}
