// Desktop `dropbeam:` links (a link clicked in a browser or chat app, or a
// second launch carrying one). The engine queues them (take_open_urls) and pings
// `open-url://incoming`; each goes through openCode — the SAME describe-and-
// confirm path as a pasted or scanned code, so a link never acts on its own.
// iOS has its own route through the native bridge (nativeBridge.ts).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { HAS_TAURI } from './api'
import { MOBILE_UI } from './platform'
import { useStore } from '../store'

export function startDesktopDeepLinks(): () => void {
  if (!HAS_TAURI || MOBILE_UI) return () => {}
  let stopped = false
  let busy = Promise.resolve()
  const take = () => {
    if (!useStore.getState().ready) return // taken once the store is ready
    // Serialize: one link's confirm flow finishes before the next starts.
    busy = busy.then(async () => {
      const urls = await invoke<string[]>('take_open_urls').catch(() => [] as string[])
      for (const url of urls) {
        if (stopped) return
        await useStore.getState().openCode(url).catch(() => false)
      }
    })
  }
  const unlisten = listen('open-url://incoming', take)
  // Links that cold-started the app were queued before this window listened.
  // Wait for the store to finish loading friends/settings first.
  const unsubscribe = useStore.subscribe((s, prev) => {
    if (s.ready && !prev.ready) take()
  })
  if (useStore.getState().ready) take()
  return () => {
    stopped = true
    unsubscribe()
    void unlisten.then((un) => un())
  }
}
