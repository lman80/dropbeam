/* eslint-disable react-refresh/only-export-components -- the offer card + servers list are shared with Chat */
// Settings → Transfer Server. Before setup: what it is (animated), whether this
// computer is a good home for it, one button. After setup: a calm status page —
// storage, who uses it, what's waiting — plus the servers this device can use.
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Check, FolderOpen, HardDrive, Monitor, Server } from 'lucide-react'
import { api, locationsApi, type MountCandidate } from '../lib/api'
import { formatBytes } from '../lib/format'
import { folderLabel } from '../lib/humanize'
import { IS_LINUX, IS_MAC, IS_WINDOWS } from '../lib/platform'
import {
  ACCESS_LABEL, expiresIn, heldFor, onServerChanged, onServersChanged, serverApi,
  type DeviceCheck, type ServerAccess, type ServerConfig, type ServerStatus, type UsableServer,
} from '../lib/transferServer'
import { useStore } from '../store'
import { Dialog } from './Dialog'
import { ServerExplainer } from './ServerExplainer'
import { Dot, MenuButton, SectionHeader, Segmented, Spinner, Toggle } from './ui'

const GB = 1_000_000_000
const DAYS = [
  { value: '7', label: '7 days' },
  { value: '14', label: '14 days' },
  { value: '30', label: '30 days' },
] as const
const COMPUTER = IS_MAC ? 'Mac' : IS_WINDOWS ? 'PC' : 'computer'

/** Friends (not the user's own devices), for access pickers. */
function usePeople() {
  const friends = useStore((s) => s.friends)
  const account = useStore((s) => s.myDevice?.account_pub)
  return useMemo(() => friends.filter((f) => !account || f.accountPub !== account), [friends, account])
}

function Row({ title, sub, children, className }: { title: React.ReactNode; sub?: React.ReactNode; children?: React.ReactNode; className?: string }) {
  return (
    <div className={`row set-row${className ? ` ${className}` : ''}`}>
      <div className="row-main">
        <div className="row-title">{title}</div>
        {sub && <div className="row-sub">{sub}</div>}
      </div>
      {children && <div className="row-trailing">{children}</div>}
    </div>
  )
}

/** "Stays on — a good choice" / "This Mac sleeps on its own…". */
function Fit({ check }: { check: DeviceCheck | null }) {
  if (!check) return null
  if (check.sleeps) {
    return <span className="srv-fit warn"><Dot tone="warn" />This {COMPUTER} goes to sleep on its own — held items wait until it wakes</span>
  }
  if (check.onBattery) {
    return <span className="srv-fit warn"><Dot tone="warn" />Running on battery — works best plugged in and always on</span>
  }
  if (check.sleeps === false) {
    return <span className="srv-fit"><Dot tone="ok" />This {COMPUTER} stays on, so it’s a good choice</span>
  }
  return null
}

// ── setup wizard ─────────────────────────────────────────────────────────────

type Draft = Pick<ServerConfig, 'name' | 'root' | 'capBytes' | 'fileDays' | 'access' | 'allowed' | 'through'>

