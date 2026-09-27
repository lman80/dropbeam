import { useEffect } from 'react'
import { Check, X } from 'lucide-react'
import { deviceIcon } from '../lib/deviceIcons'
import { formatBytesLive, formatSpeed } from '../lib/format'
import { activityLine, activityTitle, startOtherDevices, useOtherDevices, type DeviceActivity, type DeviceTransfer } from '../lib/otherDevices'
import { ProgressBar, SectionHeader } from './ui'

/** "On your other devices": live, read-only transfers on the user's other linked
 *  devices. Renders nothing while nothing is happening elsewhere. */
export function OtherDevices() {
  const devices = useOtherDevices(s => s.devices)
  useEffect(startOtherDevices, [])
  if (!devices.length) return null
  return (
    <section className="other-devices" aria-label="On your other devices">
      <SectionHeader>On your other devices</SectionHeader>
      <div className="group xfer-list">
        {devices.flatMap(d => d.items.map(t => <Row key={`${d.endpointId}:${t.id}`} device={d} t={t} />))}
      </div>
    </section>
  )
}

function Row({ device, t }: { device: DeviceActivity; t: DeviceTransfer }) {
  const Icon = deviceIcon(device.os === 'macos' && device.kind !== 'desktop' ? 'laptop' : device.kind ?? undefined)
  const done = t.state === 'completed'
  const bad = t.state === 'failed' || t.state === 'canceled'
  const live = !done && !bad && t.state !== 'held'
  const moving = t.state === 'transferring' && t.bytesTotal > 0
  const stats = moving
    ? `${formatBytesLive(t.bytesDone)} of ${formatBytesLive(t.bytesTotal)}${t.speedBps > 0 ? ` · ${formatSpeed(t.speedBps)}` : ''}`
    : null
  return (
    <div className={`xfer-row other-device-row${bad ? ' is-muted' : ''}`}>
      <div className="xfer-head">
        <span className="xfer-icon other-device-icon" aria-hidden>
          <Icon size={18} strokeWidth={1.7} />
          {(done || bad) && <span className={`xfer-badge ${done ? 'ok' : 'bad'}`}>{done ? <Check size={9} strokeWidth={3} /> : <X size={9} strokeWidth={3} />}</span>}
        </span>
        <div className="xfer-main">
          <div className="xfer-title truncate-1" title={t.names.join('\n') || undefined}>{activityTitle(t)}</div>
          <div className="xfer-meta truncate-1">
            <span className="other-device-name">{device.name}</span> · {activityLine(t)}{stats ? ` · ${stats}` : ''}
          </div>
        </div>
        {moving && <span className="other-device-pct tnum">{Math.floor(t.percent)}%</span>}
      </div>
      {live && t.bytesTotal > 0 && (
        <div className="xfer-progress">
          <ProgressBar percent={t.percent} tone={t.state === 'paused' ? 'paused' : undefined} label={`${activityTitle(t)} on ${device.name}`} />
        </div>
      )}
    </div>
  )
}
