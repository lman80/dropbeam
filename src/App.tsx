import { useEffect, useLayoutEffect, useState } from 'react'
import { JoinAccountModal } from './components/DevicesPanel'
import { motion } from 'framer-motion'
import { AlertTriangle, X } from 'lucide-react'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { api, HAS_TAURI, onFileDrop } from './lib/api'
import { setTaskbarProgress } from './lib/taskbar'
import { useStore } from './store'
import { parseCode } from './lib/codes'
import { IS_MAC, MOBILE_UI } from './lib/platform'
import { startNativeBridge } from './lib/nativeBridge'
import { nativeShellActive } from './lib/nativeShell'
import { TitleBar } from './components/TitleBar'
import { Sidebar } from './components/Sidebar'
import { MobileTabBar } from './components/MobileTabBar'
import { Toasts } from './components/Toasts'
import { ErrorBoundary } from './components/ErrorBoundary'
import { FolderInviteModal } from './components/FolderInviteModal'
import { SafetyDialogHost } from './components/SafetyDialogs'
import { QrScanner } from './components/QrScanner'
import { routeDropToScanner } from './lib/qrImage'
import { BeamLogo } from './components/bits'
import { Dialog } from './components/Dialog'
import { SendView } from './views/SendView'
import { SendToChooser } from './components/SendToChooser'
import { HistoryView } from './views/HistoryView'
import { SettingsView } from './views/SettingsView'
import { LocationsView } from './views/LocationsView'
import { FoldersView } from './views/FoldersView'
import { FriendsView } from './views/FriendsView'
import { ChatView } from './views/ChatView'
import { MobileApp } from './mobile/MobileApp'
import { MobileOnboarding } from './mobile/Onboarding'

