// Settings → Agents: the switches only a person can flip (agent access, letting
// your other devices run commands here), the activity log, and the
// "Install command-line tool" button. See src-tauri/src/agent_ctl.rs.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { HAS_TAURI } from './api'

export interface AgentPanel {
  access: boolean
  execOn: boolean
  /** When "run commands" turns itself off (ms); null = until turned off. */
  execUntil: number | null
  execSupported: boolean
  cli: { installed: boolean; path: string | null; direct: string | null }
}

export interface AgentAction {
  t: number
  agent: string
  reason?: string | null
  op: string
  summary: string
  ok: boolean
  error?: string
  message?: string
  dryRun?: boolean
}

const OFF: AgentPanel = { access: true, execOn: false, execUntil: null, execSupported: true, cli: { installed: false, path: null, direct: null } }

export const agentsApi = {
  panel: () => (HAS_TAURI ? invoke<AgentPanel>('agent_settings') : Promise.resolve(OFF)),
  setAccess: (on: boolean) => invoke<AgentPanel>('agent_set_access', { on }),
  setExec: (mode: 'off' | 'hour' | 'always') => invoke<AgentPanel>('agent_set_exec', { mode }),
  log: (limit = 200) => (HAS_TAURI ? invoke<AgentAction[]>('agent_log', { limit }) : Promise.resolve([] as AgentAction[])),
  clearLog: () => invoke<void>('agent_clear_log'),
  installCli: () => invoke<string>('agent_install_cli'),
  /** A new log line or a changed switch. */
  onChange: (cb: () => void): Promise<UnlistenFn> => {
    if (!HAS_TAURI) return Promise.resolve(() => {})
    return Promise.all([listen('agents://action', cb), listen('agents://changed', cb)]).then((fns) => () => fns.forEach((f) => f()))
  },
}