function StorageStep({ draft, setDraft, check }: { draft: Draft; setDraft: (d: Draft) => void; check: DeviceCheck | null }) {
  const [mounts, setMounts] = useState<MountCandidate[] | null>(null)
  const [space, setSpace] = useState<[number, number] | null>(null)
  const root = draft.root || check?.defaultRoot || ''
  useEffect(() => {
    let alive = true
    locationsApi.mountCandidates().then((m) => { if (alive) setMounts(m) }).catch(() => { if (alive) setMounts([]) })
    return () => { alive = false }
  }, [])
  useEffect(() => {
    let alive = true
    if (!root) return
    serverApi.folderSpace(root).then((s) => { if (alive) setSpace(s) }).catch(() => {})
    return () => { alive = false }
  }, [root])
  const free = space?.[0] ?? check?.freeBytes ?? 0
  const maxCap = Math.max(GB, Math.min(free > 0 ? free * 0.9 : 100 * GB, 4000 * GB))
  const cap = Math.min(draft.capBytes, maxCap)
  const stepGb = maxCap > 500 * GB ? 10 * GB : GB
  const choices: { path: string; label: string; kind: 'computer' | 'network' | 'removable'; free: number | null }[] = [
    { path: '', label: `This ${COMPUTER}`, kind: 'computer', free: check?.freeBytes ?? null },
    ...(mounts ?? []).map((m) => ({ path: `${m.path}/DropBeam Transfer`, label: m.label, kind: m.kind as 'network' | 'removable', free: m.freeBytes ?? null })),
  ]
  const custom = draft.root && !choices.some((c) => c.path === draft.root)
  const browse = async () => {
    const d = await api.pickDirectory().catch(() => null)
    if (d) setDraft({ ...draft, root: d })
  }
  return (
    <>
      <div className="srv-field">
        <label htmlFor="srv-name">Name</label>
        <input id="srv-name" className="input" value={draft.name} maxLength={40} autoComplete="off" spellCheck={false}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        <p className="srv-hint">What friends see, like “Linux Box”.</p>
      </div>
      <div className="srv-field">
        <span className="srv-field-title">Keep things on</span>
        <div className="group location-pick-group" role="radiogroup" aria-label="Storage">
          {choices.map((c) => (
            <button type="button" role="radio" aria-checked={draft.root === c.path} key={c.path || 'computer'}
              className={`row location-pick${draft.root === c.path ? ' on' : ''}`} onClick={() => setDraft({ ...draft, root: c.path })}>
              <span className="location-glyph" aria-hidden>{c.kind === 'network' ? <Server /> : c.kind === 'removable' ? <HardDrive /> : <Monitor />}</span>
              <span className="row-main">
                <span className="row-title truncate-1">{c.label}</span>
                <span className="row-sub truncate-1">{c.kind === 'network' ? 'Network drive' : c.kind === 'removable' ? 'External disk' : 'Built-in disk'}{c.free != null ? ` · ${formatBytes(c.free)} free` : ''}</span>
              </span>
              {draft.root === c.path && <Check size={16} className="location-pick-check" aria-hidden />}
            </button>
          ))}
          {custom && (
            <button type="button" role="radio" aria-checked className="row location-pick on">
              <span className="location-glyph" aria-hidden><FolderOpen /></span>
              <span className="row-main"><span className="row-title truncate-1">{folderLabel(draft.root)}</span><span className="row-sub truncate-1" title={draft.root}>{draft.root}</span></span>
              <Check size={16} className="location-pick-check" aria-hidden />
            </button>
          )}
          <button type="button" className="row location-pick" onClick={browse}>
            <span className="location-glyph" aria-hidden><FolderOpen /></span>
            <span className="row-main"><span className="row-title">Choose a folder…</span></span>
          </button>
        </div>
      </div>
      <div className="srv-field srv-slider">
        <span className="srv-field-title">Use up to</span>
        <input type="range" aria-label="Storage limit" min={GB} max={maxCap} step={stepGb} value={cap}
          onChange={(e) => setDraft({ ...draft, capBytes: Number(e.target.value) })} />
        <div className="srv-slider-line"><b>{formatBytes(cap)}</b>{free > 0 && <span>{formatBytes(free)} free</span>}</div>
      </div>
      <div className="srv-field">
        <span className="srv-field-title">Keep undelivered files for</span>
        <Segmented label="Keep undelivered files for" value={String(draft.fileDays) as '7' | '14' | '30'}
          options={DAYS.map((d) => ({ ...d }))} onChange={(v) => setDraft({ ...draft, fileDays: Number(v) })} />
        <p className="srv-hint">Messages are kept 30 days. Everything is deleted as soon as it’s delivered.</p>
      </div>
    </>
  )
}

function PeoplePicker({ ids, onChange }: { ids: string[]; onChange: (ids: string[]) => void }) {
  const people = usePeople()
  if (!people.length) return <p className="srv-hint">No friends yet — you can choose people later.</p>
  return (
    <div className="location-chips" role="group" aria-label="People">
      {people.map((f) => {
        const on = ids.includes(f.id)
        return (
          <button type="button" key={f.id} className={`pick-chip${on ? ' on' : ''}`} aria-pressed={on} title={f.name}
            onClick={() => onChange(on ? ids.filter((id) => id !== f.id) : [...ids, f.id])}>
            {on && <Check size={12} aria-hidden />}<span className="truncate-1">{f.name}</span>
          </button>
        )
      })}
    </div>
  )
}

