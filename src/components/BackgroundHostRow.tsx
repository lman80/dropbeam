import { useEffect, useState } from 'react'
import { backgroundHostApi, LINUX_BACKGROUND_COMMAND, type BackgroundHostStatus } from '../lib/backgroundHost'
import { errorText } from '../lib/errors'
import { IS_LINUX, IS_WINDOWS } from '../lib/platform'
import { useStore } from '../store'
import { Spinner, Toggle } from './ui'

const TITLE = 'Keep running when no one is signed in'
const SUB_WINDOWS = 'After a restart DropBeam keeps sharing your Locations and holding messages, even before anyone signs in. The app takes over while it’s open. Windows will ask for permission.'

/**
 * One settings row, shared by Transfer Server → Advanced and the hosted
 * Locations list: a switch on Windows, the systemd command on Linux, nothing
 * on macOS. `className` is the row class of the list it sits in.
 */
export function BackgroundHostRow({ className = 'row set-row' }: { className?: string }) {
  const toast = useStore((s) => s.toast)
  const [status, setStatus] = useState<BackgroundHostStatus | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    if (!IS_WINDOWS) return
    let alive = true
    backgroundHostApi.status().then((s) => { if (alive) setStatus(s) }).catch(() => {})
    return () => { alive = false }
  }, [])

  if (IS_LINUX) {
    return (
      <div className={className}>
        <div className="row-main">
          <div className="row-title">{TITLE}</div>
          <div className="row-sub">Runs the server in the background after a restart. The app takes over whenever it’s open.</div>
        </div>
        <div className="row-trailing">
          <button className="btn btn-secondary btn-sm" onClick={() => {
            void navigator.clipboard.writeText(LINUX_BACKGROUND_COMMAND)
              .then(() => toast('success', 'Copied — paste it into Terminal'))
              .catch(() => toast('error', 'Couldn’t copy'))
          }}>Copy Command</button>
        </div>
      </div>
    )
  }
  if (!IS_WINDOWS || (status && !status.supported)) return null

  const set = async (on: boolean) => {
    setBusy(true)
    try {
      const next = await backgroundHostApi.set(on)
      setStatus(next)
      if (on && next.enabled) toast('success', 'DropBeam will keep running when no one is signed in')
    } catch (e) {
      toast('error', errorText(e))
    } finally {
      setBusy(false)
    }
  }
  const moved = !!status?.enabled && !status.current
  return (
    <div className={className}>
      <div className="row-main">
        <div className="row-title">{TITLE}</div>
        <div className="row-sub">
          {moved ? 'It still starts an older copy of DropBeam. Turn it off and on again to fix it.' : SUB_WINDOWS}
        </div>
      </div>
      <div className="row-trailing">
        {busy && <Spinner />}
        <Toggle label={TITLE} on={!!status?.enabled} disabled={!status || busy} onChange={(v) => void set(v)} />
      </div>
    </div>
  )
}
