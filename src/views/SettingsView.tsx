import { useEffect, useState, type ReactNode } from 'react'
import { CheckCircle2, Download, FolderOpen, HardDrive, RefreshCw, Trash2 } from 'lucide-react'
import { api, type Settings } from '../lib/api'
import { formatBytes } from '../lib/format'
import { useStore } from '../store'
import { ChannelBadge, ProgressBar, SectionTitle, Spinner } from '../components/bits'

function Toggle({
  on,
  onChange,
  disabled,
}: {
  on: boolean
  onChange: (v: boolean) => void
  disabled?: boolean
}) {
  return (
    <button
      className={`toggle${on ? ' on' : ''}`}
      onClick={() => !disabled && onChange(!on)}
      aria-pressed={on}
      aria-disabled={disabled}
      disabled={disabled}
      style={disabled ? { opacity: 0.4, cursor: 'not-allowed' } : undefined}
    />
  )
}

function Row({
  title,
  desc,
  children,
}: {
  title: string
  desc?: string
  children: ReactNode
}) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 16,
        padding: '13px 4px',
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 14, fontWeight: 600 }}>{title}</div>
        {desc && (
          <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginTop: 2, lineHeight: 1.45 }}>
            {desc}
          </div>
        )}
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  )
}

function Card({ children }: { children: ReactNode }) {
  return (
    <div className="card" style={{ padding: '6px 16px', marginBottom: 16 }}>
      {children}
    </div>
  )
}

const SEP = (
  <div style={{ height: 1, background: 'var(--border)', margin: '0 -16px' }} />
)

