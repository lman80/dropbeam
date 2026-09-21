import { useState } from 'react'
import { ArrowDownToLine, Copy, Folder, Images, RotateCw, X } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { api, isActive, type TransferUpdate } from '../lib/api'
import { formatBytes, formatSpeed } from '../lib/format'
import { useStore } from '../store'
import { FileIcon } from '../components/FileIcon'
import { Button, IconSquare, ProgressRow, Row, Screen, Section, Sheet, TextField } from './kit'
import { useSend } from './useSend'
import { transferSharePaths as sharePaths } from '../lib/mobilePick'
import { copyText, reportError } from './shared'

function statusLine(t: TransferUpdate) {
  if (t.state === 'completed') return `${t.direction === 'send' ? 'Sent' : 'Received'} · ${formatBytes(t.bytesTotal)}`
  if (t.state === 'failed') return 'Failed · Retry'
  if (t.state === 'paused') return 'Paused · Resume'
  if (t.state === 'canceled') return 'Canceled'
  if (t.state === 'transferring') return `${t.direction === 'send' ? `Sending${t.friendName ? ` to ${t.friendName}` : ''}` : 'Receiving'} · ${formatBytes(t.bytesDone)} of ${formatBytes(t.bytesTotal)} · ${formatSpeed(t.speedBps)}`
  return t.detail || ({ starting: 'Starting…', waitingForPeer: 'Waiting for a connection', connecting: 'Connecting…', waitingForAccept: 'Waiting for acceptance' }[t.state] ?? t.state)
}
export function SendScreen() {
  const send = useSend()
  const [selected, setSelected] = useState<string | null>(null)
  const transfer = send.list.find(t => t.id === selected)
  const retry = (t: TransferUpdate) => {
    if (t.direction === 'receive') { if (t.code) void useStore.getState().receiveCode(t.code); else { send.setCode(''); send.setShowReceive(true) } }
    else void useStore.getState().retryTransfer(t.id)
  }
  return <Screen title="Send">
    <Section><Row icon={<IconSquare><Images /></IconSquare>} title="Photos and Videos" accessory="chevron" disabled={send.picking} onPress={() => void send.onPick('photos')} /><Row icon={<IconSquare><Folder /></IconSquare>} title="Files" accessory="chevron" disabled={send.picking} onPress={() => void send.onPick()} /></Section>
    <Section title="Receive"><Row icon={<IconSquare color="var(--mk-green)"><ArrowDownToLine /></IconSquare>} title="Enter a receive code" accessory="chevron" onPress={() => send.setShowReceive(true)} /></Section>
    <Section title="Transfers" footer={!send.list.length ? 'Files you send or receive appear here.' : undefined}>
      {send.list.map(t => {
        const active = isActive(t.state)
        const waiting = t.state === 'waitingForPeer' && !!t.code
        const offer = t.state === 'waitingForAccept' && t.direction === 'receive'
        const completed = t.state === 'completed'
        const failed = t.state === 'failed' || t.state === 'paused'
        const title = t.fileCount > 1 ? `${t.fileCount} files` : t.fileNames[0] || 'Files'
        const open = () => {
          const paths = completed ? sharePaths(t) : []
          if (paths.length) void api.shareFiles(paths).catch(reportError)
          else setSelected(t.id)
        }
        return <ProgressRow key={t.id} icon={<span className="mk-file-icon"><FileIcon name={t.fileNames[0] || ''} size={24} /></span>} title={title} subtitle={offer ? 'Files offered · Tap to review' : statusLine(t)} value={waiting ? t.code : undefined} progress={active ? t.percent : null} accessory={completed ? 'chevron' : 'none'} onPress={failed ? () => retry(t) : open} trailing={active ? <Button aria-label={`Cancel ${title}`} onClick={() => { if (offer) void useStore.getState().respondToOffer(t.id, false); else void api.cancelTransfer(t.id).catch(reportError) }}><X size={20} /></Button> : failed ? <Button aria-label={`${t.state === 'paused' ? 'Resume' : 'Retry'} ${title}`} onClick={() => retry(t)}><RotateCw size={20} /></Button> : undefined} />
      })}
    </Section>
    {send.showReceive && <Sheet title="Receive files" onClose={() => send.setShowReceive(false)} primary={<Button disabled={!send.code.trim() || send.receiving} onClick={() => void send.submitReceive()}>{send.receiving ? 'Receiving…' : 'Done'}</Button>}><form onSubmit={e => { e.preventDefault(); void send.submitReceive() }}><Section footer={send.receiveError ? <span className="mk-error" role="alert">{send.receiveError}</span> : undefined}><TextField label="Receive code" placeholder="Code" autoFocus value={send.code} onChange={e => send.setCode(e.target.value)} /></Section></form></Sheet>}
    {transfer && <Sheet title={transfer.state === 'waitingForPeer' ? 'Send files' : 'Transfer'} onClose={() => setSelected(null)}>
      <Section footer={transfer.error || transfer.detail || (transfer.state === 'completed' && !sharePaths(transfer).length ? 'The original files are no longer available to share from this transfer.' : statusLine(transfer))}><Row title={transfer.fileCount > 1 ? `${transfer.fileCount} files` : transfer.fileNames[0] || 'Files'} value={formatBytes(transfer.bytesTotal)} /></Section>
      {transfer.code && <Section><div className="mk-qr"><QRCodeSVG value={transfer.code} size={240} level="M" /></div><Row title="Copy Code" icon={<IconSquare><Copy /></IconSquare>} onPress={() => void copyText(transfer.code!)} tint /></Section>}
      {transfer.state === 'waitingForAccept' && transfer.direction === 'receive' && <Section><Row title="Accept Files" tint onPress={() => { void useStore.getState().respondToOffer(transfer.id, true); setSelected(null) }} /><Row title="Decline" destructive onPress={() => { void useStore.getState().respondToOffer(transfer.id, false); setSelected(null) }} /></Section>}
      {transfer.detail && transfer.state === 'waitingForPeer' && <Section><Row title="Send over relay anyway" tint onPress={() => void api.forceRelay(transfer.id).catch(reportError)} /></Section>}
      {!isActive(transfer.state) && <Section><Row title="Dismiss Transfer" onPress={() => { useStore.getState().removeTransfer(transfer.id); setSelected(null) }} tint /></Section>}
    </Sheet>}
  </Screen>
}
