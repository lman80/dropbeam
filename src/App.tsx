import { useEffect } from 'react'
import { motion } from 'framer-motion'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { onFileDrop } from './lib/api'
import { useStore } from './store'
import { TitleBar } from './components/TitleBar'
import { Sidebar } from './components/Sidebar'
import { Toasts } from './components/Toasts'
import { BeamLogo } from './components/bits'
import { SendView } from './views/SendView'
import { HistoryView } from './views/HistoryView'
import { SettingsView } from './views/SettingsView'
import { FoldersView } from './views/FoldersView'

export default function App() {
  const ready = useStore((s) => s.ready)
  const view = useStore((s) => s.view)
  const init = useStore((s) => s.init)
  const sendPaths = useStore((s) => s.sendPaths)
  const setDragHovering = useStore((s) => s.setDragHovering)

  useEffect(() => {
    init()
  }, [init])

  useEffect(() => {
    let un: UnlistenFn | undefined
    let active = true
    onFileDrop(
      (paths) => sendPaths(paths),
      (h) => setDragHovering(h),
    ).then((f) => {
      if (active) un = f
      else f()
    })
    return () => {
      active = false
      un?.()
    }
  }, [sendPaths, setDragHovering])

  if (!ready) {
    return (
      <div style={{ height: '100%', display: 'grid', placeItems: 'center' }}>
        <div className="animate-beam">
          <BeamLogo size={46} />
        </div>
      </div>
    )
  }

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      <TitleBar />
      <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>
        <Sidebar />
        <main className="scroll-area" style={{ flex: 1, minWidth: 0 }}>
          {/* Keyed remount plays a mount-fade on view change. No exit/mode="wait"
              so it never deadlocks on a view that has its own AnimatePresence. */}
          <motion.div
            key={view}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.18 }}
            style={{ minHeight: '100%' }}
          >
            {view === 'send' && <SendView />}
            {view === 'folders' && <FoldersView />}
            {view === 'history' && <HistoryView />}
            {view === 'settings' && <SettingsView />}
          </motion.div>
        </main>
      </div>
      <Toasts />
    </div>
  )
}
