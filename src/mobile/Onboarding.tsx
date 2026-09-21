import { useEffect, useState } from 'react'
import { useStore } from '../store'
import { Button, Section, Sheet, TextField } from './kit'

export function MobileOnboarding() {
  const settings = useStore(s => s.settings)
  const [show, setShow] = useState(false)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => {
    if (settings && !localStorage.getItem('dropbeam.namedSelf')) { setName(settings.displayName || ''); setShow(true) }
  }, [settings])
  if (!settings || !show) return null
  const finish = async () => {
    if (busy || !name.trim()) return
    setBusy(true); setError('')
    try {
      await useStore.getState().saveSettings({ displayName: name.trim() })
      // saveSettings rolls back failed persistence; leave the sheet open in that case.
      if (useStore.getState().settings?.displayName !== name.trim()) { setError('Could not save your name. Please try again.'); return }
      localStorage.setItem('dropbeam.namedSelf', '1'); setShow(false)
    } finally { setBusy(false) }
  }
  return <Sheet title="Welcome to DropBeam" size="large" dismissible={false} onClose={() => {}}><form onSubmit={e => { e.preventDefault(); void finish() }}><Section footer="Friends see this name when you send them files."><TextField label="Your name" placeholder="Name" autoCapitalize="words" autoFocus maxLength={40} value={name} onChange={e => setName(e.target.value)} /></Section>{error && <Section footer={<span className="mk-error" role="alert">{error}</span>} />}<div className="mk-sheet-cta"><Button filled type="submit" disabled={!name.trim() || busy}>{busy ? 'Saving…' : 'Continue'}</Button></div></form></Sheet>
}
