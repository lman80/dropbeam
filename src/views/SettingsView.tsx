import { useState, type ReactNode } from 'react'
import { CheckCircle2, Download, FolderOpen, RefreshCw } from 'lucide-react'
import { api, type Settings } from '../lib/api'
import { useStore } from '../store'
import { ProgressBar, SectionTitle, Spinner } from '../components/bits'

function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      className={`toggle${on ? ' on' : ''}`}
      onClick={() => onChange(!on)}
      aria-pressed={on}
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

  const changeDir = async () => {
    const d = await api.pickDirectory()
    if (d) save({ downloadDir: d })
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
        <Row title="Launch at login" desc="Start DropBeam automatically so transfers can arrive.">
          <Toggle on={settings.launchAtLogin} onChange={(v) => save({ launchAtLogin: v })} />
        </Row>
        {SEP}
        <Row
          title="Keep running in the menu bar"
          desc="Closing the window hides DropBeam to the menu bar instead of quitting."
        >
          <Toggle on={settings.minimizeToTray} onChange={(v) => save({ minimizeToTray: v })} />
        </Row>
        {SEP}
        <Row title="Notify when transfers finish">
          <Toggle on={settings.notifyOnComplete} onChange={(v) => save({ notifyOnComplete: v })} />
        </Row>
        {SEP}
        <Row title="Play sounds" desc="Soft cues when you send, receive, or get a file offer.">
          <Toggle on={settings.playSounds} onChange={(v) => save({ playSounds: v })} />
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
          title="Limit internet upload speed"
          desc="Cap how much of your upload a transfer uses, so video calls, streaming, and browsing stay smooth. 0 = unlimited. Local-network transfers always run full speed."
        >
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
        </Row>
        {SEP}
        <Row
          title="Show speeds in megabits (Mbps)"
          desc="Off shows megabytes per second (MB/s), what most file tools use. On shows megabits per second (Mbps), like internet plans."
        >
          <Toggle on={settings.showMegabits} onChange={(v) => save({ showMegabits: v })} />
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
        No accounts · No telemetry · Your files never touch a server
      </div>
    </div>
  )
}