function AccessStep({ draft, setDraft }: { draft: Draft; setDraft: (d: Draft) => void }) {
  const people = usePeople()
  const everyone = draft.access === 'all' ? people.map((p) => p.id) : draft.allowed
  const throughAll = everyone.length > 0 && everyone.every((id) => draft.through.includes(id))
  const choice = (value: ServerAccess, title: string, hint: string, extra?: React.ReactNode) => (
    <label className="srv-choice">
      <input type="radio" name="srv-access" checked={draft.access === value} onChange={() => setDraft({ ...draft, access: value })} />
      <span className="srv-choice-title">{title}</span>
      <span className="srv-choice-hint">{hint}</span>
      {draft.access === value && extra && <div className="srv-choice-extra">{extra}</div>}
    </label>
  )
  return (
    <>
      <ServerExplainer kind="share" compact serverName={draft.name} />
      <div className="group" role="radiogroup" aria-label="Who can use it">
        {choice('all', 'All your friends', 'Friends can leave things for you, and for each other.')}
        {choice('chosen', 'Friends you choose', 'Only the people you pick.', <PeoplePicker ids={draft.allowed} onChange={(allowed) => setDraft({ ...draft, allowed })} />)}
        {choice('me', 'Only your devices', 'Holds things sent to you while your devices are off.')}
      </div>
      {draft.access !== 'me' && (
        <label className="srv-check">
          <input type="checkbox" checked={throughAll} disabled={!everyone.length}
            onChange={(e) => setDraft({ ...draft, through: e.target.checked ? everyone : [] })} />
          <span>
            Also let them send to people who don’t use it
            <span className="srv-hint" style={{ display: 'block' }}>Off keeps it to your friends. You can change this per person later.</span>
          </span>
        </label>
      )}
    </>
  )
}

export function SetupWizard({ check, current, startAt = 'storage', onClose, onSaved }: {
  check: DeviceCheck | null
  current?: ServerConfig
  startAt?: 'storage' | 'people'
  onClose: () => void
  onSaved: (s: ServerStatus) => void
}) {
  const deviceName = useStore((s) => s.myDevice?.name ?? '')
  const editing = !!current?.enabled
  const [step, setStep] = useState<'storage' | 'people' | 'done'>(startAt)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [draft, setDraft] = useState<Draft>(() => current?.enabled ? {
    name: current.name, root: current.root, capBytes: current.capBytes, fileDays: current.fileDays,
    access: current.access, allowed: current.allowed, through: current.through,
  } : {
    name: deviceName && !/iphone|ipad/i.test(deviceName) ? deviceName : 'Transfer Server',
    root: '', capBytes: check?.suggestedCap ?? 100 * GB, fileDays: 14, access: 'all', allowed: [], through: [],
  })
  const save = async () => {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      const status = await serverApi.configure({ ...draft, name: draft.name.trim(), enabled: true })
      onSaved(status)
      if (editing) onClose()
      else setStep('done')
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }
  const next = () => {
    if (step === 'storage') {
      if (!draft.name.trim()) return
      if (editing && startAt === 'storage') void save()
      else setStep('people')
    } else if (step === 'people') void save()
    else onClose()
  }
  const title = step === 'storage' ? (editing ? 'Storage' : 'Where to keep things') : step === 'people' ? 'Who can use it' : undefined
  const primary = step === 'done' ? 'Done' : step === 'storage' && !(editing && startAt === 'storage') ? 'Next' : busy ? 'Saving…' : editing ? 'Save' : 'Turn On'
  const footer = (
    <>
      {step === 'people' && !editing && <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => setStep('storage')}>Back</button>}
      {step !== 'done' && (editing || step === 'storage') && <button type="button" className="btn btn-secondary" disabled={busy} onClick={onClose}>Cancel</button>}
      <button type="submit" form="srv-wizard" className="btn btn-primary" disabled={busy || !draft.name.trim() || (step === 'people' && draft.access === 'chosen' && !draft.allowed.length)}>
        {busy && <Spinner size={12} />}{primary}
      </button>
    </>
  )
  return (
    <Dialog title={title} ariaLabel="Transfer Server setup" width={460} busy={busy} onClose={onClose} footer={footer}>
      <form id="srv-wizard" className="srv-wizard" onSubmit={(e) => { e.preventDefault(); next() }}>
        {!editing && step !== 'done' && <div className="srv-steps" aria-hidden><span className="on" /><span className={step === 'people' ? 'on' : ''} /></div>}
        {step === 'storage' && <StorageStep draft={draft} setDraft={setDraft} check={check} />}
        {step === 'people' && <AccessStep draft={draft} setDraft={setDraft} />}
        {step === 'done' && (
          <div className="srv-done">
            <span className="srv-done-mark" aria-hidden><Check size={22} strokeWidth={2.5} /></span>
            <h3>{draft.name.trim()} is ready</h3>
            <p>Friends see it the next time they connect. Your other devices start using it on their own.</p>
          </div>
        )}
        {error && <p className="srv-hint srv-error" role="alert">{error}</p>}
      </form>
    </Dialog>
  )
}

