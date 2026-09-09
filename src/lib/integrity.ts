import type { FileIntegrity } from './api'

/** Older IPC events and caches have no integrity information. */
export function integrityRows(value: unknown): FileIntegrity[] {
  if (!Array.isArray(value)) return []
  return value.filter((r): r is FileIntegrity => r && typeof r.name === 'string'
    && Number.isSafeInteger(r.size) && r.size >= 0 && typeof r.algorithm === 'string'
    && typeof r.digest === 'string' && /^[0-9a-f]{64}$/.test(r.digest)
    && typeof r.peerDigest === 'string' && /^[0-9a-f]{64}$/.test(r.peerDigest)
    && (r.index === undefined || (Number.isSafeInteger(r.index) && r.index >= 0))
    && (r.acknowledged === undefined || typeof r.acknowledged === 'boolean')
    && typeof r.verified === 'boolean' && r.verified === (r.digest === r.peerDigest))
}

export function mergeIntegrity(previous: FileIntegrity[] = [], current: FileIntegrity[] = []): FileIntegrity[] {
  // Legacy rows lack an index: preserve their ordered occurrences as well.
  const identify = (rows: FileIntegrity[]) => rows.map((r, occurrence) =>
    [JSON.stringify([r.index ?? `legacy:${occurrence}`, r.name]), r] as const)
  return [...new Map([...identify(previous), ...identify(current)]).values()]
}
