import { useEffect, useState } from 'react'
import { motion } from 'framer-motion'
import { AlertTriangle, X } from 'lucide-react'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { api, onFileDrop } from './lib/api'
import { setTaskbarProgress } from './lib/taskbar'
import { useStore } from './store'
import { TitleBar } from './components/TitleBar'
import { Sidebar } from './components/Sidebar'
import { Toasts } from './components/Toasts'
import { FolderInviteModal } from './components/FolderInviteModal'
import { BeamLogo } from './components/bits'
import { SendView } from './views/SendView'
import { SendToChooser } from './components/SendToChooser'
import { HistoryView } from './views/HistoryView'
import { SettingsView } from './views/SettingsView'
import { FoldersView } from './views/FoldersView'
import { FriendsView } from './views/FriendsView'
import { ChatView } from './views/ChatView'

export default function App() {
  const ready = useStore((s) => s.ready)
  const view = useStore((s) => s.view)
  const init = useStore((s) => s.init)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const setDragHovering = useStore((s) => s.setDragHovering)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const folderStatuses = useStore((s) => s.folderStatuses)

  useEffect(() => {
    init()
  }, [init])

  // Drive the Windows/Linux taskbar progress from the most relevant active
  // transfer (macOS shows this on the Downloads stack instead — no-op there).
  useEffect(() => {
    const active = order
      .map((id) => transfers[id])
      .filter(Boolean)
      .filter((t) => t.state === 'transferring')
    let pct: number | null = active.length ? active[active.length - 1].percent : null
    if (pct == null) {
      const f = Object.values(folderStatuses).find(
        (s) => s.state === 'sending' || s.state === 'receiving',
      )
      if (f) pct = f.percent
    }
    setTaskbarProgress(pct)
  }, [transfers, order, folderStatuses])

  useEffect(() => {
    let un: UnlistenFn | undefined
    let active = true
    onFileDrop(
      (paths) => {
        // On the Chat page with a conversation open, a dropped file/folder is
        // STAGED in the composer (iMessage-style, GitHub #23) — it waits as a chip
        // so you can add a message and send them together — instead of firing off
        // immediately or routing to the global send chooser.
        const st = useStore.getState()
        if (st.view === 'chat' && st.activeChatId) {
          st.stageChatFiles(paths)
        } else {
          setPendingSend(paths)
        }
      },
      (h) => setDragHovering(h),
    ).then((f) => {
      if (active) un = f
      else f()
    })
    return () => {
      active = false
      un?.()
    }
  }, [setPendingSend, setDragHovering])

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
      <InstallBanner />
      <LocalNetworkBanner />
      <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>
        <Sidebar />
        <main
          className="scroll-area"
          style={{
            flex: 1,
            minWidth: 0,
            // Chat fills the pane and scrolls its message list internally — don't
            // let the whole view scroll (which dragged the composer off-screen).
            overflowY: view === 'chat' ? 'hidden' : undefined,
          }}
        >
          {/* Keyed remount plays a mount-fade on view change. No exit/mode="wait"
              so it never deadlocks on a view that has its own AnimatePresence. */}
          <motion.div
            key={view}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.18 }}
            style={{ minHeight: '100%', height: view === 'chat' ? '100%' : undefined }}
          >
            {view === 'send' && <SendView />}
            {view === 'friends' && <FriendsView />}
            {view === 'chat' && <ChatView />}
            {view === 'folders' && <FoldersView />}
            {view === 'history' && <HistoryView />}
            {view === 'settings' && <SettingsView />}
          </motion.div>
        </main>
      </div>
      <SendToChooser />
      <NameSetupModal />
      <FolderInviteModal />
      <Toasts />
    </div>
  )
}

/** First-run: ask the user what name people should see, so they're not shown as a
 * device default like "MacBook Air". Pre-filled with the current name; shown once
 * (tracked in localStorage), and always changeable later in Settings. */
