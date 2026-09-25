import { deviceKindLabel } from '../lib/deviceIcons'
import { LinkDeviceModal, LinkNewDeviceModal } from '../components/LinkDeviceModal'
import { DevicesPanel } from '../components/DevicesPanel'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { FolderOpen, Info, QrCode, ScanLine } from 'lucide-react'
import { QrCodeView } from '../components/CodeQr'
import { QrScanner } from '../components/QrScanner'
import { api, type Settings } from '../lib/api'
import { contactMailto, platformLabel } from '../lib/report'
import { formatBytes } from '../lib/format'
import { folderLabel } from '../lib/humanize'
import { useStore, type View } from '../store'
import { IS_MAC, IS_WINDOWS, MOBILE_UI, TRAY_NAME } from '../lib/platform'
import { MobileHeader } from '../components/MobileHeader'
import { LocationSettings } from '../components/LocationSettings'
import { Dot, IconButton, InfoButton, ProgressBar, SectionHeader, Segmented, Spinner, Toggle } from '../components/ui'

// ── Tabs ─────────────────────────────────────────────────────────────────────
// Settings is split into panes (like a Mac preferences window) instead of one
// long scroll. The pane you land on follows where you came from — "Add a
// location" on Locations opens the Locations pane, the GIF setup link in Chat
// opens General, "Settings" under History's recoverable files opens Transfers —
// otherwise it's the pane you last had open.
type Tab = 'general' | 'devices' | 'locations' | 'transfers' | 'privacy' | 'advanced'
const TABS: { value: Tab; label: string }[] = [
  { value: 'general', label: 'General' },
  { value: 'devices', label: 'Devices' },
  { value: 'locations', label: 'Locations' },
  { value: 'transfers', label: 'Transfers' },
  { value: 'privacy', label: 'Privacy' },
  { value: 'advanced', label: 'Advanced' },
]
let lastTab: Tab = 'general'
let cameFrom: View | null = null
useStore.subscribe((s, prev) => {
  if (s.view === 'settings' && prev.view !== 'settings') cameFrom = prev.view
})
function initialTab(): Tab {
  const from = cameFrom
  cameFrom = null
  if (from === 'locations') return 'locations'
  if (from === 'chat') return 'general'
  if (from === 'history') return 'transfers'
  return lastTab
}

// The relay URL and detailed logging only apply on the next launch. Remember
// what this launch started with, so "Restart" appears only after a real change.
let bootRelay: string | null = null
let bootVerbose: boolean | null = null
const captureBoot = (st: Settings | null | undefined) => {
  if (!st || bootRelay !== null) return
  bootRelay = st.customRelay
  bootVerbose = st.verboseLogging
}
captureBoot(useStore.getState().settings)
useStore.subscribe((s) => captureBoot(s.settings))

const RELAY_GUIDE = 'https://github.com/lman80/dropbeam/blob/main/RELAY-SETUP.md'

// ── Rows ─────────────────────────────────────────────────────────────────────
function Row({ title, sub, children }: { title: ReactNode; sub?: ReactNode; children?: ReactNode }) {
  return (
    <div className="row set-row">
      <div className="row-main">
        <div className="row-title">{title}</div>
        {sub && <div className="row-sub">{sub}</div>}
      </div>
      {children && <div className="row-trailing">{children}</div>}
    </div>
  )
}

function ToggleRow({
  title,
  sub,
  on,
  onChange,
  disabled,
  extra,
}: {
  title: string
  sub?: ReactNode
  on: boolean
  onChange: (v: boolean) => void
  disabled?: boolean
  /** Shown before the switch (e.g. a Restart button once a change needs one). */
  extra?: ReactNode
}) {
  return (
    <Row title={title} sub={sub}>
      {extra}
      <Toggle on={on} onChange={onChange} label={title} disabled={disabled} />
    </Row>
  )
}

function InfoTip({ label, children, width = 280 }: { label: string; children: ReactNode; width?: number }) {
  return (
    <InfoButton label={label} icon={<Info />} width={width} align="start" className="set-info-btn">
      <div className="set-info">{children}</div>
    </InfoButton>
  )
}

