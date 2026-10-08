import type { Delivery } from './api'
import { deviceNoun, personGroups } from './deviceIcons.ts'

/** How a multi-device send reads: every device has it, some are still on their
 *  way, or one of them has a problem worth a glance. */
export type DeliveryTone = 'done' | 'pending' | 'problem'

const firstName = (name: string) => name.trim().split(/\s+/)[0] || name

function joinAnd(xs: string[]): string {
  if (xs.length <= 1) return xs.join('')
  if (xs.length === 2) return `${xs[0]} and ${xs[1]}`
  return `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}`
}

/** A reason we only know as a code, in words. Unknown codes read as nothing. */
function noteText(note?: string | null): string | null {
  if (!note) return null
  if (note === 'files_gone') return 'the files were moved'
  if (note === 'expired') return 'waited too long'
  if (note === 'refused') return 'it couldn’t open it'
  // Human notes ("Linux Box is full") pass through; engine errors don't.
  return /^[A-Z][^_]{0,60}$/.test(note) && !note.includes(':') ? note : null
}

/** The status of one device, as a short lowercase phrase ("waiting (Linux Box
 *  is holding it)"). `plural` = it describes several devices at once. */
function phrase(d: Delivery, plural: boolean): string {
  const it = plural ? 'they’re' : 'it’s'
  switch (d.state) {
    case 'delivered': return 'delivered'
    case 'held': return `waiting (${d.via ?? 'your Transfer Server'} is holding it)`
    case 'waiting': {
      const why = noteText(d.note)
      return why ? `waiting (${why.charAt(0).toLowerCase()}${why.slice(1)})` : `waiting — sends when ${it} online`
    }
    case 'uploading': return `uploading to ${d.via ?? 'your Transfer Server'}`
    case 'offline': return 'not reachable yet'
    case 'declined': return 'declined'
    case 'canceled': return 'canceled'
    case 'paused': return 'paused'
    case 'failed': {
      const why = noteText(d.note)
      return why ? `couldn’t deliver — ${why}` : 'couldn’t deliver'
    }
    default: return 'sending'
  }
}

/** One device's status for its own row ("Waiting · Linux Box is holding it"). */
export function deviceStatus(d: Delivery): string {
  switch (d.state) {
    case 'delivered': return 'Delivered'
    case 'held': return `Waiting · ${d.via ?? 'your Transfer Server'} is holding it`
    case 'waiting': {
      const why = noteText(d.note)
      return why ? `Waiting · ${why}` : 'Waiting · sends when it’s online'
    }
    case 'uploading': return `Uploading to ${d.via ?? 'your Transfer Server'}`
    case 'offline': return 'Not reachable yet'
    case 'declined': return 'Declined'
    case 'canceled': return 'Canceled'
    case 'paused': return 'Paused'
    case 'failed': {
      const why = noteText(d.note)
      return why ? `Couldn’t deliver · ${why}` : 'Couldn’t deliver'
    }
    default: return 'Sending'
  }
}

export function deliveryTone(ds: Delivery[]): DeliveryTone {
  if (ds.length > 0 && ds.every(d => d.state === 'delivered')) return 'done'
  return ds.some(d => d.state === 'failed' || d.state === 'declined') ? 'problem' : 'pending'
}

/**
 * One line for a send to a friend's devices:
 *   "Delivered to Alex’s Mac and iPhone"
 *   "Delivered to Alex’s Mac · iPhone: waiting (Linux Box is holding it)"
 *   "Alex’s Mac and iPhone: waiting — sends when they’re online"
 */
export function deliverySummary(friend: string, ds: Delivery[]): { text: string; tone: DeliveryTone } {
  const who = firstName(friend)
  const tone = deliveryTone(ds)
  if (ds.length === 0) return { text: '', tone }
  const done = ds.filter(d => d.state === 'delivered')
  if (done.length === ds.length) {
    const text = ds.length > 3 ? `Delivered to all ${ds.length} of ${who}’s devices` : `Delivered to ${who}’s ${joinAnd(done.map(d => d.label))}`
    return { text, tone }
  }
  const parts: string[] = []
  if (done.length) parts.push(`Delivered to ${who}’s ${joinAnd(done.map(d => d.label))}`)
  // Devices in the same situation share one clause.
  const groups = new Map<string, Delivery[]>()
  for (const d of ds.filter(d => d.state !== 'delivered')) {
    const key = phrase(d, false)
    groups.set(key, [...(groups.get(key) ?? []), d])
  }
  for (const group of groups.values()) {
    const labels = joinAnd(group.map(d => d.label))
    const clause = `${labels}: ${phrase(group[0], group.length > 1)}`
    parts.push(parts.length === 0 ? `${who}’s ${clause}` : clause)
  }
  return { text: parts.join(' · '), tone }
}

/** Only a send that went to more than one device needs per-device words. */
export function multiDevice(ds?: Delivery[] | null): ds is Delivery[] {
  return !!ds && ds.length > 1
}

/** The icon kind for a device (a Mac without a kind is most likely a laptop). */
export function deliveryIconKind(d: Pick<Delivery, 'kind' | 'os'>): string | undefined {
  if (d.os === 'macos' && d.kind !== 'desktop') return 'laptop'
  if (d.os === 'ios') return d.kind === 'tablet' ? 'tablet' : 'phone'
  return d.kind ?? undefined
}

/** One of a friend's devices, for "Send to iPhone only…". */
export interface PersonDevice { friendId: string; eid: string; label: string; kind?: string | null; os?: string | null }

/**
 * Every device of the person whose conversation is `ownerId` (their records
 * sharing a verified account), labeled like the send engine labels them:
 * "Mac" and "iPhone", or "Mac 1" / "Mac 2" when two would read the same.
 */
export function personDevices<T extends { id: string; endpointId?: string | null; accountPub?: string | null; deviceKind?: string | null; deviceOs?: string | null; createdAt: number }>(
  friends: readonly T[], ownerId: string, myAccount?: string | null,
): PersonDevice[] {
  const groups = personGroups(friends, myAccount)
  const members = friends.filter(f => f.endpointId && (f.id === ownerId || groups[f.id] === ownerId))
  const nouns = members.map(f => deviceNoun(f.deviceKind, f.deviceOs))
  const byEid = [...members].sort((a, b) => (a.endpointId! < b.endpointId! ? -1 : a.endpointId! > b.endpointId! ? 1 : 0))
  return members.map((f, i) => {
    const same = byEid.filter(g => deviceNoun(g.deviceKind, g.deviceOs) === nouns[i])
    const label = same.length > 1 ? `${nouns[i]} ${same.indexOf(f) + 1}` : nouns[i]
    return { friendId: f.id, eid: f.endpointId!, label, kind: f.deviceKind, os: f.deviceOs }
  })
}
