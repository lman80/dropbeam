// Thin wrapper around tauri-plugin-updater + process. Dynamically imported so it
// never loads (or errors) in the browser preview, where there's no Tauri runtime.

import { HAS_TAURI } from './api'

export interface UpdateInfo {
  version: string
  currentVersion: string
  notes: string
}

// The Update object returned by check() — kept so install() can use it later.
let pending: { downloadAndInstall: (cb: (e: UpdateEvent) => void) => Promise<void> } | null = null

interface UpdateEvent {
  event: 'Started' | 'Progress' | 'Finished'
  data?: { contentLength?: number; chunkLength?: number }
}

export async function appVersion(): Promise<string> {
  if (!HAS_TAURI) return '0.1.0'
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return await getVersion()
  } catch {
    return '0.1.0'
  }
}

/** Returns update info if a newer version is available, else null. */
export async function checkUpdate(): Promise<UpdateInfo | null> {
  if (!HAS_TAURI) return null
  const { check } = await import('@tauri-apps/plugin-updater')
  const update = await check()
  if (!update) {
    pending = null
    return null
  }
  pending = update as unknown as typeof pending
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    notes: update.body ?? '',
  }
}

/** Download + install the pending update, reporting 0–100% progress, then relaunch. */
export async function installUpdate(onProgress?: (pct: number) => void): Promise<void> {
  if (!HAS_TAURI || !pending) return
  let total = 0
  let got = 0
  await pending.downloadAndInstall((e: UpdateEvent) => {
    if (e.event === 'Started') {
      total = e.data?.contentLength ?? 0
      onProgress?.(0)
    } else if (e.event === 'Progress') {
      got += e.data?.chunkLength ?? 0
      if (total > 0) onProgress?.(Math.min(99, Math.round((got / total) * 100)))
    } else if (e.event === 'Finished') {
      onProgress?.(100)
    }
  })
  const { relaunch } = await import('@tauri-apps/plugin-process')
  await relaunch()
}