function NameSetupModal() {
  const settings = useStore((s) => s.settings)
  const save = useStore((s) => s.saveSettings)
  const [show, setShow] = useState(false)
  const [name, setName] = useState('')

  useEffect(() => {
    if (settings && !localStorage.getItem('dropbeam.namedSelf')) {
      setName(settings.displayName || '')
      setShow(true)
    }
  }, [settings])

  if (!show || !settings) return null
  const finish = () => {
    const trimmed = name.trim()
    if (trimmed && trimmed !== settings.displayName) save({ displayName: trimmed })
    localStorage.setItem('dropbeam.namedSelf', '1')
    setShow(false)
  }
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        display: 'grid',
        placeItems: 'center',
        background: 'color-mix(in srgb, black 45%, transparent)',
        backdropFilter: 'blur(3px)',
      }}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96, y: 8 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        style={{
          width: 380,
          maxWidth: '90vw',
          background: 'var(--bg-elev)',
          border: '1px solid var(--border)',
          borderRadius: 16,
          padding: 22,
          boxShadow: '0 20px 60px rgba(0,0,0,0.4)',
        }}
      >
        <div style={{ display: 'flex', justifyContent: 'center', marginBottom: 12 }}>
          <BeamLogo size={36} />
        </div>
        <h2 style={{ fontSize: 18, fontWeight: 750, textAlign: 'center', margin: '0 0 6px' }}>
          What should people call you?
        </h2>
        <p
          style={{
            fontSize: 13,
            color: 'var(--text-muted)',
            textAlign: 'center',
            margin: '0 0 16px',
            lineHeight: 1.45,
          }}
        >
          This is the name friends see when you send files or share a folder. You can change it
          anytime in Settings.
        </p>
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && finish()}
          placeholder="Your name"
          maxLength={40}
          style={{
            width: '100%',
            boxSizing: 'border-box',
            padding: '11px 13px',
            fontSize: 15,
            borderRadius: 10,
            border: '1px solid var(--border)',
            background: 'var(--bg)',
            color: 'var(--text)',
            marginBottom: 14,
          }}
        />
        <button
          className="btn btn-primary"
          style={{ width: '100%', justifyContent: 'center', padding: '11px' }}
          onClick={finish}
          disabled={!name.trim()}
        >
          Continue
        </button>
      </motion.div>
    </div>
  )
}

/** macOS "Local Network permission" nudge. We can't READ the permission state (no
 * API), so we detect the SYMPTOM in the engine — a peer is on the LAN but every
 * transfer falls back to the slow relay — and surface a one-click fix. Polls the
 * heuristic every 15s; dismissable per session. */
function LocalNetworkBanner() {
  const [blocked, setBlocked] = useState(false)
  const [dismissed, setDismissed] = useState(false)
  useEffect(() => {
    let alive = true
    const check = () => {
      api
        .lanNetworkBlocked()
        .then((b) => alive && setBlocked(b))
        .catch(() => {})
    }
    check()
    const id = setInterval(check, 15000)
    return () => {
      alive = false
      clearInterval(id)
    }
  }, [])
  if (!blocked || dismissed) return null
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '8px 14px',
        fontSize: 12.5,
        lineHeight: 1.4,
        background: 'var(--amber-soft)',
        color: 'var(--amber)',
        borderBottom: '1px solid var(--border)',
      }}
    >
      <AlertTriangle size={15} style={{ flexShrink: 0 }} />
      <span style={{ flex: 1 }}>
        DropBeam can’t reach a device on your network directly, so transfers are using a slow relay.
        Enable DropBeam under <b>Local Network</b> — and check it on the <b>other</b> device too.
      </span>
      <button
        className="btn btn-ghost"
        style={{ flexShrink: 0, padding: '4px 10px', fontSize: 12 }}
        onClick={() => api.openLocalNetworkSettings().catch(() => {})}
      >
        Open Settings
      </button>
      <button className="icon-btn" onClick={() => setDismissed(true)} title="Dismiss">
        <X size={15} />
      </button>
    </div>
  )
}

/** macOS: a sticky warning when the app is running from a spot that breaks folder
 * permissions every launch (translocation / Downloads). Dismissable per session. */
function InstallBanner() {
  const hint = useStore((s) => s.installHint)
  const [dismissed, setDismissed] = useState(false)
  if (!hint || dismissed) return null
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '8px 14px',
        fontSize: 12.5,
        lineHeight: 1.4,
        background: 'var(--amber-soft)',
        color: 'var(--amber)',
        borderBottom: '1px solid var(--border)',
      }}
    >
      <AlertTriangle size={15} style={{ flexShrink: 0 }} />
      <span style={{ flex: 1 }}>{hint}</span>
      <button className="icon-btn" onClick={() => setDismissed(true)} title="Dismiss">
        <X size={15} />
      </button>
    </div>
  )
}
