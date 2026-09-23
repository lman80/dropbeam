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
 * When two devices would read the same ("Your Mac" twice), both fall back to
 * their device names so they stay tellable apart.
 */
export function ownDeviceLabels<T extends { id: string; name: string; deviceKind?: string | null; deviceOs?: string | null }>(devices: readonly T[]): Record<string, string> {
  const nouns = devices.map(d => deviceNoun(d.deviceKind, d.deviceOs))
  const out: Record<string, string> = {}
  devices.forEach((d, i) => { out[d.id] = nouns.filter(n => n === nouns[i]).length > 1 ? d.name : `Your ${nouns[i]}` })
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
