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
