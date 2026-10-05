import { useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { isActive } from '../lib/api'
import { useStore } from '../store'
import { Dialog } from './Dialog'

/** "Install and Restart" — but never silently cuts a transfer in half: with
 *  anything moving it asks first and says what happens to it. */
export function InstallUpdateButton({ className = 'btn btn-primary', label = 'Install and Restart' }: { className?: string; label?: string }) {
  const installUpdate = useStore((s) => s.installUpdate)
  const busy = useStore((s) => Object.values(s.transfers).filter((t) => isActive(t.state)).length)
  const syncing = useStore((s) => Object.values(s.folderStatuses).some((f) => f.state === 'sending' || f.state === 'receiving'))
  const [asking, setAsking] = useState(false)
  const go = () => { setAsking(false); void installUpdate() }
  return (
    <>
      <button className={className} onClick={() => (busy > 0 || syncing ? setAsking(true) : go())}>{label}</button>
      <AnimatePresence>
        {asking && (
          <Dialog
            title="Restart now?"
            width={380}
            onClose={() => setAsking(false)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setAsking(false)}>Wait</button>
                <button className="btn btn-primary" onClick={go}>Restart Now</button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>
              {busy > 0
                ? `${busy === 1 ? 'A transfer is' : `${busy} transfers are`} still going. Restarting stops ${busy === 1 ? 'it' : 'them'}; big files pick up where they left off, others may need sending again.`
                : 'A shared folder is still syncing. It carries on after the restart.'}{' '}
              You can also update later from Settings.
            </p>
          </Dialog>
        )}
      </AnimatePresence>
    </>
  )
}
