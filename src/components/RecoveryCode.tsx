// Recovery code screens (docs/RECOVERY-CODE.md): save the 12 (or 24) words,
// check they were written down, restore an account with them on a new device,
// and — after a restore — see who found you again and remove old devices.
import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import { KeyRound, Printer, QrCode } from 'lucide-react'
import { api, onRecoveryChanged } from '../lib/api'
import { makeQuiz, oldDeviceName, ordinal, RECOVERY_NEVER, RECOVERY_WARNING, RECOVERY_WHY, restoreSummary, splitWords, isRecoveryQr,
  type QuizQuestion, type RecoveryCheck, type RecoveryStatus } from '../lib/recovery'
import { useStore } from '../store'
import { Dialog } from './Dialog'
import { QrCodeView } from './CodeQr'
import { QrScanner } from './QrScanner'
import { ConfirmDialog } from './SafetyDialogs'

const errText = (e: unknown) => (typeof e === 'string' ? e : e instanceof Error ? e.message : String(e))

/** The numbered words, in two columns (1–6 | 7–12), like the printed sheet. */
function WordGrid({ words }: { words: string[] }) {
  return <ol className="recovery-words" aria-label="Your recovery words" style={{ gridTemplateRows: `repeat(${Math.ceil(words.length / 2)}, auto)` }}>
    {words.map((w, i) => <li key={i}><span className="recovery-n">{i + 1}</span><span className="recovery-w">{w}</span></li>)}
  </ol>
}

/** Rendered only while the words are on screen; the print stylesheet shows just this. */
function PrintSheet({ words, qr }: { words: string[]; qr: string }) {
  return createPortal(<div className="recovery-print-root" aria-hidden>
    <h1>DropBeam recovery code</h1>
    <p>{RECOVERY_WHY}</p>
    <p><b>{RECOVERY_WARNING}</b></p>
    <WordGrid words={words} />
    <QRCodeSVG value={qr} size={180} level="M" marginSize={2} />
    <p>To use it: install DropBeam on the new device, choose “Restore with Recovery Code”, then type these words or scan this code.</p>
  </div>, document.body)
}

type SaveStep = 'intro' | 'words' | 'quiz' | 'done'

