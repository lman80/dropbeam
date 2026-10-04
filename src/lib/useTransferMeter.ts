import type { TransferUpdate } from './api'
import { etaAt } from './eta'
import { useStore } from '../store'

/**
 * The speed and time left one transfer shows — the SAME numbers on its
 * Send & Receive card, its chat bubble, the menu-bar popover and the floating
 * card. Prefers the window's own measured rate (live or average, per the global
 * toggles), falls back to the engine's speed, and always derives time left from
 * the speed it shows, so the two can't disagree.
 */
export function useTransferMeter(t: Pick<TransferUpdate, 'id' | 'state' | 'bytesDone' | 'bytesTotal' | 'speedBps' | 'etaSeconds'>) {
  const rates = useStore((s) => s.transferRates[t.id])
  const speedMode = useStore((s) => s.speedMode)
  const etaMode = useStore((s) => s.etaMode)
  const engineBps = t.speedBps > 0 ? t.speedBps : null
  const speedBps =
    (speedMode === 'live' ? rates?.liveBps ?? rates?.avgBps : rates?.avgBps ?? rates?.liveBps) ?? engineBps
  const etaBps =
    (etaMode === 'avg' ? rates?.avgBps ?? rates?.liveBps : rates?.liveBps ?? rates?.avgBps) ?? engineBps
  const etaSeconds = etaAt(t.bytesDone, t.bytesTotal, etaBps) ?? (t.etaSeconds != null && t.etaSeconds > 0 ? t.etaSeconds : null)
  const settling = t.state === 'transferring' && !!rates && !rates.settled
  return { speedBps, etaSeconds, settling, speedMode, etaMode }
}
