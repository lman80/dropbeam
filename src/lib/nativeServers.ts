// Transfer Server data as the native iOS shell sees it. Pure (no Tauri import) so
// the normalization is testable in node; nativeBridge.ts does the I/O.
import type { PendingFile, UsableServer } from './transferServer'
import type { BridgeArgs } from './nativeBridgeProtocol'

const OFFERS = new Set(['new', 'seen', 'dismissed', ''])
const obj = (v: unknown): Record<string, unknown> | null => v && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : null
const str = (v: unknown, fallback = '') => typeof v === 'string' ? v : fallback
const num = (v: unknown) => typeof v === 'number' && Number.isFinite(v) ? v : 0

/** Servers this device may use: malformed rows dropped, flags coerced to booleans. */
export function nativeServerList(list: unknown): UsableServer[] {
  if (!Array.isArray(list)) return []
  const seen = new Set<string>()
  return list.flatMap((raw) => {
    const s = obj(raw)
    const eid = s && str(s.eid).trim()
    if (!s || !eid || seen.has(eid)) return []
    seen.add(eid)
    const offer = str(s.offer)
    return [{
      eid,
      name: str(s.name).trim() || 'Transfer Server',
      own: s.own === true,
      member: s.member === true,
      through: s.through === true,
      useIt: s.useIt === true,
      holdForMe: s.useIt === true && s.holdForMe === true,
      offer: OFFERS.has(offer) ? offer : '',
      revoked: s.revoked === true,
      paused: s.paused === true,
      learnedMs: num(s.learnedMs),
    }]
  })
}

/** Held files waiting for the user's OK; only rows with a link id are actionable. */
export function nativePendingFiles(list: unknown): PendingFile[] {
  if (!Array.isArray(list)) return []
  return list.flatMap((raw) => {
    const p = obj(raw)
    const linkId = p && str(p.linkId)
    if (!p || !linkId) return []
    return [{
      linkId,
      peerId: str(p.peerId),
      server: str(p.server),
      serverName: str(p.serverName).trim() || 'the Transfer Server',
      itemId: str(p.itemId),
      bytes: num(p.bytes),
      names: Array.isArray(p.names) ? p.names.filter((n): n is string => typeof n === 'string') : [],
      at: num(p.at),
    }]
  })
}

export interface ServerPrefs { useIt?: boolean; holdForMe?: boolean; offer?: string }

/** Validate a native `serverPrefs` call. Turning a server off also stops holding. */
export function serverPrefsArgs(a: BridgeArgs): { eid: string; prefs: ServerPrefs } {
  const eid = typeof a.eid === 'string' ? a.eid.trim() : ''
  if (!eid) throw new Error('Missing eid')
  const prefs: ServerPrefs = {}
  for (const key of ['useIt', 'holdForMe'] as const) {
    if (a[key] == null) continue
    if (typeof a[key] !== 'boolean') throw new Error(`Invalid ${key}`)
    prefs[key] = a[key] as boolean
  }
  if (a.offer != null) {
    if (typeof a.offer !== 'string' || !OFFERS.has(a.offer) || a.offer === 'new') throw new Error('Invalid offer')
    prefs.offer = a.offer
  }
  if (prefs.useIt === false) prefs.holdForMe = false
  if (prefs.offer == null && (prefs.useIt != null || prefs.holdForMe != null)) prefs.offer = 'seen'
  if (!Object.keys(prefs).length) throw new Error('Nothing to change')
  return { eid, prefs }
}
