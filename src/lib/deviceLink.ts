// Linking your own devices: the words and routing shared by the desktop
// screens and the native (iOS) bridge. The engine does the hard part — either
// code works whichever device scans it, and it refuses two different accounts
// before touching anything — so this only classifies codes and phrases progress.

import { deviceNoun } from './deviceIcons.ts'

export type DeviceCodeKind = 'link' | 'join' | 'friend' | 'invalid'

/** What a scanned/pasted string is: a device code ("link me" / "join me"), a friend code, or neither. */
export function deviceCodeKind(code: string): DeviceCodeKind {
  const c = code.trim().toLowerCase()
  if (c.startsWith('dropbeamlink1:')) return 'link'
  if (c.startsWith('dropbeamjoin1:')) return 'join'
  if (c.startsWith('dropbeam:') || c.startsWith('dropbeamf1:')) return 'friend'
  return 'invalid'
}

export const isDeviceCode = (code: string) => { const k = deviceCodeKind(code); return k === 'link' || k === 'join' }

/** For a device-code scanner: why this value can't be used (null = go ahead). */
export function deviceCodeProblem(code: string): string | null {
  switch (deviceCodeKind(code)) {
    case 'link': case 'join': return null
    case 'friend': return 'That’s a friend code. To link your own devices, scan the code in Settings → Devices on your other device.'
    default: return 'That isn’t a DropBeam device code. On your other device open Settings → Devices → Link a Device.'
  }
}

/** For a friend scanner: a device code belongs in Settings → Devices. */
export function friendCodeProblem(code: string): string | null {
  return isDeviceCode(code) ? 'That code links your own devices, not a friend. Open Settings → Devices to link it.' : null
}

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`

/** "7 friends and 309 messages" (or null when there's nothing to count). */
export function contentSummary(friends?: number | null, messages?: number | null): string | null {
  const parts = [friends ? plural(friends, 'friend') : null, messages ? plural(messages, 'message') : null].filter(Boolean)
  return parts.length ? parts.join(' and ') : null
}

export interface LinkProgress { stage: 'waiting' | 'sending' | 'importing'; friends?: number; messages?: number }

/** The one line shown while a link runs. */
export function progressText(p?: LinkProgress | null): string {
  if (!p) return 'Connecting to your other device…'
  const what = contentSummary(p.friends, p.messages)
  switch (p.stage) {
    case 'waiting': return 'Connected — waiting for your other device…'
    case 'sending': return what ? `Sending ${what}…` : 'Linking…'
    case 'importing': return what ? `Bringing over ${what}…` : 'Setting up this device…'
  }
}

export interface LinkedDevice { name?: string; device_kind?: string | null; device_os?: string | null; friends?: number | null; messages?: number | null }

/** "Linked with your iPhone". */
export function linkedTitle(d?: LinkedDevice | null): string {
  if (!d || (!d.device_kind && !d.device_os)) return d?.name ? `Linked with ${d.name}` : 'Your devices are linked'
  return `Linked with your ${deviceNoun(d.device_kind, d.device_os)}`
}

/** The sentence under the success title. */
export function linkedDetail(d?: LinkedDevice | null): string {
  const what = contentSummary(d?.friends, d?.messages)
  return `${what ? `${what[0].toUpperCase()}${what.slice(1)} ${(d?.friends ?? 0) + (d?.messages ?? 0) === 1 ? 'is' : 'are'} on both devices now. ` : ''}From here on your friends, chats, name and photo stay in sync.`
}

/** Tauri/Rust errors arrive as plain strings, sometimes wrapped in an Error. */
export function linkErrorText(e: unknown): string {
  const s = (e instanceof Error ? e.message : String(e ?? '')).replace(/^Error:\s*/, '').trim()
  return s || 'Linking didn’t work. Try again.'
}