/** Save your recovery code: why → the words (+ QR, print) → check two words → done. */
export function SaveRecoveryModal({ onClose, offer = false }: { onClose: () => void; offer?: boolean }) {
  const [step, setStep] = useState<SaveStep>('intro')
  const [code, setCode] = useState<{ words: string[]; qr: string } | null>(null)
  const [quiz, setQuiz] = useState<QuizQuestion[]>([])
  const [q, setQ] = useState(0)
  const [answers, setAnswers] = useState<{ index: number; word: string }[]>([])
  const [wrong, setWrong] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const show = async () => {
    setBusy(true); setError(null)
    try { const r = await api.recoveryReveal(); setCode(r); setStep('words'); void useStore.getState().refreshMyDevice().catch(() => {}) }
    catch (e) { setError(errText(e)) }
    finally { setBusy(false) }
  }
  const later = async () => { if (offer) await api.recoveryLater().catch(() => {}); onClose() }
  const startQuiz = () => { if (!code) return; setQuiz(makeQuiz(code.words)); setQ(0); setAnswers([]); setWrong(false); setStep('quiz') }
  const choose = async (word: string) => {
    const question = quiz[q]
    if (!code || !question) return
    if (code.words[question.index] !== word) { setWrong(true); return }
    const next = [...answers, { index: question.index, word }]
    setWrong(false)
    if (q + 1 < quiz.length) { setAnswers(next); setQ(q + 1); return }
    setBusy(true)
    try { await api.recoveryConfirmSaved(next); setCode(null); setStep('done') }
    catch (e) { setError(errText(e)) }
    finally { setBusy(false) }
  }
  const print = () => { void api.recoveryPrint().catch(e => setError(errText(e))) }

  if (step === 'intro') return <Dialog title="Save Your Recovery Code" width={440} onClose={busy ? undefined : () => void later()} busy={busy}
    footer={<>
      <button className="btn btn-plain" onClick={() => void later()} disabled={busy}>{offer ? 'Later' : 'Cancel'}</button>
      <span className="spacer" />
      <button className="btn btn-primary" onClick={() => void show()} disabled={busy}>{busy ? 'Getting It…' : 'Show My Code'}</button>
    </>}>
    <div className="recovery-intro">
      <KeyRound size={28} strokeWidth={1.6} aria-hidden />
      <p>{RECOVERY_WHY}</p>
      <p>Your code is a list of words. Have a pen and paper ready — it takes about two minutes.</p>
      {error && <p className="recovery-error" role="alert">{error}</p>}
    </div>
  </Dialog>

  if (step === 'words' && code) return <Dialog title="Write These Words Down" width={520} onClose={onClose}
    footer={<>
      <button className="btn btn-secondary" onClick={print}><Printer size={14} aria-hidden /> Print…</button>
      <span className="spacer" />
      <button className="btn btn-primary" onClick={startQuiz}>I’ve Written Them Down</button>
    </>}>
    <p className="recovery-lead">Write each word on paper, in order, exactly as shown.</p>
    <div className="recovery-words-row">
      <WordGrid words={code.words} />
      <QrCodeView value={code.qr} size={132} enlarge={false} label="QR code of your recovery words" hint="The same code as a picture, for printing" />
    </div>
    <p className="recovery-warning" role="note">{RECOVERY_WARNING}</p>
    <p className="recovery-small">{RECOVERY_NEVER}</p>
    {error && <p className="recovery-error" role="alert">{error}</p>}
    <PrintSheet words={code.words} qr={code.qr} />
  </Dialog>

  if (step === 'quiz' && quiz[q]) return <Dialog title="Check Your Paper" width={440} onClose={onClose} busy={busy}
    footer={<>
      <button className="btn btn-plain" onClick={() => setStep('words')} disabled={busy}>Show the Words Again</button>
      <span className="spacer" />
    </>}>
    <p className="recovery-lead">Look at your paper. Which word is <b>number {quiz[q].index + 1}</b>?</p>
    <div className="recovery-choices" role="group" aria-label={`Choices for the ${ordinal(quiz[q].index + 1)} word`}>
      {quiz[q].options.map(w => <button key={w} className="btn btn-secondary recovery-choice" disabled={busy} onClick={() => void choose(w)}>{w}</button>)}
    </div>
    <p className="recovery-small">Question {q + 1} of {quiz.length}</p>
    {wrong && <p className="recovery-error" role="alert">That’s not it. Check word number {quiz[q].index + 1} on your paper — if it’s missing or different, choose Show the Words Again.</p>}
    {error && <p className="recovery-error" role="alert">{error}</p>}
  </Dialog>

  return <Dialog title="Your Code Is Saved" width={420} onClose={onClose}
    footer={<><span className="spacer" /><button className="btn btn-primary" onClick={onClose}>Done</button></>}>
    <p className="recovery-lead">Keep the paper somewhere safe, like with your important papers. You can see your code again any time in Settings → Devices.</p>
  </Dialog>
}