// ── management (after setup) ─────────────────────────────────────────────────

function Confirm({ title, body, action, danger, extra, onConfirm, onClose }: {
  title: string; body: string; action: string; danger?: boolean; extra?: React.ReactNode
  onConfirm: () => Promise<void>; onClose: () => void
}) {
  const [busy, setBusy] = useState(false)
  return (
    <Dialog title={title} width={400} busy={busy} onClose={onClose} footer={<>
      <button className="btn btn-secondary" disabled={busy} onClick={onClose}>Cancel</button>
      <button className={`btn ${danger ? 'btn-danger' : 'btn-primary'}`} disabled={busy}
        onClick={async () => { setBusy(true); try { await onConfirm(); onClose() } finally { setBusy(false) } }}>
        {busy && <Spinner size={12} />}{action}
      </button>
    </>}>
      <p className="srv-hint" style={{ fontSize: 'var(--text-13)' }}>{body}</p>
      {extra}
    </Dialog>
  )
}

/** Whose server this is (a computer not linked to an account): their friends can use it too. */
function OwnerRow({ status, setStatus }: { status: ServerStatus; setStatus: (s: ServerStatus) => void }) {
  const toast = useStore((s) => s.toast)
  const choices = status.ownerChoices ?? []
  const owner = status.owner
  const sub = !owner
    ? 'Choose whose server this is. Their friends can then use it too.'
    : owner.sharing > 0
      ? `Shared with ${owner.name}’s friends`
      : `To share it with ${owner.name}’s friends, tap Share on ${owner.name}’s phone or Mac`
  if (!owner && choices.length === 0) return null
  return (
    <Row title="Owner" sub={sub}>
      <select className="input set-field" aria-label="Owner" value={owner?.account ?? ''}
        onChange={async (e) => {
          try { setStatus(await serverApi.setOwner(e.target.value)) } catch (err) { toast('error', String(err)) }
        }}>
        <option value="">Nobody</option>
        {choices.map((o) => <option key={o.account} value={o.account}>{o.name}</option>)}
      </select>
    </Row>
  )
}

