// Turn engine state into words people understand. Primary UI never shows raw IPs,
// latency, relay region codes, verification jargon or absolute paths — those live
// behind an info popover (see components/ConnInspector.tsx → ConnInfo).
import type { ConnDetail, Locality } from './api'

/** True for "192.168.1.40:5", "[2001:db8::2]:52011", "fe80::1" … — an address, not a name. */
export function isRawAddress(value: string | null | undefined): boolean {
  if (!value) return false
  const v = value.trim()
  return /^\d{1,3}(\.\d{1,3}){3}(:\d+)?$/.test(v) || /^\[?[0-9a-f]*:[0-9a-f:]+\]?(:\d+)?$/i.test(v)
}

/** A person/device name for a peer, or null when all we have is an address. */
export function peerLabel(peer: string | null | undefined): string | null {
  if (!peer || !peer.trim() || isRawAddress(peer)) return null
  return peer.trim()
}

export type PathKind = 'local' | 'direct' | 'relay' | 'connecting'

/** Which way the bytes flow, from the live detail if we have it, else the locality. */
export function pathKind(detail?: ConnDetail | null, locality?: Locality | null): PathKind | null {
  const p = detail?.path
  if (p === 'local' || p === 'direct' || p === 'relay') return p
  if (p === 'connecting') return 'connecting'
  if (locality === 'local') return 'local'
  if (locality === 'direct') return 'direct'
  if (locality === 'internet') return 'relay'
  return null
}

/** Short human label for a path. */
export function pathLabel(kind: PathKind | null): string | null {
  switch (kind) {
    case 'local': return 'Local network'
    case 'direct': return 'Direct'
    case 'relay': return 'Relayed'
    case 'connecting': return 'Connecting'
    default: return null
  }
}

/** One plain sentence describing the connection, for the info popover. */
export function pathSentence(kind: PathKind | null): string {
  switch (kind) {
    case 'local': return 'Connected directly over your local network.'
    case 'direct': return 'Connected directly, peer to peer.'
    case 'relay': return 'Going through an encrypted relay because a direct connection isn’t possible on this network. It still works, just slower.'
    case 'connecting': return 'Finding the best way to connect…'
    default: return 'Not connected right now.'
  }
}

/** Last path segment of a file-system path (either separator). */
export function baseName(path: string | null | undefined): string {
  if (!path) return ''
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] ?? path
}

/** A folder named the way Finder would say it: "Downloads", not /Users/you/Downloads. */
export function folderLabel(path: string | null | undefined): string {
  return baseName(path) || 'folder'
}