/** Restore an account on this (new) device from the words on paper. */
export function RestoreRecoveryModal({ onClose, onRestored }: { onClose: () => void; onRestored?: () => void }) {
  const [count, setCount] = useState<12 | 24>(12)
  const [words, setWords] = useState<string[]>(() => Array(12).fill(''))
  const [checked, setChecked] = useState<{ text: string; check: RecoveryCheck } | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [scanning, setScanning] = useState(false)
  const [done, setDone] = useState(false)
  const refs = useRef<(HTMLInputElement | null)[]>([])
  const text = useMemo(() => words.map(w => w.trim()).filter(Boolean).join(' '), [words])
  useEffect(() => {
    if (!text) return
    let stale = false
    const t = window.setTimeout(() => { void api.recoveryCheck(text).then(c => { if (!stale) setChecked({ text, check: c }) }).catch(() => {}) }, 250)
    return () => { stale = true; window.clearTimeout(t) }
  }, [text])
  // Only the answer for what's in the boxes now counts.
  const check = text && checked?.text === text ? checked.check : null

  const fill = (list: string[], from = 0) => {
    const n: 12 | 24 = from === 0 && list.length > 12 ? 24 : count
    setCount(n)
    setWords(prev => {
      const next = Array.from({ length: n }, (_, i) => prev[i] ?? '')
      list.slice(0, n - from).forEach((w, i) => { next[from + i] = w })
      return next
    })
    window.setTimeout(() => refs.current[Math.min(from + list.length, n - 1)]?.focus(), 0)
  }
  const setLength = (n: 12 | 24) => { setCount(n); setWords(prev => Array.from({ length: n }, (_, i) => prev[i] ?? '')) }
  const restore = async () => {
    setBusy(true); setError(null)
    try { await api.recoveryRestore(text); setWords(Array(count).fill('')); setDone(true); onRestored?.() }
    catch (e) { setError(errText(e)) }
    finally { setBusy(false) }
  }
  const unknown = new Set(check?.unknown ?? [])
  const problem = error ?? (check && (check.count === count || unknown.size) ? check.problem : null)

  if (done) return <Dialog title="Welcome Back" width={440} onClose={onClose}
    footer={<><span className="spacer" /><button className="btn btn-primary" onClick={onClose}>Done</button></>}>
    <p className="recovery-lead">Your account is on this device now.</p>
    <p className="recovery-small">Your friends will find you over the next few hours, as their DropBeam opens, and send back your chats. You don’t need to do anything — you can see who’s back in Settings → Devices.</p>
    <p className="recovery-small">Files and shared folders don’t come back by themselves. Settings → Devices lists the folders your friends remember, so you can ask them to invite you again.</p>
  </Dialog>

  return <Dialog title="Restore With Recovery Code" width={540} onClose={onClose} busy={busy}
    footer={<>
      <button className="btn btn-plain" onClick={() => setScanning(true)} disabled={busy}><QrCode size={14} aria-hidden /> Scan Printed Code…</button>
      <span className="spacer" />
      <button className="btn btn-primary" onClick={() => void restore()} disabled={busy || !check?.valid}>{busy ? 'Restoring…' : 'Restore'}</button>
    </>}>
    <p className="recovery-lead">Type the words from your paper, in order. The first four letters of each word are enough.</p>
    <div className="recovery-length" role="radiogroup" aria-label="How many words">
      {([12, 24] as const).map(n => <label key={n}><input type="radio" name="recovery-length" checked={count === n} onChange={() => setLength(n)} /> {n} words</label>)}
    </div>
    <div className="recovery-inputs">
      {words.map((w, i) => <label key={i} className={unknown.has(i) ? 'bad' : ''}>
        <span className="recovery-n">{i + 1}</span>
        <input ref={el => { refs.current[i] = el }} value={w} autoFocus={i === 0} autoComplete="off" autoCorrect="off" autoCapitalize="off" spellCheck={false}
          aria-label={`Word ${i + 1}`} aria-invalid={unknown.has(i) || undefined}
          onChange={e => {
            const parts = splitWords(e.target.value)
            if (parts.length > 1) { fill(parts, i); return }
            setWords(prev => prev.map((x, j) => (j === i ? e.target.value.replace(/[^a-zA-Z]/g, '').toLowerCase() : x)))
          }}
          onKeyDown={e => {
            if ((e.key === ' ' || e.key === 'Enter') && w) { e.preventDefault(); refs.current[i + 1]?.focus() }
            if (e.key === 'Enter' && i === words.length - 1 && check?.valid) void restore()
            if (e.key === 'Backspace' && !w && i > 0) { e.preventDefault(); refs.current[i - 1]?.focus() }
          }} />
      </label>)}
    </div>
    {problem ? <p className="recovery-error" role="alert">{problem}</p>
      : check?.valid ? <p className="recovery-ok">These words are right. Choose Restore.</p>
      : <p className="recovery-small">Restoring makes this device yours again: your friends will recognize it and send back your chats.</p>}
    {scanning && <QrScanner title="Scan Your Recovery Code" hint="Hold the printed code up to the camera."
      validate={t => (isRecoveryQr(t) ? null : 'That isn’t a recovery code. Look for the code printed with your words.')}
      onClose={() => setScanning(false)} onResult={t => { setScanning(false); fill(splitWords(t)) }} />}
  </Dialog>
}