function ServerManager({ status, check, setStatus }: { status: ServerStatus; check: DeviceCheck | null; setStatus: (s: ServerStatus) => void }) {
  const toast = useStore((s) => s.toast)
  const people = usePeople()
  const c = status.config
  const [edit, setEdit] = useState<'storage' | 'people' | null>(null)
  const [confirm, setConfirm] = useState<'wipe' | 'off' | { remove: string; name: string } | null>(null)
  const [alsoDelete, setAlsoDelete] = useState(false)
  const [port, setPort] = useState(c.udpPort ? String(c.udpPort) : '')
  const [portChanged, setPortChanged] = useState(false)
  const [name, setName] = useState(c.name)
  const [shownName, setShownName] = useState(c.name)
  if (shownName !== c.name) { setShownName(c.name); setName(c.name) }
  const patch = async (p: Parameters<typeof serverApi.configure>[0]) => {
    try { setStatus(await serverApi.configure(p)) } catch (e) { toast('error', String(e)) }
  }
  const pct = c.capBytes > 0 ? Math.min(100, (status.used / c.capBytes) * 100) : 0
  const members = c.access === 'all' ? people : c.access === 'chosen' ? people.filter((p) => c.allowed.includes(p.id)) : []
  const usage = new Map(status.people.map((p) => [p.id, p]))
  const own = usage.get('own')
  const denied = [
    ...people.filter((p) => c.denied.includes(p.id)),
    ...status.people.filter((p) => p.viaOwner && p.removed),
  ]
  // Friends of the owner (vouched by the owner's devices) who aren't this computer's friends.
  const theirs = status.people.filter((p) => p.viaOwner && !p.removed && !members.some((m) => m.id === p.id))
  const ownerName = status.owner?.name
  const storageLabel = c.root ? folderLabel(c.root) : `This ${COMPUTER}`
  return (
    <>
      <SectionHeader>
        Transfer Server
        <span className="srv-status"><Dot tone={!status.storageOk ? 'error' : c.paused ? 'warn' : 'ok'} />{!status.storageOk ? 'Storage unavailable' : c.paused ? 'Paused' : 'On'}</span>
      </SectionHeader>
      <div className="group">
        <Row title="Name">
          <input className="input set-field" aria-label="Server name" value={name} maxLength={40}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => { const n = name.trim(); if (n && n !== c.name) void patch({ name: n }); else setName(c.name) }}
            onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur() }} />
        </Row>
        <Row title="Storage" sub={status.storageOk ? storageLabel : <span className="srv-error">{status.storageError}</span>}>
          {status.storageOk && (
            <div className="srv-usage">
              <div className={`srv-meter${pct > 90 ? ' warn' : ''}`} role="progressbar" aria-label="Storage used" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
                <span style={{ width: `${pct > 0 ? Math.max(1.5, pct) : 0}%` }} />
              </div>
              <div className="srv-usage-line tnum">{formatBytes(status.used)} of {formatBytes(c.capBytes)}{status.items ? ` · ${status.items} waiting` : ''}</div>
            </div>
          )}
          <button className="btn btn-secondary btn-sm" onClick={() => setEdit('storage')}>{status.storageOk ? 'Change' : 'Choose Again'}</button>
        </Row>
        <Row title="Keep undelivered files">
          <Segmented label="Keep undelivered files" value={String(c.fileDays) as '7' | '14' | '30'}
            options={[...DAYS, ...(DAYS.some((d) => d.value === String(c.fileDays)) ? [] : [{ value: String(c.fileDays), label: `${c.fileDays} days` }])] as { value: '7' | '14' | '30'; label: string }[]}
            onChange={(v) => void patch({ fileDays: Number(v) })} />
        </Row>
        <Row title="Who can use it" sub={ACCESS_LABEL[c.access]}>
          <button className="btn btn-secondary btn-sm" onClick={() => setEdit('people')}>Change</button>
        </Row>
        {!status.linked && <OwnerRow status={status} setStatus={setStatus} />}
        <Row title="Accept new things" sub={c.paused ? 'Paused — what’s already here still gets delivered.' : undefined}>
          <Toggle label="Accept new things" on={!c.paused} onChange={(v) => void patch({ paused: !v })} />
        </Row>
      </div>

      <SectionHeader>Advanced</SectionHeader>
      <div className="group">
        <Row title="Network port"
          sub={portChanged ? 'Restart DropBeam to use it.' : 'Optional. Forward this UDP port on your router so friends connect straight to this computer.'}>
          <input className="input set-field" style={{ width: 110 }} aria-label="UDP port" inputMode="numeric" placeholder="Automatic"
            value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, '').slice(0, 5))}
            onBlur={() => {
              const n = port ? Number(port) : 0
              if (n === (c.udpPort || 0)) return
              if (n !== 0 && (n < 1024 || n > 65535)) { toast('error', 'Pick a port from 1024 to 65535.'); setPort(c.udpPort ? String(c.udpPort) : ''); return }
              void patch({ udpPort: n }).then(() => setPortChanged(true))
            }}
            onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur() }} />
        </Row>
        {IS_LINUX && (
          <Row title="Keep running when no one is logged in" sub="Runs the server in the background after a restart. The app takes over whenever it’s open.">
            <button className="btn btn-secondary btn-sm" onClick={() => {
              void navigator.clipboard.writeText('sudo systemctl enable --now dropbeam-server@$USER')
                .then(() => toast('success', 'Copied — paste it into Terminal'))
                .catch(() => toast('error', 'Couldn’t copy'))
            }}>Copy Command</button>
          </Row>
        )}
      </div>

      <SectionHeader count={members.length + theirs.length + 1}>People</SectionHeader>
      <div className="group">
        <Row className="srv-person" title="You" sub={own ? `${own.items} waiting · ${formatBytes(own.bytes)}` : 'Your devices'} />
        {members.map((p) => {
          const u = usage.get(p.id)
          const through = c.through.includes(p.id)
          return (
            <Row key={p.id} className="srv-person"
              title={<>{p.name}{through && <span className="srv-tag">Can send to anyone</span>}</>}
              sub={u && u.items > 0 ? `${u.items} waiting · ${formatBytes(u.bytes)}` : 'Nothing waiting'}>
              <MenuButton label={`Options for ${p.name}`} size="sm" items={[
                through
                  ? { label: 'Only send to people who use this server', onSelect: () => void patch({ through: c.through.filter((id) => id !== p.id) }) }
                  : { label: 'Let them send to anyone', onSelect: () => void patch({ through: [...c.through, p.id] }) },
                { separator: true },
                { label: 'Remove', danger: true, onSelect: () => setConfirm({ remove: p.id, name: p.name }) },
              ]} />
            </Row>
          )
        })}
        {theirs.map((p) => (
          <Row key={p.id} className="srv-person" title={p.name}
            sub={`${ownerName ? `${ownerName}’s friend` : 'Owner’s friend'} · ${p.items > 0 ? `${p.items} waiting · ${formatBytes(p.bytes)}` : 'Nothing waiting'}`}>
            <MenuButton label={`Options for ${p.name}`} size="sm" items={[
              { label: 'Remove', danger: true, onSelect: () => setConfirm({ remove: p.id, name: p.name }) },
            ]} />
          </Row>
        ))}
        {denied.map((p) => (
          <Row key={p.id} className="srv-person" title={p.name} sub="Removed">
            <button className="btn btn-plain btn-sm" onClick={async () => { try { setStatus(await serverApi.restorePerson(p.id)) } catch (e) { toast('error', String(e)) } }}>Allow Again</button>
          </Row>
        ))}
      </div>

      {status.waiting.length > 0 && (
        <>
          <SectionHeader>Waiting for delivery</SectionHeader>
          <div className="group">
            {status.waiting.map((w) => (
              <Row key={w.label} title={w.label === 'Your devices' ? 'For your devices' : w.label.startsWith('Someone') ? 'For someone a friend knows' : `For ${w.label}`}
                sub={`${w.items} ${w.items === 1 ? 'item' : 'items'} · ${formatBytes(w.bytes)} · waiting ${heldFor(w.oldestMs)} · removed ${expiresIn(w.expiresMs)} if not delivered`} />
            ))}
          </div>
        </>
      )}

      <div className="srv-danger">
        <button className="btn btn-secondary" onClick={() => setConfirm('wipe')} disabled={!status.items}>Delete Everything</button>
        <button className="btn btn-secondary" onClick={() => { setAlsoDelete(false); setConfirm('off') }}>Turn Off</button>
      </div>

      {edit && <SetupWizard check={check} current={c} startAt={edit} onClose={() => setEdit(null)} onSaved={setStatus} />}
      {confirm === 'wipe' && (
        <Confirm title="Delete everything?" danger action="Delete"
          body={`${status.items} ${status.items === 1 ? 'item' : 'items'} waiting on ${c.name} will be deleted. Senders will see them as not delivered.`}
          onClose={() => setConfirm(null)} onConfirm={async () => { setStatus(await serverApi.wipe()) }} />
      )}
      {confirm === 'off' && (
        <Confirm title={`Turn off ${c.name}?`} action="Turn Off"
          body="Friends stop leaving things here. Anything already waiting is kept until you turn it back on, unless you delete it now."
          extra={status.items > 0 ? <label className="srv-check" style={{ padding: '10px 0 0' }}><input type="checkbox" checked={alsoDelete} onChange={(e) => setAlsoDelete(e.target.checked)} /><span>Also delete the {status.items} waiting {status.items === 1 ? 'item' : 'items'}</span></label> : undefined}
          onClose={() => setConfirm(null)} onConfirm={async () => { setStatus(await serverApi.disable(alsoDelete)) }} />
      )}
      {confirm && typeof confirm === 'object' && (
        <Confirm title={`Remove ${confirm.name}?`} danger action="Remove"
          body={`${confirm.name} can’t use ${c.name} anymore. Anything they left for other people is deleted; things they left for you still arrive.`}
          onClose={() => setConfirm(null)} onConfirm={async () => { setStatus(await serverApi.removePerson(confirm.remove)) }} />
      )}
    </>
  )
}