export function SettingsView() {
  const settings = useStore((s) => s.settings) as Settings
  const save = useStore((s) => s.saveSettings)
  const myEid = useStore((s) => s.myEid)
  const [showEidQr, setShowEidQr] = useState(false)
  const [scanOperator, setScanOperator] = useState(false)
  const appVer = useStore((s) => s.appVer)
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<string | null>(null)
  const [historyUsage, setHistoryUsage] = useState<number | null>(null)
  const [freeingHistory, setFreeingHistory] = useState(false)
  const [confirmFree, setConfirmFree] = useState(false)
  const [tab, setTabState] = useState<Tab>(initialTab)
  const rootRef = useRef<HTMLDivElement>(null)

  const setTab = (t: Tab) => {
    lastTab = t
    setTabState(t)
    // Each pane starts at its top.
    rootRef.current?.closest('.scroll-area')?.scrollTo({ top: 0 })
  }

  // Total disk used by recoverable copies across all shared folders.
  useEffect(() => {
    let alive = true
    api
      .folderHistorySummary()
      .then((sums) => {
        if (alive) setHistoryUsage(sums.reduce((s, f) => s + f.bytes, 0))
      })
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [settings.folderHistoryKeepDays, settings.folderHistoryBudgetBytes])

  const changeDir = async () => {
    const d = await api.pickDirectory()
    if (d) save({ downloadDir: d })
  }

  const myDevice = useStore(s => s.myDevice)
  const [linkMode, setLinkMode] = useState<'new' | 'this' | null>(null)
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(e => useStore.getState().toast('error', String(e))) }, [])
  const deviceModals = <>{linkMode === 'new' && <LinkNewDeviceModal onClose={() => setLinkMode(null)} />}{linkMode === 'this' && <LinkDeviceModal onClose={() => setLinkMode(null)} />}</>
  const deviceDescription = myDevice ? `This device: ${settings.displayName || myDevice.name} · ${deviceKindLabel(myDevice.device_kind)}` : 'Loading this device…'
  const linkedDescription = myDevice ? `${myDevice.linked_devices} linked device${myDevice.linked_devices === 1 ? '' : 's'}` : ''
  const toast = useStore((s) => s.toast)
  const [clearing, setClearing] = useState(false)
  const clearCache = async () => {
    setClearing(true)
    try {
      const freed = await api.clearTransferCache()
      toast(
        'success',
        freed > 0
          ? `Cleared ${formatBytes(freed)} of transfer leftovers.`
          : 'Nothing to clear — no transfer leftovers found.',
      )
    } catch (e) {
      toast('error', String(e))
    } finally {
      setClearing(false)
    }
  }

  const freeHistory = async () => {
    setFreeingHistory(true)
    try {
      const freed = await api.clearAllFolderHistory()
      setHistoryUsage(0)
      toast(
        'success',
        freed > 0
          ? `Freed ${formatBytes(freed)} of recoverable copies.`
          : 'Nothing to free — no recoverable copies right now.',
      )
    } catch (e) {
      toast('error', String(e))
    } finally {
      setFreeingHistory(false)
      setConfirmFree(false)
    }
  }

  const [exporting, setExporting] = useState(false)
  const [testingDiag, setTestingDiag] = useState(false)
  const exportLogs = async () => {
    setExporting(true)
    try {
      const path = await api.exportDiagnostics()
      if (!MOBILE_UI) await api.revealPath(path).catch(() => {})
      toast('success', 'Logs exported to Downloads — send it over DropBeam or AirDrop to get it diagnosed.')
    } catch (e) {
      toast('error', String(e))
    } finally {
      setExporting(false)
    }
  }

  const runDirectTest = async () => {
    setTesting(true)
    setTestResult(null)
    try {
      setTestResult(await api.irohSelftest())
    } catch (e) {
      setTestResult(`Not ready yet — ${String(e)}`)
    } finally {
      setTesting(false)
    }
  }

  if (MOBILE_UI) {
    const toggle = (key: keyof Settings, label: string, desc: string, disabled = false) => <MobileSetting key={key} title={label} desc={desc}><button className={`toggle${settings[key] && !disabled ? ' on' : ''}`} role="switch" aria-label={label} aria-checked={!!settings[key] && !disabled} disabled={disabled} onClick={() => save({ [key]: !settings[key] })} /></MobileSetting>
    const field = (key: 'displayName' | 'giphyApiKey' | 'customRelay' | 'diagnosticsUrl' | 'labOperatorId', label: string, placeholder = '') => <input className="input" aria-label={label} autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" placeholder={placeholder} value={settings[key]} onChange={e => save({ [key]: e.target.value })} />
    return <div className="mobile-page mobile-settings">
      <MobileHeader title="Settings" />
      {deviceModals}
      <MobileSection title="Devices"><MobileSetting title={deviceDescription} desc={linkedDescription} /><div className="ios-row"><button className="ios-button" onClick={() => setLinkMode('new')}>Link a new device</button></div><div className="ios-row"><button className="ios-button" onClick={() => setLinkMode('this')}>Link this device to my account</button></div></MobileSection>
      <MobileSection title="Profile"><MobileSetting title="Display name" desc="The name your friends see.">{field('displayName', 'Display name')}</MobileSetting></MobileSection>
      <MobileSection title="Downloads"><MobileSetting title="Clear transfer cache" desc="Remove interrupted transfer leftovers. Old leftovers are cleaned automatically after a week." destructive><button className="ios-button ios-destructive" disabled={clearing} onClick={clearCache}>{clearing ? <Spinner size={18} /> : 'Clear now'}</button></MobileSetting></MobileSection>
      <MobileSection title="Appearance"><MobileSetting title="Theme"><div className="mobile-theme" role="group" aria-label="Theme">{(['system', 'light', 'dark'] as const).map(t => <button key={t} aria-pressed={settings.theme === t} onClick={() => save({ theme: t })}>{t}</button>)}</div></MobileSetting></MobileSection>
      <MobileSection title="Behavior">
        {toggle('notifyOnComplete', 'File notifications', 'Notify when files arrive. Keep DropBeam open while transferring.')}
        {toggle('notifyOnMessage', 'Chat notifications', 'Notify outside the conversation. Delivery may pause in the background.')}
        {toggle('sendReadReceipts', 'Read receipts', 'Let friends see when you have read their message.')}
        <MobileSetting title="Giphy key" desc="Add a free key from developers.giphy.com to enable GIFs. Leave blank to hide the picker.">{field('giphyApiKey', 'Giphy API key')}</MobileSetting>
        {toggle('playSounds', 'Play sounds', 'Soft cues for sends, receives and file offers.')}
      </MobileSection>
      <MobileSection title="Connection">
        <MobileSetting title="Direct peer-to-peer" desc="End-to-end encrypted. Keep both devices open until transfers finish."><span className="mobile-accent">On</span></MobileSetting>
        <MobileSetting title="Test connection" desc={testResult || 'Check the connection on this device.'}><button className="ios-button" disabled={testing} onClick={runDirectTest}>{testing ? <Spinner size={18} /> : 'Test'}</button></MobileSetting>
        <MobileSetting title="Local network access" desc="If nearby transfers use the relay, enable DropBeam in iOS Settings → Privacy & Security → Local Network on both devices." />
        {toggle('requireDirect', 'Direct connections only', 'Fail the send if a direct path cannot be made. Shared folders use the best available path.')}
        {toggle('waitForDirect', 'Wait for direct connection', settings.requireDirect ? 'Unavailable while direct connections are required.' : 'Wait for a fast direct path before sending. You can choose the relay on each transfer.', settings.requireDirect)}
        {toggle('parallelStreams', 'Parallel streams', 'Send files over 16 MB using several connections. Turn off if transfers stall.')}
        <MobileSetting title="Upload limit" desc="Mbps; 0 means unlimited. Local transfers run at full speed. Start at 100 Mbps and adjust if your Wi-Fi stutters."><input className="input" aria-label="Upload limit in Mbps" type="number" min={0} max={100000} value={settings.uploadLimitMbps || 0} onChange={e => save({ uploadLimitMbps: Math.max(0, Math.floor(Number(e.target.value) || 0)) })} /></MobileSetting>
        {toggle('showMegabits', 'Speeds in megabits', 'Use Mbps instead of kB/s or MB/s on this device.')}
      </MobileSection>
      <MobileSection title="How transfers connect">
        <MobileSetting title="Local" desc="Same Wi-Fi or network. Files travel directly across your network, without the internet." />
        <MobileSetting title="Direct" desc="An encrypted peer-to-peer link across the internet connects both devices." />
        <MobileSetting title="Relay" desc="If a direct path is unavailable, an encrypted relay carries files. It cannot read them, but may be slower." />
        <MobileSetting title="Connecting" desc="Finding the best available route to the other device." />
      </MobileSection>
      <MobileSection title="Custom relay (advanced)"><MobileSetting title="Relay URL" desc="Use the same relay on both devices; leave blank for public relays. Close and reopen DropBeam to apply. Setup: github.com/lman80/dropbeam → RELAY-SETUP.md">{field('customRelay', 'Relay URL', 'https://…')}</MobileSetting></MobileSection>
      <MobileSection title="About"><MobileSetting title="Version"><span>{appVer || '…'}</span></MobileSetting></MobileSection>
      <MobileSection title="Diagnostics">
        {toggle('verboseLogging', 'Detailed logging', 'Extra network logs for reproducing issues. Close and reopen DropBeam to apply.')}
        <MobileSetting title="Export logs" desc="Save diagnostics in DropBeam’s folder in Files to share for troubleshooting."><button className="ios-button" disabled={exporting} onClick={exportLogs}>{exporting ? <Spinner size={18} /> : 'Export'}</button></MobileSetting>
        {toggle('shareDiagnostics', 'Background diagnostics', 'Send a redacted error and performance summary about once a day. Never includes file names or contents.')}
        {settings.shareDiagnostics && <>
          <MobileSetting title="Diagnostics endpoint" desc="Leave blank for the built-in collector. Override only if you run your own.">{field('diagnosticsUrl', 'Diagnostics endpoint', 'Built-in')}</MobileSetting>
          <MobileSetting title="Test diagnostics"><button className="ios-button" disabled={testingDiag || (settings.diagnosticsUrl !== '' && !settings.diagnosticsUrl.startsWith('https://'))} onClick={async () => { setTestingDiag(true); try { toast('info', await api.diagnosticsTest()) } catch (e) { toast('error', String(e)) } finally { setTestingDiag(false) } }}>{testingDiag ? <Spinner size={18} /> : 'Send test'}</button></MobileSetting>
        </>}
      </MobileSection>
      <MobileSection title="Lab Mode">
        {toggle('labModeEnabled', 'Enable Lab Mode', 'Allow one trusted developer device to run encrypted diagnostics. Only the operator ID below is accepted. Enable only when asked by the developer.')}
        {settings.labModeEnabled && <>
          <MobileSetting title="Operator ID" desc="Only this device can run Lab Mode. Blank accepts no device.">{field('labOperatorId', 'Operator device ID')}</MobileSetting>
          <MobileSetting title="This device’s ID" desc="Share with the developer for testing."><button className="ios-button" disabled={!myEid} onClick={() => { if (myEid) void navigator.clipboard.writeText(myEid).then(() => toast('info', 'Device ID copied')).catch(e => toast('error', String(e))) }}>Copy ID</button></MobileSetting>
        </>}
      </MobileSection>
      <p className="mobile-inset ios-footnote mobile-footer">DropBeam · Direct, end-to-end encrypted transfers</p>
    </div>
  }

  const restartButton = (
    <button className="btn btn-secondary btn-sm" onClick={() => api.restartApp().catch(() => {})}>
      Restart
    </button>
  )
  const relayChanged = settings.customRelay.trim() !== (bootRelay ?? '').trim()
  const verboseChanged = bootVerbose !== null && settings.verboseLogging !== bootVerbose
  const diagUrlInvalid = settings.diagnosticsUrl !== '' && !settings.diagnosticsUrl.startsWith('https://')

  // The self-test answers "ok · node ab12…" when the engine is up — say that in words.
  const testOk = testResult?.startsWith('ok') ?? false
  const connectionSub = testing ? (
    'Checking…'
  ) : testResult ? (
    testOk ? (
      <span className="set-status"><Dot tone="ok" /> Ready for direct transfers</span>
    ) : (
      <span className="set-status set-status-error">{testResult}</span>
    )
  ) : (
    'Transfers go straight to the other device, end-to-end encrypted.'
  )

  const general = (
    <>
      <SectionHeader>Profile</SectionHeader>
      <div className="group">
        <Row title="Name" sub="What friends see, on all your devices.">
          <DisplayNameInput value={settings.displayName} onSave={(displayName) => void save({ displayName })} />
        </Row>
      </div>

      <SectionHeader>Appearance</SectionHeader>
      <div className="group">
        <Row title="Theme">
          <Segmented
            label="Theme"
            value={settings.theme}
            onChange={(theme) => save({ theme })}
            options={[
              { value: 'system', label: 'System' },
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
          />
        </Row>
      </div>

      <SectionHeader>App</SectionHeader>
      <div className="group">
        <ToggleRow
          title="Open at login"
          sub={`Waits in the ${TRAY_NAME} so files can arrive anytime.`}
          on={settings.launchAtLogin}
          onChange={(v) => save({ launchAtLogin: v })}
        />
        <ToggleRow
          title="Keep running when the window is closed"
          sub={`DropBeam stays in the ${TRAY_NAME} and keeps receiving.`}
          on={settings.minimizeToTray}
          onChange={(v) => save({ minimizeToTray: v })}
        />
        <ToggleRow
          title="Show folder sync progress"
          sub={`A small panel near the ${TRAY_NAME} while a shared folder syncs.`}
          on={settings.showSyncPopup}
          onChange={(v) => save({ showSyncPopup: v })}
        />
        <ToggleRow title="Play sounds" on={settings.playSounds} onChange={(v) => save({ playSounds: v })} />
      </div>

      <SectionHeader>Notifications</SectionHeader>
      <div className="group">
        <ToggleRow
          title="When a file arrives"
          on={settings.notifyOnComplete}
          onChange={(v) => save({ notifyOnComplete: v })}
        />
        <ToggleRow
          title="When a message arrives"
          sub="Only while you’re not looking at DropBeam."
          on={settings.notifyOnMessage}
          onChange={(v) => save({ notifyOnMessage: v })}
        />
      </div>

      <SectionHeader>Chat</SectionHeader>
      <div className="group">
        <ToggleRow
          title="Send read receipts"
          sub="Friends see when you’ve read their messages."
          on={settings.sendReadReceipts}
          onChange={(v) => save({ sendReadReceipts: v })}
        />
        <Row title="GIFs" sub="Add a free key from developers.giphy.com to turn on GIFs.">
          <input
            className="input set-field"
            type="text"
            aria-label="Giphy API key"
            autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off"
            placeholder="Giphy API key"
            defaultValue={settings.giphyApiKey}
            onBlur={(e) => {
              const v = e.target.value.trim()
              if (v !== settings.giphyApiKey) save({ giphyApiKey: v })
            }}
            onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur() }}
          />
        </Row>
      </div>

      <SectionHeader>Updates</SectionHeader>
      <div className="group">
        <UpdateRow />
      </div>
    </>
  )

  const transfers = (
    <>
      <SectionHeader>Downloads</SectionHeader>
      <div className="group">
        <Row title="Save files to">
          <button className="btn btn-secondary set-dir" onClick={changeDir} title={settings.downloadDir}>
            <FolderOpen />
            <span className="truncate-1">{folderLabel(settings.downloadDir)}</span>
          </button>
        </Row>
        <Row title="Transfer leftovers" sub="Lets interrupted transfers resume. Cleared after a week.">
          <button className="btn btn-secondary" onClick={clearCache} disabled={clearing}>
            {clearing && <Spinner size={12} />}
            Clear Now
          </button>
        </Row>
      </div>

      <SectionHeader>
        Connection
          <InfoTip label="How transfers connect">
            <p><b>Local network.</b> Same Wi‑Fi or network — the fastest path, and it never leaves your network.</p>
            <p><b>Direct.</b> Peer to peer across the internet, end-to-end encrypted.</p>
            <p><b>Relayed.</b> When a direct path isn’t possible, an encrypted relay carries the data. It can’t read your files, but it’s slower.</p>
          </InfoTip>
      </SectionHeader>
      <div className="group">
        <Row title="Test connection" sub={connectionSub}>
          <button className="btn btn-secondary" onClick={runDirectTest} disabled={testing}>
            {testing && <Spinner size={12} />}
            Test
          </button>
        </Row>
        {IS_MAC ? (
          <Row title="Local network access" sub="Needed for fast transfers on the same Wi‑Fi. Check both devices.">
            <button className="btn btn-secondary" onClick={() => api.openLocalNetworkSettings().catch(() => {})}>
              Open Settings…
            </button>
          </Row>
        ) : (
          <Row title="Local network access" sub="Needed for fast transfers on the same Wi‑Fi. Check both devices.">
            <InfoTip label="How to allow DropBeam on your local network">
              <p>
                {IS_WINDOWS
                  ? 'Allow DropBeam on Private networks when Windows Firewall asks, or in Windows Security → Firewall & network protection → Allow an app.'
                  : 'Make sure a firewall (ufw, firewalld…) isn’t blocking DropBeam on your local network.'}
              </p>
            </InfoTip>
          </Row>
        )}
        <ToggleRow
          title="Only send over direct connections"
          sub="If there’s no direct path, the send stops instead of using a relay."
          on={settings.requireDirect}
          onChange={(v) => save({ requireDirect: v })}
        />
        <ToggleRow
          title="Wait for a direct connection"
          sub={
            settings.requireDirect
              ? 'Not needed while only direct connections are allowed.'
              : 'Hold sends until a direct path forms, instead of using a relay.'
          }
          on={settings.requireDirect ? false : settings.waitForDirect}
          disabled={settings.requireDirect}
          onChange={(v) => save({ waitForDirect: v })}
        />
        <ToggleRow
          title="Parallel streams"
          sub="Faster sends for files over 16 MB. Turn off if transfers stall."
          on={settings.parallelStreams}
          onChange={(v) => save({ parallelStreams: v })}
        />
      </div>

      <SectionHeader>Speed</SectionHeader>
      <div className="group">
        <Row title="Upload limit" sub="In Mbps. Local network transfers always run at full speed.">
          <UploadLimit value={settings.uploadLimitMbps || 0} onChange={(uploadLimitMbps) => save({ uploadLimitMbps })} />
        </Row>
        <ToggleRow
          title="Show speeds in megabits"
          sub="Mbps instead of MB/s, on this device."
          on={settings.showMegabits}
          onChange={(v) => save({ showMegabits: v })}
        />
      </div>

      <SectionHeader>
        Recoverable files
          <InfoTip label="About recoverable files">
            <p>When a file in a shared folder is deleted or replaced, DropBeam keeps a copy you can restore from History. Old copies are removed automatically.</p>
          </InfoTip>
      </SectionHeader>
      <div className="group">
        <Row title="Keep copies for">
          <Segmented
            label="Keep copies for"
            value={String(settings.folderHistoryKeepDays)}
            onChange={(v) => save({ folderHistoryKeepDays: Number(v) })}
            options={[
              { value: '7', label: '7 days' },
              { value: '30', label: '30 days' },
              { value: '90', label: '90 days' },
              { value: '0', label: 'Forever' },
            ]}
          />
        </Row>
        <Row title="Limit per folder">
          <Segmented
            label="Storage limit per folder"
            value={String(settings.folderHistoryBudgetBytes)}
            onChange={(v) => save({ folderHistoryBudgetBytes: Number(v) })}
            options={[
              { value: String(500 * 1024 * 1024), label: '500 MB' },
              { value: String(2 * 1024 * 1024 * 1024), label: '2 GB' },
              { value: String(5 * 1024 * 1024 * 1024), label: '5 GB' },
              { value: '0', label: 'No limit' },
            ]}
          />
        </Row>
        {confirmFree ? (
          <Row title="Remove all recoverable copies?" sub="Your live files aren’t touched.">
            <button className="btn btn-secondary" onClick={() => setConfirmFree(false)} disabled={freeingHistory}>
              Cancel
            </button>
            <button className="btn btn-destructive" onClick={freeHistory} disabled={freeingHistory}>
              {freeingHistory && <Spinner size={12} />}
              Remove
            </button>
          </Row>
        ) : (
          <Row
            title="Space used"
            sub={historyUsage === null ? '…' : historyUsage > 0 ? formatBytes(historyUsage) : 'None'}
          >
            <button className="btn btn-secondary" onClick={() => setConfirmFree(true)} disabled={historyUsage === 0}>
              Free Up Space…
            </button>
          </Row>
        )}
      </div>
    </>
  )

  const privacy = (
    <>
      <SafetySection />

      <SectionHeader>Diagnostics</SectionHeader>
      <div className="group">
        <ToggleRow
          title="Share diagnostics"
          sub="A redacted daily summary of errors. Never file names or contents."
          on={settings.shareDiagnostics}
          onChange={(v) => save({ shareDiagnostics: v })}
        />
        {settings.shareDiagnostics && (
          <Row
            title="Custom collector"
            sub={diagUrlInvalid ? <span className="set-status-error">Use an https:// address.</span> : 'Leave empty to use the built-in one.'}
          >
            <input
              className="input set-field"
              aria-label="Diagnostics endpoint"
              autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off"
              placeholder="https://"
              value={settings.diagnosticsUrl}
              onChange={(e) => save({ diagnosticsUrl: e.target.value })}
            />
            <button
              className="btn btn-secondary"
              disabled={testingDiag || diagUrlInvalid}
              onClick={async () => {
                setTestingDiag(true)
                try {
                  toast('info', await api.diagnosticsTest())
                } catch (e) {
                  toast('error', String(e))
                } finally {
                  setTestingDiag(false)
                }
              }}
            >
              {testingDiag && <Spinner size={12} />}
              Send Test
            </button>
          </Row>
        )}
      </div>

      <SectionHeader>
        Support
          <InfoTip label="Reporting a person">
            <p>To report a person or a message, use Report… on their friend card, in their chat, or on the message. We reply within 24 hours.</p>
          </InfoTip>
      </SectionHeader>
      <div className="group">
        <Row title="Report a problem" sub="Opens an email to the DropBeam team.">
          <button
            className="btn btn-secondary"
            onClick={() =>
              api
                .openMailto(contactMailto(appVer || null, platformLabel(navigator.userAgent)))
                .catch((e) => toast('error', `Couldn’t open your mail app: ${String(e)}`))
            }
          >
            Email Us…
          </button>
        </Row>
      </div>
    </>
  )

  const advanced = (
    <>
      <SectionHeader>
        Relay
          <InfoTip label="About relays">
            <p>When two devices can’t connect directly, data goes through a relay. Point both devices at your own relay for a faster, steadier fallback.</p>
            <button className="btn btn-plain btn-sm set-info-link" onClick={() => api.openUrl(RELAY_GUIDE).catch(() => {})}>
              Relay setup guide
            </button>
          </InfoTip>
      </SectionHeader>
      <div className="group">
        <Row
          title="Custom relay"
          sub={relayChanged ? 'Restart DropBeam to use this relay.' : 'Use the same relay on both devices. Leave empty for public relays.'}
        >
          {relayChanged && restartButton}
          <input
            className="input set-field set-field-wide"
            aria-label="Relay server URL"
            autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off"
            placeholder="https://relay.example.com"
            value={settings.customRelay}
            onChange={(e) => save({ customRelay: e.target.value })}
          />
        </Row>
      </div>

      <SectionHeader>Logs</SectionHeader>
      <div className="group">
        <ToggleRow
          title="Detailed logging"
          sub={verboseChanged ? 'Restart DropBeam to apply.' : 'Adds network internals while you reproduce a connection problem.'}
          on={settings.verboseLogging}
          onChange={(v) => save({ verboseLogging: v })}
          extra={verboseChanged ? restartButton : undefined}
        />
        <Row title="Export logs" sub="Saves a log bundle to Downloads. No passwords or file contents.">
          <button className="btn btn-secondary" onClick={exportLogs} disabled={exporting}>
            {exporting && <Spinner size={12} />}
            Export…
          </button>
        </Row>
      </div>

      <SectionHeader>Lab Mode</SectionHeader>
      <div className="group">
        <ToggleRow
          title="Allow Lab Mode"
          sub="Lets one trusted developer device run tests on this app. Turn on only if asked."
          on={settings.labModeEnabled}
          onChange={(v) => save({ labModeEnabled: v })}
        />
        {settings.labModeEnabled && (
          <>
            <Row title="Operator ID" sub={settings.labOperatorId ? 'Only this device can connect.' : 'No device can connect until you add one.'}>
              <input
                className="input set-field set-field-wide set-mono"
                aria-label="Operator device ID"
                autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off"
                placeholder="Paste operator ID"
                value={settings.labOperatorId}
                onChange={(e) => save({ labOperatorId: e.target.value.trim() })}
              />
              <IconButton label="Scan operator ID QR code" tooltip="Scan QR code" onClick={() => setScanOperator(true)}>
                <ScanLine />
              </IconButton>
            </Row>
            <Row title="This device’s ID" sub="Share it with the developer for testing.">
              <button
                className="btn btn-secondary"
                disabled={!myEid}
                onClick={async () => {
                  if (!myEid) return
                  try {
                    await navigator.clipboard.writeText(myEid)
                    toast('info', 'Device ID copied')
                  } catch (e) {
                    toast('error', String(e))
                  }
                }}
              >
                {myEid ? 'Copy ID' : 'Starting…'}
              </button>
              <IconButton
                label={showEidQr ? 'Hide device ID QR code' : 'Show device ID QR code'}
                tooltip={showEidQr ? 'Hide QR code' : 'Show QR code'}
                disabled={!myEid}
                active={showEidQr}
                aria-pressed={showEidQr}
                onClick={() => setShowEidQr((v) => !v)}
              >
                <QrCode />
              </IconButton>
            </Row>
            {showEidQr && myEid && (
              <div className="set-qr">
                <QrCodeView value={myEid} size={148} hint="Scan to read this device ID" />
              </div>
            )}
          </>
        )}
      </div>
      {scanOperator && (
        <QrScanner
          title="Scan operator ID"
          hint="Scan the operator device ID QR code."
          validate={(t) => (/^[0-9a-f]{64}$/i.test(t.trim()) ? null : 'That QR code isn’t a device ID.')}
          onClose={() => setScanOperator(false)}
          onResult={(t) => { setScanOperator(false); void save({ labOperatorId: t.trim().toLowerCase() }) }}
        />
      )}
    </>
  )

  const visibleTabs = TABS.filter((t) => t.value !== 'locations' || !MOBILE_UI)

  return (
    <div className="page settings-page" ref={rootRef}>
      <div className="page-header titlebar-drag">
        <h1 className="page-title">Settings</h1>
        <div className="page-actions">
          <Segmented role="tablist" label="Settings sections" value={tab} onChange={setTab} options={visibleTabs} className="settings-tabs" />
        </div>
      </div>
      {deviceModals}
      <div role="tabpanel" aria-label={TABS.find((t) => t.value === tab)?.label} className={`settings-pane settings-pane-${tab}`}>
        {tab === 'general' && general}
        {tab === 'devices' && <DevicesPanel />}
        {tab === 'locations' && <LocationSettings />}
        {tab === 'transfers' && transfers}
        {tab === 'privacy' && privacy}
        {tab === 'advanced' && advanced}
      </div>
    </div>
  )
}