/** Settings → Devices: the recovery code row, and after a restore, what came back. */
export function RecoverySection() {
  const [status, setStatus] = useState<RecoveryStatus | null>(null)
  const [saving, setSaving] = useState(false)
  const [confirm, setConfirm] = useState<null | string[]>(null)
  const refresh = () => { void api.recoveryStatus().then(setStatus).catch(() => {}) }
  useEffect(() => {
    refresh()
    const un = onRecoveryChanged(refresh)
    return () => { void un.then(f => f()) }
  }, [])
  if (!status) return null
  const r = status.restore
  const remove = async (ids: string[]) => {
    try { await api.recoveryRemoveOldDevices(ids); useStore.getState().toast('success', ids.length === 1 ? 'The old device was removed' : 'The old devices were removed'); refresh(); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', errText(e)) }
  }
  const keep = async (id: string) => { await api.recoveryKeepOldDevice(id).catch(() => {}); refresh() }
  return <section className="recovery-section">
    <h2 className="section-title">Recovery code</h2>
    <div className="group">
      <button className="row account-device-add" onClick={() => setSaving(true)}>
        <span className="account-device-glyph" aria-hidden><KeyRound size={16} /></span>
        <span className="row-main">
          <span className="row-title">{status.saved ? 'Show Recovery Code…' : 'Save Your Recovery Code…'}</span>
          <span className={`row-sub ${status.saved ? '' : 'recovery-unsaved'}`}>{status.saved
            ? 'Saved. Keep the paper somewhere safe.'
            : 'Not saved yet. If you lose all your devices, it’s the only way to get your friends and chats back.'}</span>
        </span>
      </button>
    </div>
    {r && <>
      <h3 className="recovery-sub">Since you restored</h3>
      <p className="account-devices-note">{restoreSummary(r)}</p>
      {r.oldDevices.length > 0 && <div className="group">
        <div className="row"><div className="row-main"><div className="row-sub">Your devices from before. If one is lost or stolen, remove it so nobody can use it as you. Still have it? Keep it, then link it again in Link a Device.</div></div>
          {r.oldDevices.length > 1 && <div className="row-trailing"><button className="btn btn-secondary btn-sm" onClick={() => setConfirm(r.oldDevices.map(d => d.endpointId))}>Remove All…</button></div>}
        </div>
        {r.oldDevices.map(d => <div key={d.endpointId} className="row">
          <div className="row-main">
            <div className="row-title truncate-1">{oldDeviceName(d)}</div>
            <div className="row-sub truncate-1">{d.via} still knew it</div>
          </div>
          <div className="row-trailing">
            <button className="btn btn-plain btn-sm" onClick={() => void keep(d.endpointId)}>I Still Have It</button>
            <button className="btn btn-secondary btn-sm" onClick={() => setConfirm([d.endpointId])}>Remove…</button>
          </div>
        </div>)}
      </div>}
      {r.folders.length > 0 && <p className="account-devices-note">Shared folders you were in: {r.folders.map(f => `${f.name} (with ${f.with})`).join(', ')}. Their files are still with your friends — ask them to invite you again.</p>}
    </>}
    {saving && <SaveRecoveryModal onClose={() => { setSaving(false); refresh() }} />}
    <AnimatePresence>
      {confirm && <ConfirmDialog key="remove-old" title={confirm.length === 1 ? 'Remove this old device?' : 'Remove your old devices?'}
        confirmLabel="Remove" onConfirm={() => remove(confirm)} onClose={() => setConfirm(null)}>
        Your friends will stop treating {confirm.length === 1 ? 'it' : 'them'} as you, and {confirm.length === 1 ? 'it gets' : 'they get'} no new messages. If you find {confirm.length === 1 ? 'it' : 'one'} later, you can link it again.
      </ConfirmDialog>}
    </AnimatePresence>
  </section>
}
