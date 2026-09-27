// Contacts that might be the same person on two unlinked devices (same name or
// same photo, different identities). A hint only — nothing is merged.
import { useSyncExternalStore } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { HAS_TAURI } from './api'

export interface LookAlike { id: string; name: string }

let groups: LookAlike[][] = []
const subs = new Set<() => void>()
let started = false

export function refreshLookAlikes(): Promise<void> {
  if (!HAS_TAURI) {
    const q = typeof location !== 'undefined' ? new URLSearchParams(location.search) : null
    groups = q?.get('lookalike') === '1' ? [[{ id: 'f1', name: 'Alex' }, { id: 'f9', name: 'Alex' }]] : []
    subs.forEach((cb) => cb())
    return Promise.resolve()
  }
  return invoke<LookAlike[][]>('friends_look_alike')
    .then((g) => { groups = Array.isArray(g) ? g : [] })
    .catch(() => {})
    .then(() => subs.forEach((cb) => cb()))
}

function subscribe(cb: () => void) {
  subs.add(cb)
  if (!started) {
    started = true
    void refreshLookAlikes()
    if (HAS_TAURI) void listen('friends://changed', () => void refreshLookAlikes())
  }
  return () => { subs.delete(cb) }
}

export function useLookAlikes(): LookAlike[][] {
  return useSyncExternalStore(subscribe, () => groups)
}

export function lookAlikeGroups(): LookAlike[][] {
  return groups
}

/** The other contacts `id` might be the same person as. */
export function lookAlikesOf(all: LookAlike[][], id: string): LookAlike[] {
  return all.find((g) => g.some((x) => x.id === id))?.filter((x) => x.id !== id) ?? []
}

/** "Mong and Mong", "Chen, Wei and Chen W." */
export function joinNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ''
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

export const LOOK_ALIKE_HINT = 'might be the same person on two devices — ask them to link their devices (Settings → Devices)'