// ── servers this device can use ──────────────────────────────────────────────

export function useUsableServers() {
  const [servers, setServers] = useState<UsableServer[] | null>(null)
  const reload = useCallback(() => { serverApi.servers().then(setServers).catch(() => setServers([])) }, [])
  useEffect(() => {
    reload()
    const un = onServersChanged(reload)
    return () => { void un.then((f) => f()) }
  }, [reload])
  return { servers, setServers, reload }
}

function describe(s: UsableServer): string {
  if (s.revoked) return 'No longer available'
  if (s.paused) return 'Paused by its owner'
  if (s.own) return 'Yours · holds your messages and sends for you'
  if (s.owner && s.shareFriends) return 'Yours · shared with your friends'
  if (s.owner && !s.useIt) return 'Set up as yours · not in use yet'
  if (s.viaName && !s.useIt && !s.holdForMe) return `${s.viaName}’s · not in use`
  if (s.useIt && s.holdForMe) return 'Holds your messages and sends for you'
  if (s.useIt) return 'Sends for you when friends are offline'
  return 'Not in use'
}

export function ServersYouCanUse({ hosting }: { hosting: boolean }) {
  const { servers, setServers } = useUsableServers()
  const toast = useStore((s) => s.toast)
  if (servers === null) return null
  const set = async (eid: string, prefs: { useIt?: boolean; holdForMe?: boolean; offer?: string; shareFriends?: boolean }) => {
    try { setServers(await serverApi.serverPrefs(eid, { ...prefs, offer: prefs.offer ?? 'seen' })) } catch (e) { toast('error', String(e)) }
  }
  return (
    <>
      <SectionHeader>{hosting ? 'Other servers you can use' : 'Servers you can use'}</SectionHeader>
      <div className="group">
        {servers.length === 0 && (
          <div className="row"><span className="row-sub">When a friend shares their Transfer Server with you, it shows up here.</span></div>
        )}
        {servers.map((s) => (
          <div key={s.eid}>
            <Row title={<>{s.name}{(s.offer === 'new' || s.offer === 'share') && <span className="srv-tag">New</span>}</>} sub={describe(s)}>
              {s.revoked ? (
                <button className="btn btn-plain btn-sm" onClick={async () => setServers(await serverApi.forgetServer(s.eid))}>Remove</button>
              ) : !s.own && (
                <Toggle label={`Use ${s.name}`} on={s.useIt} disabled={s.paused} onChange={(v) => void set(s.eid, { useIt: v, holdForMe: v ? undefined : false })} />
              )}
            </Row>
            {s.owner && !s.own && !s.revoked && s.access !== 'me' && (
              <Row title="Let my friends use it" sub="They can leave messages for you here, and it holds theirs while they’re offline.">
                <Toggle label="Let my friends use it" on={!!s.shareFriends} onChange={(v) => void set(s.eid, { shareFriends: v })} />
              </Row>
            )}
            {!s.own && !s.revoked && s.useIt && (
              <Row title="Hold my messages here" sub="Friends leave things for you here while you’re offline.">
                <Toggle label="Hold my messages here" on={s.holdForMe} onChange={(v) => void set(s.eid, { holdForMe: v })} />
              </Row>
            )}
          </div>
        ))}
      </div>
    </>
  )
}

