// "On your other devices" (GitHub #31): what the user's other linked devices
// (same account) are sending and receiving right now. Read-only; the engine
// (device_activity.rs) keeps it current and drops a device that goes quiet.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { create } from 'zustand'
import { HAS_TAURI, type TransferState } from './api'
import { mockListen, mockOtherDevices } from './mock'

export interface DeviceTransfer {
  id: string
  direction: 'send' | 'receive'
  state: TransferState
  /** Up to three file names; `fileCount` has the total. */
  names: string[]
  fileCount: number
  bytesDone: number
  bytesTotal: number
  percent: number
  speedBps: number
  /** Who it's going to / coming from, as that device knows them. */
  peer: string | null
}

export interface DeviceActivity {
  endpointId: string
  name: string
  kind: string | null
  os: string | null
  items: DeviceTransfer[]
  updatedMs: number
}

export const useOtherDevices = create<{ devices: DeviceActivity[] }>(() => ({ devices: [] }))

const set = (devices: DeviceActivity[]) => useOtherDevices.setState({ devices: devices.filter(d => d.items.length > 0) })

let started = false
/** Subscribe once (idempotent): the current digest now, then every change. */
export function startOtherDevices(): void {
  if (started) return
  started = true
  if (!HAS_TAURI) {
    set(mockOtherDevices())
    void mockListen('account://activity', p => set(p as DeviceActivity[]))
    return
  }
  void invoke<DeviceActivity[]>('other_device_activity').then(set).catch(() => {})
  void listen<DeviceActivity[]>('account://activity', e => set(e.payload ?? [])).catch(() => {})
}

/** "Sending to Alex" / "Receiving from Alex" / "Sent to Alex" … */
export function activityLine(t: DeviceTransfer): string {
  const send = t.direction === 'send'
  const who = t.peer ? `${send ? ' to' : ' from'} ${t.peer}` : ''
  switch (t.state) {
    case 'completed': return (send ? 'Sent' : 'Received') + who
    case 'failed': return (send ? 'Couldn’t send' : 'Couldn’t receive') + who
    case 'canceled': return 'Canceled'
    case 'paused': return 'Paused'
    case 'held': return 'Waiting on a Transfer Server' + who
    case 'waitingForPeer': return t.peer ? `Waiting for ${t.peer}` : 'Waiting for the other device'
    case 'waitingForAccept': return t.peer ? `Waiting for ${t.peer} to accept` : 'Waiting to be accepted'
    case 'starting': case 'connecting': return (send ? 'Getting ready to send' : 'Getting ready to receive') + who
    default: return (send ? 'Sending' : 'Receiving') + who
  }
}

/** "Drone footage.mov", "Drone footage.mov + 2 more", or "3 files". */
export function activityTitle(t: DeviceTransfer): string {
  const first = t.names[0]
  if (!first) return `${t.fileCount} file${t.fileCount === 1 ? '' : 's'}`
  return t.fileCount > 1 ? `${first} + ${t.fileCount - 1} more` : first
}
