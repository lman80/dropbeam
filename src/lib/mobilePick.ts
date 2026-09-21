import { api, type TransferUpdate } from './api'
import { useStore } from '../store'
import { withMobileFileSource } from '../components/MobileFileSheet'

let picking = false
/** Shared by the React fallback and SwiftUI: one picker and one send pipeline. */
export async function pickAndSend(source: 'photos' | 'files', friendId?: string) {
  if (picking) return
  picking = true
  try {
    const paths = await withMobileFileSource(source, source === 'photos' ? api.pickPhotos : api.pickFiles)
    if (paths.length) {
      if (friendId) await useStore.getState().sendToFriend(friendId, paths)
      else useStore.getState().setPendingSend(paths)
    }
  } finally {
    picking = false
    useStore.getState().setDragHovering(false)
  }
}

/** Reuse the retained original send paths, or the received destination paths. */
export function transferSharePaths(t: TransferUpdate): string[] {
  if (t.outDir) return t.fileNames.map(name => `${t.outDir}/${name}`)
  try {
    const cached = JSON.parse(localStorage.getItem('dropbeam-retry-payloads') || '{}')?.[t.id]?.paths
    return Array.isArray(cached) ? cached.filter((path): path is string => typeof path === 'string' && !!path) : []
  } catch { return [] }
}