export default function App() {
  const [nativeShell, setNativeShell] = useState(false)
  const ready = useStore((s) => s.ready)
  const view = useStore((s) => s.view)
  const init = useStore((s) => s.init)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const setDragHovering = useStore((s) => s.setDragHovering)

  useEffect(() => {
    if (MOBILE_UI) void startNativeBridge().then(() => {
      setNativeShell(nativeShellActive)
    })
  }, [])

  useLayoutEffect(() => {
    if (!MOBILE_UI) return
    const root = document.documentElement
    const viewport = window.visualViewport
    let frame = 0
    let fullHeight = window.innerHeight
    let width = window.innerWidth
    const update = () => {
      // Reset the baseline on rotation; ignore small browser/accessory-bar changes.
      if (width !== window.innerWidth) {
        width = window.innerWidth
        fullHeight = window.innerHeight
      }
      fullHeight = Math.max(fullHeight, window.innerHeight)
      const height = viewport?.height ?? window.innerHeight
      const keyboard = !!viewport && viewport.scale === 1 && fullHeight - height > 150
      root.classList.toggle('keyboard-visible', keyboard)
      // WKWebView can pan the visual viewport even with overflow hidden.
      // Anchor the fixed shell to its visible origin; only inner panes scroll.
      root.style.setProperty('--mobile-viewport-height', `${viewport?.height ?? window.innerHeight}px`)
      root.style.setProperty('--mobile-viewport-top', `${viewport?.offsetTop ?? 0}px`)
    }
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(update)
    }
    update()
    viewport?.addEventListener('resize', schedule)
    viewport?.addEventListener('scroll', schedule)
    window.addEventListener('resize', schedule)
    return () => {
      cancelAnimationFrame(frame)
      viewport?.removeEventListener('resize', schedule)
      viewport?.removeEventListener('scroll', schedule)
      window.removeEventListener('resize', schedule)
      root.classList.remove('keyboard-visible')
      root.style.removeProperty('--mobile-viewport-height')
      root.style.removeProperty('--mobile-viewport-top')
    }
  }, [])

  useEffect(() => {
    init()
  }, [init])

  // Drive the Windows/Linux taskbar progress from the most relevant active
  // transfer (macOS shows this on the Downloads stack instead — no-op there).
  // A plain store SUBSCRIPTION, not useStore selectors: transfers/order change on
  // EVERY progress event, and root-level selectors re-rendered the entire app tree
  // per tick. The taskbar is a side effect — it needs no React render at all.
  useEffect(() => {
    const drive = (s: ReturnType<typeof useStore.getState>) => {
      const active = s.order
        .map((id) => s.transfers[id])
        .filter(Boolean)
        .filter((t) => t.state === 'transferring')
      let pct: number | null = active.length ? active[active.length - 1].percent : null
      if (pct == null) {
        const f = Object.values(s.folderStatuses).find(
          (st) => st.state === 'sending' || st.state === 'receiving',
        )
        if (f) pct = f.percent
      }
      setTaskbarProgress(pct)
    }
    drive(useStore.getState())
    return useStore.subscribe(drive)
  }, [])

  useEffect(() => {
    let un: UnlistenFn | undefined
    let active = true
    onFileDrop(
      (paths) => {
        // An open QR scanner takes the drop (a screenshot of a QR to decode).
        if (routeDropToScanner(paths)) return
        // On the Chat page with a conversation open, a dropped file/folder is
        // STAGED in the composer (iMessage-style, GitHub #23) — it waits as a chip
        // so you can add a message and send them together — instead of firing off
        // immediately or routing to the global send chooser.
        const st = useStore.getState()
        if (st.view === 'locations') {
          window.dispatchEvent(new CustomEvent('dropbeam:location-drop', { detail: paths }))
        } else if (st.view === 'chat' && st.activeChatId) {
          st.stageChatFiles(paths)
        } else {
          setPendingSend(paths)
        }
      },
      (h) => {
        setDragHovering(h)
        if (useStore.getState().view === 'locations') window.dispatchEvent(new CustomEvent('dropbeam:location-hover', { detail: h }))
      },
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
      <ErrorBoundary region="window controls">
        {!MOBILE_UI && IS_MAC && <TitleBar />}
        <InstallBanner />
        <LocalNetworkBanner />
      </ErrorBoundary>
      <div className="app-workspace" style={{ flex: 1, display: 'flex', minHeight: 0 }}>
        {!MOBILE_UI && (
          <ErrorBoundary region="sidebar">
            <Sidebar />
          </ErrorBoundary>
        )}


        <main
          className={MOBILE_UI ? `scroll-area mobile-main${view === 'chat' ? ' mobile-main-chat' : ''}` : "scroll-area"}
          style={{
            flex: 1,
            minWidth: 0,
            // Chat fills the pane and scrolls its message list internally — don't
            // let the whole view scroll (which dragged the composer off-screen).
            overflowY: view === 'chat' ? 'hidden' : undefined,
          }}
        >
          <ErrorBoundary region={`content:${view}`} key={view}>
            {/* Keyed remount plays a mount-fade on view change. No exit/mode="wait"
                so it never deadlocks on a view that has its own AnimatePresence. */}
            {MOBILE_UI ? <MobileApp bridgeOnly={nativeShell} /> : <motion.div
              initial={MOBILE_UI ? false : { opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.18 }}
              style={{ minHeight: '100%', height: view === 'chat' ? '100%' : undefined }}
            >
              {view === 'send' && <SendView />}
              {view === 'friends' && <FriendsView />}
              {view === 'chat' && <ChatView />}
              {view === 'folders' && <FoldersView />}
              {view === 'locations' && <LocationsView />}
              {view === 'history' && <HistoryView />}
              {view === 'settings' && <SettingsView />}
            </motion.div>}
          </ErrorBoundary>
        </main>
      </div>
      {MOBILE_UI && !nativeShell && (
        <ErrorBoundary region="mobile navigation">
          <MobileTabBar />
        </ErrorBoundary>
      )}
      <ErrorBoundary region="overlays" fallbackStyle={{ position: 'fixed', bottom: 12, left: 12, zIndex: 201 }}>
        {!nativeShell && <SendToChooser />}
        {MOBILE_UI ? (!nativeShell && <MobileOnboarding />) : <NameSetupModal />}
        {!nativeShell && <FolderInviteModal />}
        {!nativeShell && <SafetyDialogHost />}
        {!MOBILE_UI && <PopoverCodeHandoff />}
      </ErrorBoundary>
      <ErrorBoundary region="toasts" fallbackStyle={{ position: 'fixed', bottom: 12, right: 12, zIndex: 101 }}>
        <Toasts />
      </ErrorBoundary>
    </div>
  )
}

/** First-run: ask the user what name people should see, so they're not shown as a
 * device default like "MacBook Air". Pre-filled with the current name; shown once
 * (tracked in localStorage), and always changeable later in Settings. */
/** The menu-bar popover is a tiny, focus-sensitive panel (it hides when the
 *  camera prompt or a file picker takes focus), so it hands codes that need the
 *  full window — and QR scanning — to the main window. */
function PopoverCodeHandoff() {
  const openCode = useStore((s) => s.openCode)
  const [scanning, setScanning] = useState(false)
  useEffect(() => {
    if (!HAS_TAURI) return
    const uns: Promise<UnlistenFn>[] = [
      listen<string>('dropbeam://open-code', (e) => { if (typeof e.payload === 'string') void openCode(e.payload) }),
      listen('dropbeam://scan-code', () => setScanning(true)),
      // Tray-menu quick actions (the Linux tray has no popover, only this menu).
      listen<string>('dropbeam://tray-action', (e) => {
        const st = useStore.getState()
        const action = e.payload
        if (action === 'send') {
          st.setView('send')
          void api.pickFiles().then((paths) => { if (paths.length) st.setPendingSend(paths) }).catch((err) => st.toast('error', String(err)))
        } else if (action === 'friends' || action === 'chat' || action === 'folders' || action === 'settings') {
          st.setView(action)
        }
      }),
    ]
    return () => { uns.forEach((u) => void u.then((f) => f())) }
  }, [openCode])
  if (!scanning) return null
  return <QrScanner
    title="Scan a DropBeam code"
    hint="Hold the QR code up to your camera — a Quick Send, friend or folder code."
    validate={(text) => (parseCode(text) ? null : 'That QR code isn’t a DropBeam code.')}
    onClose={() => setScanning(false)}
    onResult={(text) => { setScanning(false); void openCode(text) }}
  />
}

function NameSetupModal() {
  const settings = useStore((s) => s.settings)
  const save = useStore((s) => s.saveSettings)
  const [show, setShow] = useState(false)
  const [name, setName] = useState('')
  const [joining, setJoining] = useState(false)

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
    <>
      <Dialog width={400} ariaLabel="Choose your name" className="onboard-dialog">
        <div style={{ display: 'flex', justifyContent: 'center', margin: '6px 0 14px' }}>
          <BeamLogo size={44} />
        </div>
        <h2 className="dialog-title" style={{ textAlign: 'center', marginBottom: 6 }}>
          What should people call you?
        </h2>
        <p className="dialog-text" style={{ textAlign: 'center', marginBottom: 18 }}>
          {MOBILE_UI
            ? 'Friends see this name in chats and when you send photos or files. You can change it anytime in Settings.'
            : 'This is the name friends see when you send files or share a folder. You can change it anytime in Settings.'}
        </p>
        {MOBILE_UI && <p style={{ lineHeight: 1.5, color: 'var(--text-muted)' }}>Keep DropBeam open while sending or receiving. Find received files in Files → On My iPhone → DropBeam.</p>}
        <input
          autoFocus
          className="input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && name.trim() && finish()}
          placeholder="Your name"
          aria-label="Your name"
          maxLength={40}
          style={{ fontSize: 'var(--font-md)', padding: '10px 13px' }}
        />
        <div className="dialog-actions" style={{ display: 'flex', flexDirection: 'column', gap: 8, paddingTop: 16 }}>
          <button className="btn btn-primary btn-lg btn-block" onClick={finish} disabled={!name.trim()}>
            Continue
          </button>
          {!MOBILE_UI && <button className="btn btn-quiet btn-block" onClick={() => setJoining(true)}>
            Already use DropBeam on another device? Link it
          </button>}
        </div>
      </Dialog>
      {joining && <JoinAccountModal onClose={() => setJoining(false)} onShowCode={() => setJoining(false)} />}
    </>
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
    <div className="app-banner warn" role="status">
      <AlertTriangle size={15} style={{ flexShrink: 0 }} />
      <span style={{ flex: 1 }}>
        DropBeam can’t reach a device on your network directly, so transfers are using a slow relay.
        {IS_MAC ? <> Enable DropBeam under <b>Local Network</b> — and check it on the <b>other</b> device too.</>
          : <> Check that your firewall allows DropBeam on private networks — on <b>both</b> devices.</>}
      </span>
      {IS_MAC && <button className="btn btn-ghost btn-sm" style={{ flexShrink: 0 }} onClick={() => api.openLocalNetworkSettings().catch(() => {})}>
        Open Settings
      </button>}
      <button className="icon-btn icon-btn-sm" onClick={() => setDismissed(true)} title="Dismiss" aria-label="Dismiss">
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
    <div className="app-banner warn" role="status">
      <AlertTriangle size={15} style={{ flexShrink: 0 }} />
      <span style={{ flex: 1 }}>{hint}</span>
      <button className="icon-btn icon-btn-sm" onClick={() => setDismissed(true)} title="Dismiss" aria-label="Dismiss">
        <X size={15} />
      </button>
    </div>
  )
}