/** The one-time card a friend's shared server shows in their chat. */
export function ServerOfferCard({ server, friendName }: { server: UsableServer; friendName: string }) {
  if (server.offer === 'share') return <ShareOwnServerCard server={server} />
  if (server.via?.length) return <OwnersServerCard server={server} ownerName={server.viaName ?? friendName} />
  return <FriendServerCard server={server} friendName={friendName} />
}

/** The owner's own device: "may your friends use it?" (asked once). */
function ShareOwnServerCard({ server }: { server: UsableServer }) {
  const [busy, setBusy] = useState(false)
  const toast = useStore((s) => s.toast)
  const answer = async (share: boolean) => {
    setBusy(true)
    try {
      await serverApi.serverPrefs(server.eid, share ? { shareFriends: true, offer: 'seen' } : { offer: 'dismissed' })
      if (share) toast('success', `Your friends can use ${server.name}`)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="srv-offer" role="group" aria-label={`Share ${server.name} with your friends`}>
      <div>
        <h4>{server.name} is set up as your Transfer Server</h4>
        <p>Share it with your friends? They can leave messages for you there when you’re offline, and it holds theirs until they’re back. It can’t read any of it.</p>
      </div>
      <div className="srv-offer-actions">
        <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void answer(false)}>Not Now</button>
        <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => void answer(true)}>Share</button>
      </div>
    </div>
  )
}