/** Version + update check + install progress, as one row. */
function UpdateRow() {
  const appVer = useStore((s) => s.appVer)
  const update = useStore((s) => s.update)
  const checkingUpdate = useStore((s) => s.checkingUpdate)
  const updateError = useStore((s) => s.updateError)
  const checkForUpdates = useStore((s) => s.checkForUpdates)
  const installUpdate = useStore((s) => s.installUpdate)

  let sub: ReactNode
  let control: ReactNode
  if (update?.installing) {
    sub = update.progress < 100 ? `Downloading version ${update.version}… ${update.progress}%` : 'Installing — DropBeam will restart…'
    control = <div className="set-update-bar"><ProgressBar percent={update.progress} label="Update download" /></div>
  } else if (update) {
    sub = `Version ${update.version} is available.`
    control = (
      <button className="btn btn-primary" onClick={() => installUpdate()}>
        Install and Restart
      </button>
    )
  } else if (updateError) {
    sub = 'Couldn’t reach the update server.'
    control = (
      <>
        <button className="btn btn-secondary" onClick={() => api.openUrl('https://github.com/lman80/dropbeam/releases/latest').catch(() => {})}>
          Download…
        </button>
        <button className="btn btn-secondary" onClick={() => checkForUpdates(true)} disabled={checkingUpdate}>
          {checkingUpdate && <Spinner size={12} />}
          Try Again
        </button>
      </>
    )
  } else {
    sub = checkingUpdate ? 'Checking for updates…' : null
    control = (
      <button className="btn btn-secondary" onClick={() => checkForUpdates(true)} disabled={checkingUpdate}>
        {checkingUpdate && <Spinner size={12} />}
        Check for Updates
      </button>
    )
  }
  return <Row title={<span className="tnum">DropBeam {appVer || '…'}</span>} sub={sub}>{control}</Row>
}

