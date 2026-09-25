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
import { Sidebar } from './components/Sidebar'
import { MobileTabBar } from './components/MobileTabBar'
import { Toasts } from './components/Toasts'
import { ErrorBoundary } from './components/ErrorBoundary'
import { FolderInviteModal } from './components/FolderInviteModal'
import { SafetyDialogHost } from './components/SafetyDialogs'
import { QrScanner } from './components/QrScanner'
import { routeDropToScanner } from './lib/qrImage'
import { BeamLogo } from './components/bits'
import { IconButton, Spinner } from './components/ui'
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
      <div style={{ height: '100%', display: 'grid', placeItems: 'center' }} aria-busy="true" aria-label="Loading DropBeam">
        <Spinner size={18} style={{ animationDelay: '-0.2s' }} />
      </div>
    )
  }

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      {MOBILE_UI && <ErrorBoundary region="window controls">
        <InstallBanner />
        <LocalNetworkBanner />
      </ErrorBoundary>}
      <div className="app-workspace" style={{ flex: 1, display: 'flex', minHeight: 0 }}>
        {!MOBILE_UI && (
          <ErrorBoundary region="sidebar">
            <Sidebar />
          </ErrorBoundary>
        )}
        <div className="app-main-col" style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        {/* Notices sit at the top of the content column (the sidebar runs to the
            top of the window under the macOS traffic lights). */}
        {!MOBILE_UI && <ErrorBoundary region="window controls">
          <InstallBanner />
          <LocalNetworkBanner />
        </ErrorBoundary>}
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
              initial={MOBILE_UI ? false : { opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.12 }}
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

/** A device name ("Ashton's MacBook Pro", "DESKTOP-4F2K", "ubuntu") is not what
 *  people should call you — offer the person's first name when we can find it. */
function suggestedName(current: string): string {
  const name = current.trim()
  const possessive = name.match(/^(.+?)[’']s\s+(mac|macbook|imac|mac mini|mac studio|mac pro|pc|laptop|desktop|computer|iphone|ipad)\b/i)
  if (possessive) return possessive[1].trim()
  if (/^(desktop|laptop)-[a-z0-9]+$/i.test(name) || /^(macbook|imac|mac|ubuntu|localhost|pc)( (air|pro))?$/i.test(name)) return ''
  return name
}

function NameSetupModal() {
  const settings = useStore((s) => s.settings)
  const save = useStore((s) => s.saveSettings)
  const [show, setShow] = useState(false)
  const [name, setName] = useState('')
  const [joining, setJoining] = useState(false)

  useEffect(() => {
    if (settings && !localStorage.getItem('dropbeam.namedSelf')) {
      setName(suggestedName(settings.displayName || ''))
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
      <Dialog
        width={380}
        ariaLabel="Welcome to DropBeam"
        className="onboard-dialog"
        footer={
          <>
            {!MOBILE_UI && (
              <button className="btn btn-plain" onClick={() => setJoining(true)} style={{ marginLeft: -8 }}>
                Link an existing device…
              </button>
            )}
            <span className="spacer" />
            <button className="btn btn-primary" onClick={finish} disabled={!name.trim()}>
              Continue
            </button>
          </>
        }
      >
        <div className="onboard">
          <BeamLogo size={44} />
          <h2 className="onboard-title">Welcome to DropBeam</h2>
          <p className="onboard-text">
            {MOBILE_UI
              ? 'Choose the name friends see when you send them something.'
              : 'Choose the name friends see when you send files or share a folder.'}
          </p>
          <input
            autoFocus
            className="input onboard-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && name.trim() && finish()}
            placeholder="Your name"
            aria-label="Your name"
            maxLength={40}
          />
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
      <AlertTriangle />
      <span style={{ flex: 1, minWidth: 0 }}>
        {IS_MAC
          ? <>Transfers to devices nearby are slow. Allow DropBeam under <b>Local Network</b> on both devices.</>
          : <>Transfers to devices nearby are slow. Allow DropBeam through the firewall on private networks, on both devices.</>}
      </span>
      {IS_MAC && <button className="btn btn-secondary btn-sm" style={{ flexShrink: 0 }} onClick={() => api.openLocalNetworkSettings().catch(() => {})}>
        Open Settings
      </button>}
      <IconButton label="Dismiss" size="sm" onClick={() => setDismissed(true)}>
        <X />
      </IconButton>
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
      <AlertTriangle />
      <span style={{ flex: 1, minWidth: 0 }}>{hint}</span>
      <IconButton label="Dismiss" size="sm" onClick={() => setDismissed(true)}>
        <X />
      </IconButton>
    </div>
  )
}
