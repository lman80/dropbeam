import { useEffect, useState } from 'react'
import { Gauge } from 'lucide-react'
import { Dialog } from './Dialog'
import { ProgressBar, Spinner } from './ui'
import { api, onSafeSpeedProgress, type SafeSpeedProgress, type SafeSpeedReport } from '../lib/api'
import { applySafeSpeed, safeSpeedResult, safeSpeedStep, STEPS_MBPS } from '../lib/safeSpeed'
import { useStore } from '../store'

/** "Find my safe speed" (#20): explains, runs the test while showing what it
 *  does, then offers to use the recommended limit. */
export function SafeSpeedDialog({ onClose }: { onClose: () => void }) {
  const save = useStore((s) => s.saveSettings)
  const toast = useStore((s) => s.toast)
  const [phase, setPhase] = useState<'intro' | 'running' | 'done' | 'failed'>('intro')
  const [progress, setProgress] = useState<SafeSpeedProgress | null>(null)
  const [report, setReport] = useState<SafeSpeedReport | null>(null)
  const [error, setError] = useState('')

  useEffect(() => {
    const un = onSafeSpeedProgress(setProgress)
    return () => { void un.then((f) => f()) }
  }, [])

  const start = async () => {
    setPhase('running'); setProgress(null); setError('')
    try {
      setReport(await api.findSafeSpeed())
      setPhase('done')
    } catch (e) {
      setError(String(e))
      setPhase('failed')
    }
  }
  const cancel = () => { void api.cancelSafeSpeed(); onClose() }
  const result = report ? safeSpeedResult(report) : null
  const step = progress?.rateMbps ? STEPS_MBPS.indexOf(progress.rateMbps) + 1 : 0

  const footer = phase === 'intro' ? <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={onClose}>Cancel</button>
    <button className="btn btn-primary" onClick={() => void start()}>Start Test</button>
  </> : phase === 'running' ? <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={cancel}>Stop</button>
  </> : phase === 'done' && result && report ? <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={onClose}>Not Now</button>
    <button className="btn btn-primary" onClick={() => { void save(applySafeSpeed(report)); toast('success', result.apply ? `Speed limit set to ${result.apply} Mbps` : 'Speed limit turned off'); onClose() }}>
      {result.apply ? `Use ${result.apply} Mbps` : 'Turn Off Limit'}
    </button>
  </> : <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={onClose}>Close</button>
    <button className="btn btn-primary" onClick={() => void start()}>Try Again</button>
  </>

  return (
    <Dialog title="Find my safe speed" icon={<Gauge />} width={440} onClose={phase === 'running' ? cancel : onClose} footer={footer}>
      {phase === 'intro' && <div className="safe-speed">
        <p>Some Wi‑Fi routers stop working when files are sent too fast. This test finds the fastest speed your router handles comfortably.</p>
        <p>DropBeam sends test data to one of your devices or a friend that’s online — a little faster every few seconds — while it keeps checking that your router still answers quickly. Nothing is saved on the other device.</p>
        <p className="muted">Takes up to a minute. If your router starts to struggle, the test stops right away.</p>
      </div>}
      {phase === 'running' && <div className="safe-speed" role="status" aria-live="polite">
        <div className="safe-speed-now"><Spinner size={14} /> <span>{safeSpeedStep(progress)}</span></div>
        <ProgressBar percent={(step / STEPS_MBPS.length) * 100} label="Test progress" />
        {progress?.achievedMbps != null && <p className="muted tnum">Sent {progress.achievedMbps} Mbps · router answered in {Math.round(progress.routerMs ?? 0)} ms</p>}
      </div>}
      {phase === 'done' && result && <div className="safe-speed">
        <h3 className="safe-speed-title">{result.title}</h3>
        <p>{result.detail}</p>
        {report?.kind === 'limit' && report.local && <p className="muted">This limit will apply to sends on your home network too.</p>}
      </div>}
      {phase === 'failed' && <div className="safe-speed"><p role="alert">{error.replace(/^Error:\s*/, '')}</p></div>}
    </Dialog>
  )
}