/** Upload cap: presets as a segmented control, plus a custom value. 0 = off. */
const UPLOAD_PRESETS = [0, 50, 100, 150, 300]
function UploadLimit({ value, onChange }: { value: number; onChange: (mbps: number) => void }) {
  const [custom, setCustom] = useState(!UPLOAD_PRESETS.includes(value))
  const seg = custom ? 'custom' : String(value)
  return (
    <div className="set-upload">
      {custom && (
        <label className="set-upload-custom">
          <input
            className="input tnum"
            type="number"
            min={0}
            max={100000}
            aria-label="Upload limit in Mbps"
            autoFocus
            value={value || ''}
            placeholder="0"
            onChange={(e) => onChange(Math.max(0, Math.floor(Number(e.target.value) || 0)))}
          />
          <span className="muted">Mbps</span>
        </label>
      )}
      <Segmented
        label="Upload limit"
        value={seg}
        onChange={(v) => {
          if (v === 'custom') { setCustom(true); return }
          setCustom(false)
          onChange(Number(v))
        }}
        options={[
          { value: '0', label: 'Off' },
          { value: '50', label: '50' },
          { value: '100', label: '100', title: 'A good starting point for most home Wi‑Fi' },
          { value: '150', label: '150' },
          { value: '300', label: '300' },
          { value: 'custom', label: 'Custom' },
        ]}
      />
    </div>
  )
}

