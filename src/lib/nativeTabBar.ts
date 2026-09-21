import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useStore } from '../store'
import { MOBILE_UI } from './platform'
import { nativeShellActive } from './nativeShell'
import { NATIVE_TABS, nativeTabBarModel } from './nativeTabBarModel'

export { nativeTabBarModel } from './nativeTabBarModel'

// App-lifetime singleton, including React StrictMode's repeated mount effect.
let installation: Promise<void> | undefined
let dispose: (() => void) | undefined

export async function installNativeTabBar(): Promise<void> {
  if (nativeShellActive) return
  if (!MOBILE_UI || typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return
  installation ??= install()
  return installation
}

async function install(): Promise<void> {
  let unlisten: UnlistenFn | undefined
  let unsubscribe: (() => void) | undefined
  let observer: MutationObserver | undefined
  let installed = false
  try {
    // Attach first so a tap immediately after installation cannot get lost.
    unlisten = await listen<number>('native-tab', ({ payload }) => {
      const view = NATIVE_TABS[payload]
      if (view) useStore.getState().setView(view)
    })
    await invoke<number>('native_tabbar_install')
    installed = true
    let previous: ReturnType<typeof nativeTabBarModel> | undefined
    let running = true
    const sync = () => {
      if (!running) return
      const next = nativeTabBarModel(useStore.getState(), document.documentElement.classList.contains('keyboard-visible'))
      const calls: Promise<unknown>[] = []
      if (next.index !== previous?.index) calls.push(invoke('native_tabbar_select', { index: next.index }))
      if (next.sendBadge !== previous?.sendBadge) calls.push(invoke('native_tabbar_badge', { index: 0, count: next.sendBadge }))
      if (next.chatBadge !== previous?.chatBadge) calls.push(invoke('native_tabbar_badge', { index: 2, count: next.chatBadge }))
      if (next.hidden !== previous?.hidden) calls.push(invoke('native_tabbar_hidden', { hidden: next.hidden }))
      previous = next
      // A failed bridge restores the DOM fallback and releases the native inset.
      return Promise.all(calls).catch(() => {
        running = false
        dispose?.()
        useStore.setState({ nativeTabBar: false }); document.documentElement.classList.remove('native-tabbar')
        void invoke('native_tabbar_hidden', { hidden: true }).catch(() => {})
      })
    }
    dispose = () => {
      running = false
      unlisten?.()
      unsubscribe?.()
      observer?.disconnect()
    }
    await sync()
    if (!running) return
    unsubscribe = useStore.subscribe(() => { void sync() })
    observer = new MutationObserver(() => { void sync() })
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
    useStore.setState({ nativeTabBar: true })
    document.documentElement.classList.add('native-tabbar')
  } catch {
    unlisten?.()
    unsubscribe?.()
    observer?.disconnect()
    useStore.setState({ nativeTabBar: false }); document.documentElement.classList.remove('native-tabbar')
    if (installed) void invoke('native_tabbar_hidden', { hidden: true }).catch(() => {})
  }
}

if (import.meta.hot) import.meta.hot.dispose(() => {
  dispose?.()
  useStore.setState({ nativeTabBar: false }); document.documentElement.classList.remove('native-tabbar')
  void invoke('native_tabbar_hidden', { hidden: true }).catch(() => {})
})
