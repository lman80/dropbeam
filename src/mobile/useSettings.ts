import { useState } from 'react'
import { api, type Settings } from '../lib/api'
import { formatBytes } from '../lib/format'
import { useStore } from '../store'
import { reportError } from './shared'

/** Saves go through the existing optimistic update + rollback action. */
export function useSettings() {
  const settings = useStore(s => s.settings)
  const save = useStore(s => s.saveSettings)
  const [busy, setBusy] = useState<string | null>(null)
  const [testResult, setTestResult] = useState('')
  const run = async (key: string, task: () => Promise<void>) => {
    if (busy) return
    setBusy(key)
    try { await task() } catch (error) { reportError(error) } finally { setBusy(null) }
  }
  const clearCache = () => run('cache', async () => {
    const freed = await api.clearTransferCache()
    useStore.getState().toast('success', freed > 0 ? `Cleared ${formatBytes(freed)} of transfer leftovers.` : 'Nothing to clear — no transfer leftovers found.')
  })
  const testConnection = () => run('connection', async () => { setTestResult(await api.irohSelftest()) })
  const testDiagnostics = () => run('diagnostics', async () => { useStore.getState().toast('info', await api.diagnosticsTest()) })
  const exportLogs = () => run('export', async () => { const path = await api.exportDiagnostics(); await api.shareFiles([path]) })
  const field = <K extends keyof Settings>(key: K, value: Settings[K]) => save({ [key]: value })
  return { settings, save, field, busy, testResult, clearCache, testConnection, testDiagnostics, exportLogs }
}