function MobileSection({ title, children }: { title: string; children: ReactNode }) {
  return <section><h2 className="ios-section-title">{title}</h2><div className="ios-list">{children}</div></section>
}

function MobileSetting({ title, desc, children, destructive = false }: { title: string; desc?: string; children?: ReactNode; destructive?: boolean }) {
  return <div className={`ios-row mobile-setting${destructive ? ' mobile-setting-destructive' : ''}`}>
    <div className="mobile-grow"><div>{title}</div>{desc && <p className="ios-footnote">{desc}</p>}</div>
    {children && <div className="mobile-setting-control">{children}</div>}
  </div>
}

/** Edit a local draft and save on blur/Enter. Saving per keystroke round-tripped
 * through the engine, which trims the name — so a typed space vanished at once
 * ("John Smith" became "JohnSmith") and every keystroke re-broadcast the profile. */
function DisplayNameInput({ value, onSave }: { value: string; onSave: (name: string) => void }) {
  // null = not editing: show the saved value (so outside changes still appear).
  const [draft, setDraft] = useState<string | null>(null)
  const commit = () => {
    const name = (draft ?? '').trim()
    if (name && name !== value) onSave(name)
    setDraft(null)
  }
  return (
    <input
      className="input set-field"
      aria-label="Display name"
      value={draft ?? value}
      maxLength={64}
      onFocus={() => setDraft(value)}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur() }}
    />
  )
}

