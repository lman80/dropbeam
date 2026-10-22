import { useCallback, useEffect, useState } from 'react'
import { FolderOpen, HardDrive } from 'lucide-react'
import { SectionHeader, Spinner, Toggle } from './ui'
import { errorText } from '../lib/errors'
import { formatBytes } from '../lib/format'
import { IS_MAC, IS_WINDOWS } from '../lib/platform'
import { useStore } from '../store'
import { driveApi, streamApi, type DriveStatus } from '../lib/streamApi'

const WHERE = IS_MAC ? 'Finder' : IS_WINDOWS ? 'File Explorer' : 'Files'

/** Settings → Drives: friends' Locations as one drive in Finder / File
 *  Explorer / Files, and the space kept for files opened in place. */
export function DriveSettings() {
  const toast = useStore(s => s.toast)
  const [status, setStatus] = useState<DriveStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [cacheGb, setCacheGb] = useState('')
  const load = useCallback(() => driveApi.status().then(s => { setStatus(s); setCacheGb(String(s.settings.cacheGb)) }).catch(e => toast('error', errorText(e))), [toast])
  useEffect(() => { void load(); const t = window.setInterval(() => { void driveApi.status().then(setStatus).catch(() => {}) }, 4000); return () => window.clearInterval(t) }, [load])
  if (!status) return <div className="group"><div className="row set-row"><Spinner /> Loading…</div></div>
  const act = async (job: () => Promise<unknown>) => {
    setBusy(true)
    try { await job() } catch (e) { toast('error', errorText(e)) } finally { setBusy(false); void load() }
  }
  const save = (patch: Partial<DriveStatus['settings']>) => act(async () => setStatus(await driveApi.setSettings({ ...status.settings, ...patch })))
  const nothing = status.locations === 0
  const state = !status.supported ? 'Not available on this device.'
    : status.busy ? 'Connecting the drive…'
    : status.mounted ? `In ${WHERE}${status.path ? ` at ${status.path}` : ''}.`
    : nothing ? 'Appears when a friend shares a Location with you.'
    : status.settings.enabled ? 'Not showing right now.' : 'Off.'
  return <>
    <SectionHeader>Drive</SectionHeader>
    <div className="group">
      <div className="row set-row">
        <div className="row-main">
          <div className="row-title">Show Locations as a drive</div>
          <div className="row-sub">Every folder friends share with you, in {WHERE} like a disk. Files open straight from their device — nothing is copied first. {state}</div>
        </div>
        <div className="row-trailing"><Toggle on={status.settings.enabled} disabled={busy || !status.supported} label="Show Locations as a drive" onChange={v => void save({ enabled: v })} /></div>
      </div>
      <div className="row set-row">
        <div className="row-main"><div className="row-title">Show it when DropBeam starts</div><div className="row-sub">Otherwise use Open Drive when you need it.</div></div>
        <div className="row-trailing"><Toggle on={status.settings.autoMount} disabled={busy || !status.settings.enabled} label="Show it when DropBeam starts" onChange={v => void save({ autoMount: v })} /></div>
      </div>
      <div className="row set-row">
        <div className="row-main">
          <div className="row-title">{status.devices === 1 ? '1 device' : `${status.devices} devices`} · {status.locations === 1 ? '1 Location' : `${status.locations} Locations`}</div>
          {status.pendingUploads > 0 && <div className="row-sub">Uploading {status.pendingUploads} saved {status.pendingUploads === 1 ? 'file' : 'files'} — they appear on the other device once checked.</div>}
          {status.uploadErrors.length > 0 && <div className="row-sub set-status-error">{status.uploadErrors[0]}</div>}
          {status.error && <div className="row-sub set-status-error" role="alert">{status.error}</div>}
        </div>
        <div className="row-trailing">
          {status.mounted
            ? <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void act(driveApi.unmount)}>Remove Drive</button>
            : null}
          <button className="btn btn-primary btn-sm" disabled={busy || !status.supported || !status.settings.enabled} onClick={() => void act(() => driveApi.open())}>
            {busy ? <Spinner size={12} /> : <FolderOpen />}Open Drive</button>
        </div>
      </div>
      {status.windowsFix && <div className="row set-row">
        <div className="row-main">
          <div className="row-title">Allow large files</div>
          <div className="row-sub">{status.windowsFix} DropBeam can fix this in one step: Windows asks for permission, then files up to 4 GB work on the drive.</div>
        </div>
        <div className="row-trailing"><button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void act(async () => setStatus(await driveApi.fixWindows()))}>Fix…</button></div>
      </div>}
      <p className="set-footnote"><HardDrive size={12} /> Saving into the drive keeps a copy here first, then uploads and checks it. Changing a file moves its old version to the Location’s trash. Making folders, renaming and deleting need “Can also rename & delete”.</p>
    </div>
    <SectionHeader>Opened files</SectionHeader>
    <div className="group">
      <div className="row set-row">
        <div className="row-main">
          <div className="row-title">Space for opened files</div>
          <div className="row-sub">Parts of files you open are kept here so they open instantly next time. Using {formatBytes(status.cachedBytes)}.</div>
        </div>
        <div className="row-trailing">
          <input className="input" style={{ width: 72 }} type="number" min={1} max={2000} aria-label="Space for opened files in GB" value={cacheGb}
            onChange={e => setCacheGb(e.target.value)}
            onBlur={() => { const gb = Math.max(1, Math.min(2000, Math.round(Number(cacheGb) || 10))); setCacheGb(String(gb)); if (gb !== status.settings.cacheGb) void save({ cacheGb: gb }) }} />
          <span className="muted">GB</span>
          <button className="btn btn-secondary btn-sm" disabled={busy || status.cachedBytes === 0} onClick={() => void act(streamApi.clearCache)}>Clear</button>
        </div>
      </div>
    </div>
  </>
}
