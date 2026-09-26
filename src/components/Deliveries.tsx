import { Check } from 'lucide-react'
import type { Delivery } from '../lib/api'
import { deliveryIconKind, deliverySummary, deviceStatus } from '../lib/deliveries'
import { deviceIcon } from '../lib/deviceIcons'

/** One quiet line under a file sent to a friend with several devices:
 *  "Delivered to Alex’s Mac · iPhone: waiting (Linux Box is holding it)". */
export function DeliveryLine({ friend, deliveries }: { friend: string; deliveries: Delivery[] }) {
  const { text, tone } = deliverySummary(friend, deliveries)
  if (!text) return null
  return (
    <div className={`xfer-line delivery-line is-${tone}`} title={deliveries.map(d => `${d.label}: ${deviceStatus(d)}`).join('\n')}>
      <span>{text}</span>
    </div>
  )
}

/** The per-device rows on a transfer card: which of their devices has it. */
export function DeliveryRows({ deliveries }: { deliveries: Delivery[] }) {
  return (
    <ul className="delivery-rows" aria-label="Devices">
      {deliveries.map(d => {
        const Icon = deviceIcon(deliveryIconKind(d))
        const status = deviceStatus(d)
        return (
          <li key={d.eid} className={`delivery-row is-${d.state}`}>
            <Icon className="delivery-icon" aria-hidden />
            <span className="delivery-label">{d.label}</span>
            <span className="delivery-status truncate-1" title={status}>{status}</span>
            {d.state === 'delivered' && <Check className="delivery-check" aria-label="Delivered" strokeWidth={2.5} />}
          </li>
        )
      })}
    </ul>
  )
}