/** Settings → Privacy: the Blocked list (unblock). */
function SafetySection() {
  const blocked = useStore((s) => s.blocked)
  const unblock = useStore((s) => s.unblockPerson)
  const toast = useStore((s) => s.toast)
  const [busy, setBusy] = useState<string | null>(null)
  return (
    <>
      <SectionHeader>
        Blocked
          <InfoTip label="About blocking">
            <p>Blocked people can’t message you, send you files, invite you to folders or browse your Locations, and they aren’t told. Blocks apply on all your devices.</p>
            <p>To block someone, use … → Block on their friend card or in their chat.</p>
          </InfoTip>
      </SectionHeader>
      <div className="group">
        {blocked.length === 0 ? (
          <div className="row set-row"><div className="row-main row-sub set-empty">No one is blocked.</div></div>
        ) : (
          blocked.map((p) => (
            <Row
              key={p.id}
              title={<span className="truncate-1 set-block-name" title={p.name}>{p.name}</span>}
              sub={`${p.endpointIds.length > 1 ? `${p.endpointIds.length} devices · ` : ''}Blocked ${new Date(p.at).toLocaleDateString()}`}
            >
              <button
                className="btn btn-secondary"
                disabled={busy === p.id}
                onClick={async () => {
                  setBusy(p.id)
                  await unblock(p.id)
                  setBusy(null)
                  toast('info', `${p.name} is unblocked. Add them again with their code if you want to.`)
                }}
              >
                {busy === p.id && <Spinner size={12} />}
                Unblock
              </button>
            </Row>
          ))
        )}
      </div>
    </>
  )
}
