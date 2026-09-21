import { deviceIcon, deviceKindLabel } from '../lib/deviceIcons'
const icons = { phone: deviceIcon('phone'), tablet: deviceIcon('tablet'), laptop: deviceIcon('laptop'), desktop: deviceIcon('desktop') }
const fallbackIcon = deviceIcon()
export function DeviceBadge({ kind }: { kind?: string | null }) {
  if (!kind) return null
  const Icon = Object.hasOwn(icons, kind) ? icons[kind as keyof typeof icons] : fallbackIcon
  return <span className="device-badge" title={deviceKindLabel(kind)}><Icon size={16} aria-label={deviceKindLabel(kind)} /></span>
}
