import { useEffect, useState } from 'react'
import { Activity, Bell, Copy, FlaskConical, HardDrive, MessageCircle, Moon, Network, Radio, Share2, Smartphone, Trash2, Volume2, Wifi } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { api, fileSrc, type Settings } from '../lib/api'
import { deviceKindLabel } from '../lib/deviceIcons'
import { useStore } from '../store'
import { LinkDeviceModal, LinkNewDeviceModal } from '../components/LinkDeviceModal'
import { ActionSheet, Avatar, Button, IconSquare, Row, Screen, Section, TextField } from './kit'
import { copyText, NameAlert, reportError } from './shared'
import { useSettings } from './useSettings'

export type SettingsPage = 'settings' | 'profile' | 'appearance' | 'messages' | 'connection' | 'relay' | 'diagnostics' | 'lab'
const titles: Record<SettingsPage, string> = { settings: 'Settings', profile: 'Profile', appearance: 'Appearance', messages: 'Messages', connection: 'Connection', relay: 'Custom Relay', diagnostics: 'Diagnostics', lab: 'Lab Mode' }
type BooleanKey = { [K in keyof Settings]: Settings[K] extends boolean ? K : never }[keyof Settings]
export function SettingsScreen({ page, push, back }: { page: SettingsPage; push: (page: SettingsPage) => void; back: () => void }) {
  const model = useSettings()
  const { settings, save } = model
  const appVer = useStore(s => s.appVer)
  const myDevice = useStore(s => s.myDevice)
  const myEid = useStore(s => s.myEid)
  const [clear, setClear] = useState(false)
  const [rename, setRename] = useState(false)
  const [picture, setPicture] = useState(false)
  const [link, setLink] = useState<'new' | 'this' | null>(null)
  const [code, setCode] = useState('')
  const [codeError, setCodeError] = useState('')
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(reportError) }, [])
  useEffect(() => {
    if (page !== 'profile') return
    let alive = true
    void api.myInviteCode().then(value => { if (alive) { setCode(value); setCodeError('') } }).catch(error => { if (alive) setCodeError(String(error)) })
    return () => { alive = false }
  }, [page, settings?.displayName])
  if (!settings) return null
  const toggle = (key: BooleanKey, title: string, icon?: React.ReactNode, disabled = false) => <Row key={key} title={title} icon={icon} accessory="toggle" checked={!!settings[key] && !disabled} disabled={disabled} onChange={value => void model.field(key, value)} />
  const field = (key: 'customRelay' | 'giphyApiKey' | 'diagnosticsUrl' | 'labOperatorId', label: string, placeholder = '') => <TextField label={label} placeholder={placeholder} value={settings[key]} onChange={e => void model.field(key, e.target.value)} />
  const profileAvatar = <Avatar name={settings.displayName || 'You'} src={settings.avatar ? fileSrc(settings.avatar) : undefined} size={80} />
  const shareCode = async () => {
    if (!code) return
    if (!navigator.share) { await copyText(code); return }
    try { await navigator.share({ title: 'DropBeam Code', text: code }) } catch (error) { if (!(error instanceof Error && error.name === 'AbortError')) reportError(error) }
  }
  return <Screen title={titles[page]} leading={page !== 'settings' ? { title: 'Settings', onPress: back } : undefined}>
    {page === 'settings' && <>
      <Section><Row avatar={<Avatar name={settings.displayName || 'You'} src={settings.avatar ? fileSrc(settings.avatar) : undefined} size={56} />} title={settings.displayName || 'You'} emphasized subtitle="DropBeam code, devices" accessory="chevron" onPress={() => push('profile')} /></Section>
      <Section title="General"><Row icon={<IconSquare color="var(--mk-gray)"><Moon /></IconSquare>} title="Appearance" value={{ system: 'System', light: 'Light', dark: 'Dark' }[settings.theme]} accessory="chevron" onPress={() => push('appearance')} />{toggle('playSounds', 'Sounds', <IconSquare color="var(--mk-red)"><Volume2 /></IconSquare>)}{toggle('notifyOnComplete', 'Notifications', <IconSquare color="var(--mk-red)"><Bell /></IconSquare>)}<Row icon={<IconSquare color="var(--mk-green)"><MessageCircle /></IconSquare>} title="Messages" accessory="chevron" onPress={() => push('messages')} /></Section>
      <Section title="Transfers">{toggle('preferDirectP2p', 'Prefer direct connections', <IconSquare><Wifi /></IconSquare>)}{toggle('waitForDirect', 'Wait for a direct link', <IconSquare color="var(--mk-green)"><Radio /></IconSquare>, settings.requireDirect)}<Row icon={<IconSquare><Network /></IconSquare>} title="Connection" accessory="chevron" onPress={() => push('connection')} /><Row icon={<IconSquare color="var(--mk-gray)"><Trash2 /></IconSquare>} title="Clear transfer cache" tint disabled={!!model.busy} onPress={() => setClear(true)} /></Section>
      <Section title="Advanced"><Row icon={<IconSquare color="var(--mk-gray)"><HardDrive /></IconSquare>} title="Custom relay" accessory="chevron" onPress={() => push('relay')} /><Row icon={<IconSquare color="var(--mk-green)"><Activity /></IconSquare>} title="Diagnostics" accessory="chevron" onPress={() => push('diagnostics')} /><Row icon={<IconSquare color="var(--mk-orange)"><FlaskConical /></IconSquare>} title="Lab Mode" accessory="chevron" onPress={() => push('lab')} /></Section>
      <Section title="About"><Row title="Version" value={appVer || '…'} /></Section>
    </>}
    {page === 'profile' && <>
      <div className="mk-profile"><Button aria-label="Change picture" onClick={() => setPicture(true)}>{profileAvatar}</Button></div>
      <Section><Row title="Name" value={settings.displayName} accessory="chevron" onPress={() => setRename(true)} /></Section>
      <Section title="Your Code" footer={codeError ? <span className="mk-error" role="alert">{codeError}</span> : undefined}>{code ? <div className="mk-qr"><QRCodeSVG value={code} size={240} level="M" /></div> : <Row title={codeError ? 'Code unavailable' : 'Loading code…'} />}<Row title="Copy Code" icon={<IconSquare><Copy /></IconSquare>} tint disabled={!code} onPress={() => void copyText(code)} /><Row title="Share Code" icon={<IconSquare><Share2 /></IconSquare>} tint disabled={!code} onPress={() => void shareCode()} /></Section>
      <Section title="Devices" footer={myDevice ? `${myDevice.linked_devices} linked device${myDevice.linked_devices === 1 ? '' : 's'}` : undefined}><Row icon={<IconSquare color="var(--mk-gray)"><Smartphone /></IconSquare>} title="This iPhone" value={myDevice ? deviceKindLabel(myDevice.device_kind) : '…'} /><Row title="Link a New Device" tint accessory="chevron" onPress={() => setLink('new')} /><Row title="Link This Device to Another Account" tint accessory="chevron" onPress={() => setLink('this')} /></Section>
    </>}
    {page === 'appearance' && <Section>{(['system', 'light', 'dark'] as const).map(theme => <Row key={theme} title={{ system: 'System', light: 'Light', dark: 'Dark' }[theme]} accessory="checkmark" checked={settings.theme === theme} onPress={() => void save({ theme })} />)}</Section>}
    {page === 'messages' && <>
      <Section footer="Chat notifications appear outside the conversation. Delivery may pause while DropBeam is in the background.">{toggle('notifyOnMessage', 'Chat notifications')}{toggle('sendReadReceipts', 'Read receipts')}</Section>
      <Section title="Giphy Key" footer="Add a free key from developers.giphy.com to enable GIFs. Leave blank to hide the picker.">{field('giphyApiKey', 'Giphy API key', 'API key')}</Section>
    </>}
    {page === 'connection' && <>
      <Section footer="Keep both devices open until transfers finish. Transfers are end-to-end encrypted."><Row title="Direct peer-to-peer" value="On" /><Row title="Test connection" subtitle={model.testResult || undefined} tint disabled={!!model.busy} onPress={() => void model.testConnection()} /></Section>
      <Section title="Direct Connections" footer="Direct connections only fails a send if a direct path cannot be made. Shared folders use the best available path. Waiting is unavailable while direct connections are required.">{toggle('requireDirect', 'Direct connections only')}{toggle('waitForDirect', 'Wait for a direct link', undefined, settings.requireDirect)}</Section>
      <Section footer="Parallel streams use several connections for files over 16 MB. Turn off if transfers stall.">{toggle('parallelStreams', 'Parallel streams')}</Section>
      <Section title="Upload Limit" footer="Mbps; 0 means unlimited. Local transfers run at full speed. Start at 100 Mbps and adjust if Wi-Fi stutters."><TextField label="Upload limit (Mbps)" type="number" min={0} max={100000} inputMode="numeric" value={settings.uploadLimitMbps || 0} onChange={e => void save({ uploadLimitMbps: Math.min(100000, Math.max(0, Math.floor(Number(e.target.value) || 0))) })} /></Section>
      <Section>{toggle('showMegabits', 'Speeds in megabits')}</Section>
      <Section title="Local Network Access" footer="If nearby transfers use the relay, enable DropBeam in iOS Settings → Privacy & Security → Local Network on both devices." />
      <Section title="How Transfers Connect" footer={<><p>Local: Files travel directly over the same Wi-Fi or network, without the internet.</p><p>Direct: An encrypted peer-to-peer link connects both devices across the internet.</p><p>Relay: An encrypted relay carries files when a direct path is unavailable. It cannot read them, but may be slower.</p><p>Connecting: Finding the best available route to the other device.</p></>} />
    </>}
    {page === 'relay' && <Section title="Relay URL" footer="Use the same relay on both devices; leave blank for public relays. Close and reopen DropBeam to apply. Setup: github.com/lman80/dropbeam → RELAY-SETUP.md">{field('customRelay', 'Relay URL', 'https://…')}</Section>}
    {page === 'diagnostics' && <>
      <Section footer="Extra network logs for reproducing issues. Close and reopen DropBeam to apply.">{toggle('verboseLogging', 'Detailed logging')}</Section>
      <Section footer="Send a redacted error and performance summary about once a day. Never includes file names or contents.">{toggle('shareDiagnostics', 'Background diagnostics')}</Section>
      {settings.shareDiagnostics && <Section title="Diagnostics Endpoint" footer="Leave blank for the built-in collector. Override only if you run your own.">{field('diagnosticsUrl', 'Diagnostics endpoint', 'Built-in')}<Row title="Send test" tint disabled={!!model.busy || (!!settings.diagnosticsUrl && !settings.diagnosticsUrl.startsWith('https://'))} onPress={() => void model.testDiagnostics()} /></Section>}
      <Section footer="Export diagnostics from DropBeam’s folder in Files to share for troubleshooting."><Row title={model.busy === 'export' ? 'Exporting…' : 'Export logs'} tint disabled={!!model.busy} onPress={() => void model.exportLogs()} /></Section>
    </>}
    {page === 'lab' && <>
      <Section footer="Allow one trusted developer device to run encrypted diagnostics. Only the operator ID below is accepted. Enable only when asked by the developer.">{toggle('labModeEnabled', 'Enable Lab Mode')}</Section>
      {settings.labModeEnabled && <><Section title="Operator ID" footer="Only this device can run Lab Mode. Blank accepts no device.">{field('labOperatorId', 'Operator device ID', 'Device ID')}</Section><Section footer="Share this device’s ID with the developer for testing."><Row title="Copy This Device’s ID" tint disabled={!myEid} onPress={() => { if (myEid) void copyText(myEid) }} /></Section></>}
    </>}
    {clear && <ActionSheet title="Clear Transfer Cache" message="Remove interrupted transfer leftovers? Received files are kept. Old leftovers are cleaned automatically after a week." onClose={() => setClear(false)} actions={[{ label: 'Clear Transfer Cache', destructive: true, onPress: () => void model.clearCache() }]} />}
    {rename && <NameAlert initial={settings.displayName} onSave={name => save({ displayName: name })} onClose={() => setRename(false)} />}
    {picture && <ActionSheet title="Profile Picture" onClose={() => setPicture(false)} actions={[{ label: 'Change Picture', onPress: () => void useStore.getState().pickAvatar() }, ...(settings.avatar ? [{ label: 'Remove Picture', destructive: true, onPress: () => void useStore.getState().clearAvatar() }] : [])]} />}
    {link === 'new' && <LinkNewDeviceModal onClose={() => setLink(null)} />}
    {link === 'this' && <LinkDeviceModal onClose={() => setLink(null)} />}
  </Screen>
}