export function SettingsView() {
  const settings = useStore((s) => s.settings) as Settings
  const save = useStore((s) => s.saveSettings)
  const appVer = useStore((s) => s.appVer)
  const update = useStore((s) => s.update)
  const checkingUpdate = useStore((s) => s.checkingUpdate)
  const updateError = useStore((s) => s.updateError)
  const checkForUpdates = useStore((s) => s.checkForUpdates)
  const installUpdate = useStore((s) => s.installUpdate)
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<string | null>(null)
  const [historyUsage, setHistoryUsage] = useState<number | null>(null)
  const [freeingHistory, setFreeingHistory] = useState(false)

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
    }
  }

  const [exporting, setExporting] = useState(false)
  const [testingDiag, setTestingDiag] = useState(false)
  const exportLogs = async () => {
    setExporting(true)
    try {
      const path = await api.exportDiagnostics()
      await api.revealPath(path).catch(() => {})
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

  return (
    <div style={{ maxWidth: 620, margin: '0 auto', padding: '8px 28px 40px' }}>
      <h1 className="titlebar-drag" style={{ fontSize: 20, fontWeight: 750, margin: '0 0 16px' }}>
        Settings
      </h1>

      <SectionTitle>Profile</SectionTitle>
      <Card>
        <Row title="Display name" desc="What paired devices see you as.">
          <input
            className="input"
            style={{ width: 200 }}
            value={settings.displayName}
            onChange={(e) => save({ displayName: e.target.value })}
          />
        </Row>
      </Card>

      <SectionTitle>Downloads</SectionTitle>
      <Card>
        <Row title="Save received files to">
          <button className="btn btn-ghost" onClick={changeDir} title={settings.downloadDir}>
            <FolderOpen size={15} />
            <span
              style={{
                maxWidth: 200,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              {settings.downloadDir.split('/').pop() || settings.downloadDir}
            </span>
          </button>
        </Row>
        {SEP}
        <Row
          title="Clear transfer cache"
          desc="Interrupted transfers keep their progress on disk so they can resume. Old leftovers are cleaned automatically after a week; this removes them now."
        >
          <button className="btn btn-ghost" onClick={clearCache} disabled={clearing}>
            {clearing ? <Spinner size={14} /> : <Trash2 size={15} />}
            <span>Clear now</span>
          </button>
        </Row>
      </Card>

      <SectionTitle>Appearance</SectionTitle>
      <Card>
        <Row title="Theme">
          <div className="seg">
            {(['system', 'light', 'dark'] as const).map((t) => (
              <button
                key={t}
                className={settings.theme === t ? 'active' : ''}
                onClick={() => save({ theme: t })}
                style={{ textTransform: 'capitalize', padding: '6px 12px' }}
              >
                {t}
              </button>
            ))}
          </div>
        </Row>
      </Card>

      <SectionTitle>Behavior</SectionTitle>
      <Card>
        <Row
          title="Stay ready in the background"
          desc="Start DropBeam automatically at login and keep it quietly in the menu bar, so files can arrive even when you haven’t opened it. Turn off and DropBeam only receives while it’s open."
        >
          <Toggle on={settings.launchAtLogin} onChange={(v) => save({ launchAtLogin: v })} />
        </Row>
        {SEP}
        <Row
          title="Keep running when you close the window"
          desc="Closing the window tucks DropBeam into the menu bar instead of quitting, so it keeps receiving."
        >
          <Toggle on={settings.minimizeToTray} onChange={(v) => save({ minimizeToTray: v })} />
        </Row>
        {SEP}
        <Row
          title="Notify when a file arrives"
          desc="Pop a notification when someone sends you a file, even if DropBeam is in the background."
        >
          <Toggle on={settings.notifyOnComplete} onChange={(v) => save({ notifyOnComplete: v })} />
        </Row>
        {SEP}
        <Row
          title="Chat message notifications"
          desc="Pop a notification when a friend messages you and the app isn’t focused."
        >
          <Toggle on={settings.notifyOnMessage} onChange={(v) => save({ notifyOnMessage: v })} />
        </Row>
        {SEP}
        <Row
          title="Send read receipts"
          desc="Let friends see when you’ve read their message. Turning this off stops you sending them."
        >
          <Toggle on={settings.sendReadReceipts} onChange={(v) => save({ sendReadReceipts: v })} />
        </Row>
        {SEP}
        <Row
          title="GIFs (Giphy key)"
          desc="Paste a free key from developers.giphy.com to enable the GIF picker in chat. Leave blank to hide it."
        >
          <input
            className="input"
            style={{ width: 220 }}
            type="text"
            placeholder="Giphy API key"
            defaultValue={settings.giphyApiKey}
            onBlur={(e) => {
              const v = e.target.value.trim()
              if (v !== settings.giphyApiKey) save({ giphyApiKey: v })
            }}
          />
        </Row>
        {SEP}
        <Row title="Play sounds" desc="Soft cues when you send, receive, or get a file offer.">
          <Toggle on={settings.playSounds} onChange={(v) => save({ playSounds: v })} />
        </Row>
        {SEP}
        <Row
          title="Show the folder-sync popup"
          desc="The little floating “syncing folder…” card that appears during a shared-folder transfer. Turn it off if it’s distracting."
        >
          <Toggle on={settings.showSyncPopup} onChange={(v) => save({ showSyncPopup: v })} />
        </Row>
      </Card>

      <SectionTitle>Connection</SectionTitle>
      <Card>
        <Row
          title="Direct peer-to-peer"
          desc="Every transfer — Quick Send, friends, and shared folders — goes straight to the other computer, end-to-end encrypted, as fast as your network allows. Your firewall may ask once to allow DropBeam."
        >
          <span style={{ fontSize: 13, fontWeight: 650, color: 'var(--accent)' }}>On</span>
        </Row>
        {SEP}
        <Row
          title="Test direct connection"
          desc={testResult || 'Confirm the direct engine is running on this computer.'}
        >
          <button className="btn btn-ghost" onClick={runDirectTest} disabled={testing}>
            {testing ? <Spinner size={14} /> : <RefreshCw size={15} />} Test
          </button>
        </Row>
        {SEP}
        <Row
          title="Local network access"
          desc="macOS must allow DropBeam on your Local Network for fast same-Wi-Fi transfers. If transfers to a nearby device keep using the slow relay, enable DropBeam here — and check it on the other device too."
        >
          <button
            className="btn btn-ghost"
            onClick={() => api.openLocalNetworkSettings().catch(() => {})}
          >
            Open Settings
          </button>
        </Row>
        {SEP}
        <Row
          title="Only send over direct connections"
          desc="Refuse the slow relay: if a direct path (local network or peer-to-peer) can't be made, the send fails instead of crawling through the relay. Applies to Quick Send + friend sends; shared folders always use the best available path."
        >
          <Toggle on={settings.requireDirect} onChange={(v) => save({ requireDirect: v })} />
        </Row>
        {SEP}
        <Row
          title="Wait for a direct connection"
          desc={
            settings.requireDirect
              ? 'Off because “Only send over direct connections” above already refuses the relay outright — there’s nothing to wait for.'
              : 'When a send can only reach the relay, DropBeam holds off and keeps trying for a fast direct path instead of crawling through the relay. Your files stay put until a direct link forms — and each transfer card shows a “Send over relay anyway” button if you’d rather not wait.'
          }
        >
          <Toggle
            on={settings.requireDirect ? false : settings.waitForDirect}
            disabled={settings.requireDirect}
            onChange={(v) => save({ waitForDirect: v })}
          />
        </Row>
        {SEP}
        <Row
          title="Use parallel streams for big files"
          desc="Send one large file (16 MB or more) over several connections at once for higher speed — the file is reassembled exactly. On by default. Turn off only if a transfer keeps stalling on a particular network; it'll then send the steady, single-stream way."
        >
          <Toggle on={settings.parallelStreams} onChange={(v) => save({ parallelStreams: v })} />
        </Row>
        {SEP}
        <Row
          title="Limit internet upload speed"
          desc="Cap how much of your upload a transfer uses, so video calls, streaming, and browsing stay smooth — and so a big transfer doesn't overwhelm an older Wi-Fi router. 0 = unlimited. Local-network transfers always run full speed. There's no safe way to auto-test a router's breaking point, so start at the recommended 100 Mbps and raise it until your Wi-Fi stutters, then back off a little."
        >
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 8 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
              <input
                type="number"
                min={0}
                max={100000}
                value={settings.uploadLimitMbps || 0}
                onChange={(e) =>
                  save({ uploadLimitMbps: Math.max(0, Math.floor(Number(e.target.value) || 0)) })
                }
                style={{
                  width: 72,
                  fontSize: 13,
                  fontWeight: 600,
                  padding: '6px 8px',
                  borderRadius: 8,
                  border: '1px solid var(--border)',
                  background: 'var(--bg-elev)',
                  color: 'var(--text)',
                  textAlign: 'right',
                }}
              />
              <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>Mbps</span>
            </div>
            <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
              {[
                { label: '50', v: 50 },
                { label: '100', v: 100, rec: true },
                { label: '150', v: 150 },
                { label: '300', v: 300 },
                { label: 'Unlimited', v: 0 },
              ].map((p) => {
                const on = (settings.uploadLimitMbps || 0) === p.v
                return (
                  <button
                    key={p.v}
                    onClick={() => save({ uploadLimitMbps: p.v })}
                    title={p.rec ? 'Recommended starting point for most home routers' : undefined}
                    style={{
                      fontSize: 12,
                      fontWeight: 600,
                      padding: '4px 9px',
                      borderRadius: 999,
                      cursor: 'pointer',
                      border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                      background: on ? 'var(--accent-soft)' : 'var(--surface-2)',
                      color: on ? 'var(--accent)' : 'var(--text-muted)',
                    }}
                  >
                    {p.label}
                    {p.rec ? ' ★' : ''}
                  </button>
                )
              })}
            </div>
          </div>
        </Row>
        {SEP}
        <Row
          title="Show speeds in megabits (Mbps)"
          desc="Off shows megabytes per second (MB/s), what most file tools use. On shows megabits per second (Mbps), like internet plans."
        >
          <Toggle on={settings.showMegabits} onChange={(v) => save({ showMegabits: v })} />
        </Row>
      </Card>

      <SectionTitle>How transfers connect</SectionTitle>
      <Card>
        <div style={{ padding: '4px 2px', display: 'flex', flexDirection: 'column', gap: 14 }}>
          {[
            {
              loc: 'local' as const,
              text: "You and the other device are on the same Wi-Fi / network. Files go straight across your local network — the fastest option, and they never touch the internet.",
            },
            {
              loc: 'direct' as const,
              text: 'A direct peer-to-peer link across the internet (DropBeam "hole-punches" through both routers). Files go straight between the two computers, end-to-end encrypted, no middleman. Fast and private.',
            },
            {
              loc: 'internet' as const,
              text: "When a direct link can't be made (a strict or locked-down network), files hop through an encrypted relay server. Still private — the relay can't read them — but much slower, since everything routes through a shared middle server.",
            },
            {
              loc: 'unknown' as const,
              text: "Still working out the best route to the other device — usually a second or two while it tries for a direct path before settling.",
            },
          ].map((c) => (
            <div key={c.loc} style={{ display: 'flex', gap: 11, alignItems: 'flex-start' }}>
              <div style={{ flexShrink: 0, marginTop: 1 }}>
                <ChannelBadge locality={c.loc} showConnecting />
              </div>
              <span style={{ fontSize: 12.5, color: 'var(--text-muted)', lineHeight: 1.5 }}>
                {c.text}
              </span>
            </div>
          ))}
          <span style={{ fontSize: 11.5, color: 'var(--text-faint)', lineHeight: 1.5 }}>
            The badge on each transfer shows which one it's using. Want to avoid the slow relay
            entirely? Turn on "Only send over direct connections" above.
          </span>
        </div>
      </Card>

      <SectionTitle>Recoverable files</SectionTitle>
      <Card>
        <div style={{ padding: '12px 2px 4px' }}>
          <div style={{ fontSize: 14, fontWeight: 600 }}>Deleted &amp; replaced files in shared folders</div>
          <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginTop: 2, lineHeight: 1.45 }}>
            When something is deleted or overwritten in a shared folder, DropBeam keeps a copy so you can
            get it back. Old copies are cleaned up automatically so they never pile up.{' '}
            {historyUsage !== null && (
              <b style={{ color: 'var(--text)' }}>Currently using {formatBytes(historyUsage)}.</b>
            )}
          </div>
        </div>

        <div style={{ padding: '10px 2px' }}>
          <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>Keep copies for</div>
          <div className="seg" style={{ display: 'flex', width: '100%' }}>
            {(
              [
                { label: '7 days', v: 7 },
                { label: '30 days', v: 30 },
                { label: '90 days', v: 90 },
                { label: 'Forever', v: 0 },
              ] as const
            ).map((o) => (
              <button
                key={o.v}
                className={settings.folderHistoryKeepDays === o.v ? 'active' : ''}
                style={{ flex: 1, justifyContent: 'center' }}
                onClick={() => save({ folderHistoryKeepDays: o.v })}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>

        <div style={{ padding: '10px 2px 14px' }}>
          <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>Storage limit per folder</div>
          <div className="seg" style={{ display: 'flex', width: '100%' }}>
            {(
              [
                { label: '500 MB', v: 500 * 1024 * 1024 },
                { label: '2 GB', v: 2 * 1024 * 1024 * 1024 },
                { label: '5 GB', v: 5 * 1024 * 1024 * 1024 },
                { label: 'No limit', v: 0 },
              ] as const
            ).map((o) => (
              <button
                key={o.v}
                className={settings.folderHistoryBudgetBytes === o.v ? 'active' : ''}
                style={{ flex: 1, justifyContent: 'center' }}
                onClick={() => save({ folderHistoryBudgetBytes: o.v })}
              >
                {o.label}
              </button>
            ))}
          </div>
        </div>

        {SEP}
        <Row
          title="Free up space now"
          desc="Remove every saved copy across all your shared folders. Your live files aren’t touched."
        >
          <button className="btn btn-ghost" onClick={freeHistory} disabled={freeingHistory}>
            {freeingHistory ? <Spinner size={14} /> : <HardDrive size={15} />} Free up
          </button>
        </Row>
      </Card>

      <SectionTitle>Custom relay (advanced)</SectionTitle>
      <Card>
        <Row
          title="Relay server URL"
          desc="When two devices can't connect directly, files fall back to a relay. By default that's iroh's shared public relays — fine, but sometimes slow or flaky for far-apart devices. Point BOTH devices at your own free relay for a fast, reliable fallback. Leave blank to use the public relays. Applied on restart. See RELAY-SETUP.md for the free 10-minute setup."
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <button
              className="btn btn-ghost"
              onClick={() => api.restartApp().catch(() => {})}
              title="Restart DropBeam so the relay change takes effect"
            >
              <RefreshCw size={14} /> Restart
            </button>
            <input
              className="input"
              style={{ width: 230 }}
              placeholder="https://relay.example.com"
              value={settings.customRelay}
              onChange={(e) => save({ customRelay: e.target.value })}
            />
          </div>
        </Row>
      </Card>

      <SectionTitle>Updates</SectionTitle>
      <Card>
        <Row title="Version" desc={`DropBeam ${appVer || '…'}`}>
          <button
            className="btn btn-ghost"
            onClick={() => checkForUpdates(true)}
            disabled={checkingUpdate}
          >
            {checkingUpdate ? <Spinner size={14} /> : <RefreshCw size={15} />} Check for updates
          </button>
        </Row>
        {update && (
          <>
            {SEP}
            <div style={{ padding: '13px 4px' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <CheckCircle2 size={17} color="var(--green)" />
                <span style={{ fontWeight: 650, fontSize: 14 }}>
                  Version {update.version} is available
                </span>
              </div>
              {update.installing ? (
                <div style={{ marginTop: 11 }}>
                  <ProgressBar percent={update.progress} />
                  <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 7 }}>
                    {update.progress < 100
                      ? `Downloading… ${update.progress}%`
                      : 'Installing — DropBeam will restart…'}
                  </div>
                </div>
              ) : (
                <button
                  className="btn btn-primary"
                  style={{ marginTop: 11 }}
                  onClick={() => installUpdate()}
                >
                  <Download size={15} /> Install &amp; restart
                </button>
              )}
            </div>
          </>
        )}
        {updateError && !update && (
          <>
            {SEP}
            <div style={{ padding: '13px 4px' }}>
              <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginBottom: 9, lineHeight: 1.5 }}>
                Couldn't reach the update server. If you're on a network that blocks
                GitHub, download the latest installer manually:
              </div>
              <button
                className="btn btn-ghost"
                onClick={() =>
                  api.openUrl('https://github.com/lman80/dropbeam/releases/latest').catch(() => {})
                }
              >
                <Download size={15} /> Get the latest from GitHub
              </button>
            </div>
          </>
        )}
      </Card>

      <SectionTitle>Diagnostics</SectionTitle>
      <Card>
        <Row
          title="Detailed logging"
          desc="Add the deepest network-internal logs (iroh connection setup, path selection, hole-punch) on top of the usual diagnostics — turn this on only while reproducing a hard-to-spot connection issue. Sharing diagnostics (below) already keeps DropBeam's own logs detailed; this adds the heavier transport internals. Takes effect after a restart."
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <button
              className="btn btn-ghost"
              onClick={() => api.restartApp().catch(() => {})}
              title="Restart DropBeam so the logging change takes effect"
            >
              <RefreshCw size={14} /> Restart
            </button>
            <Toggle on={settings.verboseLogging} onChange={(v) => save({ verboseLogging: v })} />
          </div>
        </Row>
        {SEP}
        <Row
          title="Export logs"
          desc="Bundle the logs into one file in your Downloads folder (no passwords or file contents — just diagnostics). Send it over DropBeam (drop it on a friend) or AirDrop to get the issue diagnosed."
        >
          <button className="btn btn-ghost" onClick={exportLogs} disabled={exporting}>
            {exporting ? <Spinner size={14} /> : <Download size={15} />}
            <span>Export</span>
          </button>
        </Row>
        {SEP}
        <Row
          title="Share background diagnostics"
          desc="Automatically send a small, redacted summary of errors and transfer performance (never file names or contents) so problems can be found and fixed without you reporting them. Keeps DropBeam's own logs detailed so the summary is useful. Uploads about once a day — only to the endpoint set below."
        >
          <Toggle on={settings.shareDiagnostics} onChange={(v) => save({ shareDiagnostics: v })} />
        </Row>
        {settings.shareDiagnostics && (
          <>
            {SEP}
            <Row
              title="Diagnostics endpoint"
              desc="A collector is built in, so this works out of the box. Override it here only if you run your own (advanced)."
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <input
                  className="input"
                  style={{ minWidth: 240 }}
                  placeholder="Built-in (leave blank)"
                  value={settings.diagnosticsUrl}
                  onChange={(e) => save({ diagnosticsUrl: e.target.value })}
                  spellCheck={false}
                  autoCapitalize="off"
                />
                <button
                  className="btn btn-ghost"
                  disabled={
                    testingDiag ||
                    (settings.diagnosticsUrl !== '' && !settings.diagnosticsUrl.startsWith('https://'))
                  }
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
                  {testingDiag ? <Spinner size={14} /> : 'Send test'}
                </button>
              </div>
            </Row>
          </>
        )}
      </Card>

      <div
        style={{
          textAlign: 'center',
          fontSize: 12,
          color: 'var(--text-faint)',
          marginTop: 18,
          lineHeight: 1.6,
        }}
      >
        DropBeam · Direct, end-to-end encrypted peer-to-peer transfers
        <br />
        No accounts · Your files never touch a server · Diagnostics are opt-out
      </div>
    </div>
  )
}
