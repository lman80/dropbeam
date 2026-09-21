import type { SyncedFolder, SyncedFolderStatus } from './api.ts'

/**
 * Pure helpers behind "Synced to a location" — the plain-language wording of a
 * folder's card. Kept out of the view so they can be tested without a DOM.
 *
 * Every string here is written for someone who has never heard the words
 * "reconcile", "upload queue" or "backoff": a folder is either up to date, being
 * looked at, copying, waiting for a device that's asleep, paused, or stuck.
 */

export type Pill = { tone: 'ok' | 'busy' | 'wait' | 'off' | 'bad'; label: string }

/** The status pill for one synced folder. `status` is absent until the engine reports. */
export function statusPill(folder: SyncedFolder, status?: SyncedFolderStatus): Pill {
  if (!folder.enabled) return { tone: 'off', label: 'Paused' }
  const state = status?.state
  const message = status?.message?.trim() || ''
  switch (state) {
    case 'uploading': {
      const n = status?.pendingFiles ?? 0
      return { tone: 'busy', label: n > 0 ? `Copying ${n} file${n === 1 ? '' : 's'}` : 'Copying' }
    }
    case 'scanning':
      return { tone: 'busy', label: 'Checking for new files' }
    case 'waiting':
      return { tone: 'wait', label: message || 'Waiting for the other device' }
    case 'paused':
      return { tone: 'off', label: 'Paused' }
    case 'error':
      return { tone: 'bad', label: message || "Something went wrong" }
    case 'idle':
      return { tone: 'ok', label: message || 'Up to date' }
    default:
      // No word from the engine yet — fall back to what we last wrote down.
      if (folder.lastResult && !folder.lastResult.ok) return { tone: 'bad', label: folder.lastResult.message }
      return { tone: 'wait', label: folder.lastCheckAt ? 'Up to date' : 'Getting ready…' }
  }
}

/** "Travel → Buddy NAS › Photos/Travel", or just the location when it lands at the top. */
export function destinationLabel(locationName: string, relPath: string): string {
  const clean = relPath.replace(/^\/+|\/+$/g, '')
  return clean ? `${locationName} › ${clean}` : locationName
}

/** The local folder's own name, without dragging the whole path across the card. */
export function folderName(localPath: string): string {
  const parts = localPath.replace(/[/\\]+$/, '').split(/[/\\]/)
  return parts[parts.length - 1] || localPath
}

/**
 * The one-sentence promise shown on the confirm step — the whole feature in the
 * user's own words, including whether deletes travel.
 */
export function summarySentence(localPath: string, locationName: string, relPath: string, deleteRemote: boolean): string {
  const here = folderName(localPath)
  const there = destinationLabel(locationName, relPath)
  const deletes = deleteRemote
    ? `Files you delete here are moved to ${locationName}'s trash too.`
    : `Files you delete here stay on ${locationName}.`
  return `Everything you put in “${here}” on this device will be copied to ${there}. ${deletes}`
}
