// Settings → Agents. AI agents on this computer (Claude Code, Codex…) use
// DropBeam with the same abilities as the person, through the `dropbeam`
// command or its MCP server — and everything they do is listed here.
import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Check, Copy } from 'lucide-react'
import { agentsApi, type AgentAction, type AgentPanel } from '../lib/agents'
import { dateTime, clockTime } from '../lib/dates'
import { errorText } from '../lib/errors'
import { IS_MAC, IS_WINDOWS } from '../lib/platform'
import { useStore } from '../store'
import { SectionHeader, Segmented, Spinner, Toggle } from './ui'

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

function Snippet({ label, text }: { label: string; text: string }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }
  return (
    <div className="agent-snippet">
      <div className="row-sub">{label}</div>
      <div className="agent-snippet-line">
        <code className="selectable">{text}</code>
        <button className="btn btn-secondary btn-sm" onClick={copy} aria-label={`Copy: ${label}`}>
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
    </div>
  )
}

type ExecMode = 'off' | 'hour' | 'always'

export function AgentsPane() {
  const toast = useStore((s) => s.toast)
  const [panel, setPanel] = useState<AgentPanel | null>(null)
  const [log, setLog] = useState<AgentAction[]>([])
  const [installing, setInstalling] = useState(false)
  const [confirmAlways, setConfirmAlways] = useState(false)

  const refresh = useCallback(() => {
    void agentsApi.panel().then(setPanel).catch(() => {})
    void agentsApi.log(200).then((l) => setLog(l.slice().reverse())).catch(() => {})
  }, [])
  useEffect(() => {
    refresh()
    const un = agentsApi.onChange(refresh)
    // "For 1 hour" ends on its own: keep the status honest.
    const tick = setInterval(refresh, 60_000)
    return () => {
      clearInterval(tick)
      void un.then((f) => f())
    }
  }, [refresh])

  if (!panel) return <div className="settings-loading"><Spinner /></div>

  const cmd = panel.cli.installed ? 'dropbeam' : panel.cli.direct ?? 'dropbeam'
  const execMode: ExecMode = !panel.execOn ? 'off' : panel.execUntil ? 'hour' : 'always'
  const setExec = async (mode: ExecMode) => {
    if (mode === 'always' && !confirmAlways) {
      setConfirmAlways(true)
      return
    }
    setConfirmAlways(false)
    try {
      setPanel(await agentsApi.setExec(mode))
    } catch (e) {
      toast('error', e)
    }
  }
  const install = async () => {
    setInstalling(true)
    try {
      toast('info', await agentsApi.installCli())
      refresh()
    } catch (e) {
      toast('error', errorText(e))
    } finally {
      setInstalling(false)
    }
  }
  const terminal = IS_MAC ? 'Terminal' : IS_WINDOWS ? 'PowerShell or Command Prompt' : 'a terminal'

  return (
    <>
      <SectionHeader>Agents</SectionHeader>
      <div className="group">
        <Row
          title="Let AI agents on this computer use DropBeam"
          sub="Agents like Claude Code or Codex can then do what you can here: send files, chat, browse Locations. Only programs running as you on this computer can connect. Everything they do is listed below."
        >
          <Toggle
            on={panel.access}
            label="Let AI agents on this computer use DropBeam"
            onChange={(on) => void agentsApi.setAccess(on).then(setPanel).catch((e) => toast('error', e))}
          />
        </Row>
        <Row
          title="Command-line tool"
          sub={panel.cli.installed ? `Installed: ${panel.cli.path}. Type dropbeam help in ${terminal}.` : `Adds the dropbeam command, so you and your agents can type dropbeam send, dropbeam chat… in ${terminal}.`}
        >
          <button className="btn btn-secondary" onClick={() => void install()} disabled={installing}>
            {installing ? <Spinner size={14} /> : panel.cli.installed ? 'Reinstall' : 'Install Command-Line Tool'}
          </button>
        </Row>
      </div>

      <SectionHeader>Connect an AI agent</SectionHeader>
      <div className="group agent-snippets">
        <Snippet label="Claude Code" text={`claude mcp add dropbeam -- ${cmd} mcp`} />
        <Snippet label="Codex" text={`codex mcp add dropbeam -- ${cmd} mcp`} />
        <Snippet label="Any agent with a terminal" text={`${cmd} help`} />
        <div className="row-sub agent-note">
          Agents can message each other through DropBeam: one sends with chat, the other waits for it with wait (or the chat_wait tool) and answers the same way.
        </div>
      </div>

      {panel.execSupported && (
        <>
          <SectionHeader>Run commands from your other devices</SectionHeader>
          <div className="group">
            <Row
              title="Let my other devices run commands here"
              sub={
                execMode === 'off'
                  ? 'Off. Your other devices (and agents on them) can’t run anything on this computer.'
                  : execMode === 'hour'
                    ? `On until ${clockTime(panel.execUntil ?? 0)}. Then it turns itself off.`
                    : 'On until you turn it off.'
              }
            >
              <Segmented<ExecMode>
                label="Let my other devices run commands here"
                value={execMode}
                onChange={(m) => void setExec(m)}
                options={[
                  { value: 'off', label: 'Off' },
                  { value: 'hour', label: 'For 1 Hour' },
                  { value: 'always', label: 'Until I Turn It Off' },
                ]}
              />
            </Row>
            {confirmAlways && (
              <Row title="Leave it on until you turn it off?" sub="Any of your linked devices could run commands here at any time, not just while you set this computer up.">
                <button className="btn btn-secondary" onClick={() => setConfirmAlways(false)}>Cancel</button>
                <button className="btn btn-danger" onClick={() => void setExec('always')}>Turn On</button>
              </Row>
            )}
            <div className="agent-warning" role="note">
              While this is on, any device linked to your DropBeam account (Settings → Devices) — and any AI agent you let use it —
              can run any command on this computer as you. Friends never can. Each command shows a notification here and is listed below.
            </div>
            <div className="row-sub agent-note">
              <strong>Set up a new computer with your agent:</strong> install DropBeam on it, link it to your account (Settings → Devices,
              scan the code), and turn this on there for 1 hour. Your agent on your other computer can then run commands on it,
              for example <code>dropbeam exec "{'<this computer>'}" -- whoami</code> — to install SSH keys or anything else it needs.
            </div>
          </div>
        </>
      )}

      <SectionHeader action={log.length ? <button className="btn btn-secondary btn-sm" onClick={() => void agentsApi.clearLog().then(refresh)}>Clear</button> : undefined}>
        What agents did
      </SectionHeader>
      <div className="group agent-log">
        {log.length === 0 && <Row title="Nothing yet" sub="When an agent uses DropBeam on this computer, it shows up here: who, what, why, and whether it worked." />}
        {log.slice(0, 200).map((a, i) => (
          <div className="row set-row agent-log-row" key={`${a.t}-${i}`}>
            <div className="row-main">
              <div className="row-title">
                <span className={a.ok ? 'agent-ok' : 'agent-failed'}>{a.ok ? '✓' : '✕'}</span> {a.agent} · {a.summary}
                {a.dryRun ? ' (dry run)' : ''}
              </div>
              <div className="row-sub">
                {dateTime(a.t)}
                {a.reason ? ` · “${a.reason}”` : ''}
                {!a.ok && (a.message || a.error) ? ` · ${a.message || a.error}` : ''}
              </div>
            </div>
          </div>
        ))}
      </div>
    </>
  )
}
