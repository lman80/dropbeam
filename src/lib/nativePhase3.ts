import type { HistoryEntry, LocationPage, SharedLocation } from './api'

/** Location cursors are opaque. The UI receives only the NEXT cursor. */
export function nativeBrowserPage(page: LocationPage) {
  return { entries: page.entries, hasMore: page.hasMore, cursor: page.nextCursor ?? null, total: page.total ?? page.entries.length }
}
export function locationChild(path: string, name: string): string {
  if (!name.trim() || name === '.' || name === '..' || /[/\\\0]/.test(name)) throw new Error('Enter a single file or folder name.')
  return path ? `${path}/${name}` : name
}
export function requireLocationRight(location: SharedLocation | undefined, right: 'upload' | 'manage' | 'read') {
  if (!location || (right !== 'read' && !location.rights[right])) throw new Error('This location no longer permits that action. Refresh Locations.')
}
export function nativeHistoryPaths(entry: HistoryEntry): string[] {
  if (!entry.outDir || entry.state !== 'completed') return []
  return entry.fileNames.filter(n => n && !n.startsWith('/') && !n.split(/[/\\]/).includes('..')).map(n => `${entry.outDir}/${n}`)
}
