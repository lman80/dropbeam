// A short looping explainer for the Transfer Server (~9s). Three stations —
// you, the server, your friend — and one paper plane that tells the story:
// it can't reach a sleeping friend, waits (locked) on the server, then lands
// when they wake. Reduce Motion shows the same three beats as a still, numbered
// strip instead. Pure CSS; no gradients, no glow — the accent is spent only on
// the plane and the check.
import { useEffect, useState } from 'react'
import { Bell, Check, Laptop, Lock, Send, Server, Smartphone, Users } from 'lucide-react'

export type ExplainerKind = 'offline' | 'share' | 'push'

const SCENES: Record<ExplainerKind, string[]> = {
  offline: [
    'Your friend is offline',
    'It waits on your Transfer Server, locked so only they can open it',
    'It arrives when they’re back — then the server deletes its copy',
  ],
  // #45: an iPhone that's asleep still hears about it.
  push: [
    'A friend sends something while your iPhone is asleep',
    'Your Transfer Server holds it, locked, and sends the iPhone a notification',
    'They see who it’s from — open DropBeam and it arrives',
  ],
  share: [
    'Friends can leave things here too',
    'Everything stays locked — the server can’t open any of it',
    'You choose who can use it and how much space they get',
  ],
}

function useReducedMotion(): boolean {
  const query = typeof window !== 'undefined' && window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null
  const [reduced, setReduced] = useState(!!query?.matches)
  useEffect(() => {
    if (!query) return
    const on = () => setReduced(query.matches)
    query.addEventListener('change', on)
    return () => query.removeEventListener('change', on)
  }, [query])
  return reduced
}

export function ServerExplainer({ kind = 'offline', serverName, friendName, compact }: {
  kind?: ExplainerKind
  serverName?: string
  friendName?: string
  compact?: boolean
}) {
  const reduced = useReducedMotion()
  const scenes = SCENES[kind]
  const box = serverName || 'Transfer Server'
  const them = friendName || 'Friend'
  const stage = (
    <div className={`sx-stage sx-${kind}`} aria-hidden>
      <div className="sx-station sx-you">
        <span className="sx-disc">{kind === 'share' ? <Users /> : kind === 'push' ? <Laptop /> : <Smartphone />}</span>
        <span className="sx-label">{kind === 'share' ? 'Friends' : kind === 'push' ? 'A friend' : 'You'}</span>
        <span className="sx-check"><Check strokeWidth={3} /></span>
      </div>
      <div className="sx-station sx-box">
        <span className="sx-disc"><Server /></span>
        <span className="sx-label truncate-1">{box}</span>
        <span className="sx-lock"><Lock strokeWidth={2.5} /></span>
        {kind === 'share' && <span className="sx-gauge"><span /></span>}
      </div>
      <div className="sx-station sx-them">
        <span className="sx-disc">{kind === 'share' ? <Lock /> : kind === 'push' ? <Smartphone /> : <Laptop />}</span>
        <span className="sx-label truncate-1">{kind === 'share' ? 'Only for them' : kind === 'push' ? 'iPhone' : them}</span>
        <span className="sx-zzz">z</span>
        {kind === 'push' && <span className="sx-bell"><Bell strokeWidth={2.5} /></span>}
      </div>
      <span className="sx-track" />
      <span className="sx-plane"><Send /></span>
      {kind === 'share' && <><span className="sx-plane sx-plane-2"><Send /></span><span className="sx-plane sx-plane-3"><Send /></span></>}
    </div>
  )
  if (reduced) {
    return (
      <figure className={`sx sx-still${compact ? ' sx-compact' : ''}`}>
        {stage}
        <ol className="sx-steps">
          {scenes.map((s) => <li key={s}>{s}</li>)}
        </ol>
      </figure>
    )
  }
  return (
    <figure className={`sx${compact ? ' sx-compact' : ''}`} aria-label={scenes.join('. ')}>
      {stage}
      <figcaption className="sx-captions" aria-hidden>
        {scenes.map((s, i) => <span key={s} className={`sx-cap sx-cap-${i + 1}`}>{s}</span>)}
      </figcaption>
    </figure>
  )
}
