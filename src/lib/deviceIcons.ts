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