/** A friend's device: their own server can hold your messages too. */
function OwnersServerCard({ server, ownerName }: { server: UsableServer; ownerName: string }) {
  const [busy, setBusy] = useState(false)
  const toast = useStore((s) => s.toast)
  const answer = async (on: boolean) => {
    setBusy(true)
    try {
      await serverApi.serverPrefs(server.eid, on ? { useIt: true, holdForMe: true, offer: 'seen' } : { offer: 'dismissed' })
      if (on) toast('success', `${server.name} will hold your messages`)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="srv-offer" role="group" aria-label={`${ownerName}’s ${server.name}`}>
      <div>
        <h4>{ownerName}’s {server.name} can hold your messages when you’re offline</h4>
        <p>They wait there, locked, until you’re back.</p>
      </div>
      <div className="srv-offer-actions">
        <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void answer(false)}>Not Now</button>
        <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => void answer(true)}>Turn On</button>
      </div>
    </div>
  )
}

function FriendServerCard({ server, friendName }: { server: UsableServer; friendName: string }) {
  const [hold, setHold] = useState(true)
  const [busy, setBusy] = useState(false)
  const toast = useStore((s) => s.toast)
  const answer = async (use: boolean) => {
    setBusy(true)
    try {
      await serverApi.serverPrefs(server.eid, use ? { useIt: true, holdForMe: hold, offer: 'seen' } : { offer: 'dismissed' })
      if (use) toast('success', `Using ${server.name}`)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="srv-offer" role="group" aria-label={`${friendName} shared a Transfer Server`}>
      <ServerExplainer kind="offline" compact serverName={server.name} />
      <div>
        <h4>{friendName} shared a Transfer Server with you</h4>
        <p>When someone you send to is offline, {server.name} holds it until they’re back. Everything stays locked — only they can open it.</p>
      </div>
      <div className="srv-offer-actions">
        <label><input type="checkbox" checked={hold} onChange={(e) => setHold(e.target.checked)} />Hold my messages there too</label>
        <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void answer(false)}>Not Now</button>
        <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => void answer(true)}>Use It</button>
      </div>
    </div>
  )
}

// ── the pane ─────────────────────────────────────────────────────────────────

export function TransferServerPane() {
  const [status, setStatus] = useState<ServerStatus | null>(null)
  const [check, setCheck] = useState<DeviceCheck | null>(null)
  const [wizard, setWizard] = useState(false)
  useEffect(() => {
    let alive = true
    let timer: number | undefined
    const load = () => serverApi.status().then((s) => { if (alive) setStatus(s) }).catch(() => {})
    load()
    serverApi.checkDevice().then((c) => { if (alive) setCheck(c) }).catch(() => {})
    const un = onServerChanged(() => { window.clearTimeout(timer); timer = window.setTimeout(load, 400) })
    const poll = window.setInterval(load, 30_000)
    return () => { alive = false; window.clearTimeout(timer); window.clearInterval(poll); void un.then((f) => f()) }
  }, [])
  if (!status) return <div className="row"><Spinner size={12} /></div>
  const hosting = status.supported && status.config.enabled
  return (
    <>
      {status.supported && !status.config.enabled && (
        <>
          <SectionHeader>Transfer Server</SectionHeader>
          <div className="card srv-intro">
            <ServerExplainer kind="offline" />
            <div className="srv-intro-copy">
              <h3>Hold messages and files for friends who are offline</h3>
              <p>When someone you send to is away, this {COMPUTER} keeps it — locked so only they can open it — and hands it over the moment they’re back.</p>
            </div>
            <div className="srv-intro-actions">
              <button className="btn btn-primary" onClick={() => setWizard(true)}>Set Up Transfer Server</button>
              <Fit check={check} />
            </div>
          </div>
        </>
      )}
      {hosting && <ServerManager status={status} check={check} setStatus={setStatus} />}
      <ServersYouCanUse hosting={hosting} />
      {wizard && <SetupWizard check={check} onClose={() => setWizard(false)} onSaved={setStatus} />}
    </>
  )
}
