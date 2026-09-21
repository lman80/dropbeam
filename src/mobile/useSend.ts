import { useMemo, useRef, useState } from 'react'
import { pickAndSend } from '../lib/mobilePick'
import { useStore } from '../store'

/** Mobile presentation adapter for SendView's picker/list/receive flow.
 * The store remains the sole implementation of sending, receiving and retrying. */
export function useSend() {
  const transfers = useStore(s => s.transfers)
  const order = useStore(s => s.order)
  const [picking, setPicking] = useState(false)
  const pickingRef = useRef(false)
  const [receiving, setReceiving] = useState(false)
  const receivingRef = useRef(false)
  const [code, setCode] = useState('')
  const [showReceive, setShowReceive] = useState(false)
  const [receiveError, setReceiveError] = useState('')
  const list = useMemo(() => order.map(id => transfers[id]).filter(Boolean).reverse().filter(t => !(t.state === 'canceled' && t.fileNames.length === 0)), [order, transfers])
  const onPick = async (source: 'files' | 'photos' = 'files') => {
    if (pickingRef.current) return
    pickingRef.current = true; setPicking(true)
    try {
      await pickAndSend(source)
    } catch (error) { useStore.getState().toast('error', String(error)) }
    finally { pickingRef.current = false; setPicking(false); useStore.getState().setDragHovering(false) }
  }
  const submitReceive = async () => {
    if (!code.trim() || receivingRef.current) return
    receivingRef.current = true; setReceiving(true); setReceiveError('')
    try {
      if (await useStore.getState().receiveCode(code)) { setCode(''); setShowReceive(false) }
      else setReceiveError(useStore.getState().toasts.at(-1)?.message || 'Could not start receiving. Check the code and try again.')
    } finally { receivingRef.current = false; setReceiving(false) }
  }
  return { list, picking, onPick, code, setCode, showReceive, setShowReceive, receiving, receiveError, submitReceive }
}
