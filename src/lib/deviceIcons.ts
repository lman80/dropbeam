import { Smartphone, Tablet, Laptop, Monitor, type LucideIcon } from 'lucide-react'

export function deviceIcon(kind?: string): LucideIcon {
  switch (kind) {
    case 'phone': return Smartphone
    case 'tablet': return Tablet
    case 'laptop': return Laptop
    default: return Monitor
  }
}

export function deviceKindLabel(kind?: string): string {
  switch (kind) {
    case 'phone': return 'Phone'
    case 'tablet': return 'Tablet'
    case 'laptop': return 'Laptop'
    case 'desktop': return 'Desktop'
    default: return 'Computer'
  }
}

export function groupDevices<T extends { accountPub?: string | null }>(friends: readonly T[], accountPub?: string | null): { myDevices: T[]; others: T[] } {
  const myDevices: T[] = [], others: T[] = []
  for (const friend of friends) (accountPub && friend.accountPub === accountPub ? myDevices : others).push(friend)
  return { myDevices, others }
}

/** What the device is, in the words Apple uses: "iPhone", "Mac", "PC"… */
export function deviceNoun(kind?: string | null, os?: string | null): string {
  if (os === 'ios') return kind === 'tablet' ? 'iPad' : 'iPhone'
  if (os === 'macos') return 'Mac'
  if (os === 'windows') return 'PC'
  if (os === 'linux') return 'Linux PC'
  if (os === 'android') return kind === 'tablet' ? 'Tablet' : 'Phone'
  return kind === 'phone' ? 'Phone' : kind === 'tablet' ? 'Tablet' : 'Computer'
}

/**
 * Labels for the user's OWN devices, Blip-style: "Your Mac", "Your iPhone".
 * When two devices would read the same ("Your iPhone" twice), each says its
 * model instead ("Your iPhone 15" / "Your iPhone 12", "Your MacBook Air" /
 * "Your Mac mini"). Devices still alike (same model, or an older build that
 * doesn't say) use their device names when those tell them apart, else
 * "Your iPhone" / "Your iPhone (2)" in a stable order.
 * Mirrored in DevicesView.swift (ownDeviceLabels).
 */
export function ownDeviceLabels<T extends { id: string; name: string; deviceKind?: string | null; deviceOs?: string | null; deviceModel?: string | null }>(devices: readonly T[]): Record<string, string> {
  const nouns = devices.map(d => deviceNoun(d.deviceKind, d.deviceOs))
  // Step 1: the plain noun, or — when that's shared — the model, if known.
  const first = devices.map((d, i) => {
    if (nouns.filter(n => n === nouns[i]).length === 1) return `Your ${nouns[i]}`
    const model = d.deviceModel?.trim()
    return `Your ${model || nouns[i]}`
  })
  const out: Record<string, string> = {}
  devices.forEach((d, i) => {
    const same = devices.filter((_, j) => first[j] === first[i])
    if (same.length === 1) { out[d.id] = first[i]; return }
    // Step 2: device names, when they tell these apart (iPhones all say "iPhone").
    const names = same.map(x => x.name.trim())
    const taken = new Set(first.filter((_, j) => first[j] !== first[i]))
    const tellable = new Set(names).size === names.length && names.every(n => n && n !== nouns[i] && !taken.has(n))
    if (tellable) { out[d.id] = d.name.trim(); return }
    // Step 3: number them, Finder-style.
    const n = [...same].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0).indexOf(d) + 1
    out[d.id] = n === 1 ? first[i] : `${first[i]} (${n})`
  })
  return out
}

/**
 * A friend's extra devices (records sharing someone ELSE's verified account)
 * fold into one record of that person — the one that owns the conversation
 * (mirrors friends::owner_in in Rust). Returns member id → owner id, for the
 * extra devices only.
 */
export function personGroups<T extends { id: string; createdAt: number; accountPub?: string | null; endpointId?: string | null }>(friends: readonly T[], myAccount?: string | null): Record<string, string> {
  const owners = new Map<string, T>()
  for (const f of friends) {
    if (!f.accountPub || f.accountPub === myAccount || !f.endpointId) continue
    // Smallest endpoint id: the same choice on every device (see Rust owner_in).
    const cur = owners.get(f.accountPub)
    if (!cur || f.endpointId < (cur.endpointId ?? '')) owners.set(f.accountPub, f)
  }
  const out: Record<string, string> = {}
  for (const f of friends) {
    const owner = f.accountPub && f.accountPub !== myAccount ? owners.get(f.accountPub) : undefined
    if (owner && owner.id !== f.id) out[f.id] = owner.id
  }
  return out
}

/**
 * Who you can send to, one entry per PERSON: your own devices (each its own
 * entry — "Your iPhone", "Your Mac") and every friend once, however many
 * devices they have (their extra devices fold into the record that owns the
 * conversation, and a send to it reaches all of them).
 */
export function sendTargets<T extends { id: string; createdAt: number; accountPub?: string | null; endpointId?: string | null }>(friends: readonly T[], myAccount?: string | null): { myDevices: T[]; others: T[] } {
  const grouped = personGroups(friends, myAccount)
  return groupDevices(friends.filter((f) => !grouped[f.id]), myAccount)
}

/** The stable id a person's avatar colour is keyed by: the record that owns
 *  their conversation (so every device of one friend shares one colour). */
export function personKey<T extends { id: string; createdAt: number; accountPub?: string | null; endpointId?: string | null }>(friends: readonly T[], id: string, myAccount?: string | null): string {
  return personGroups(friends, myAccount)[id] ?? id
}
