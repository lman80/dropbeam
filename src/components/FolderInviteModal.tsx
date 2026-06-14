import { useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { FolderSync, X } from 'lucide-react'
import { api, onFolderInvite, type FolderInvite } from '../lib/api'
import { useStore } from '../store'
import { Spinner } from './bits'

/** Global listener + prompt for a friend inviting us directly into a shared folder.
 *  An invite arrives over iroh (folder-invite://incoming); we queue it and ask the
 *  user to accept + pick where to save the folder, then run the normal acceptPair. */
export function FolderInviteModal() {
  const reloadPairs = useStore((s) => s.reloadPairs)
  const toast = useStore((s) => s.toast)
  const [queue, setQueue] = useState<FolderInvite[]>([])
  const [busy, setBusy] = useState(false)
  const invite = queue[0]

  useEffect(() => {
    const un = onFolderInvite((i) =>
      // Ignore a duplicate beacon for an invite already queued (same code).
      setQueue((q) => (q.some((x) => x.code === i.code) ? q : [...q, i])),
    )
    return () => {
      un.then((f) => f()).catch(() => {})
    }
  }, [])

  const dismiss = () => setQueue((q) => q.slice(1))

  const accept = async () => {
    if (!invite) return
    const folder = await api.pickDirectory()
    if (!folder) return // user cancelled the folder picker — keep the prompt open
    setBusy(true)
    try {
      await api.acceptPair(invite.code, folder)
      await reloadPairs()
      toast('success', `Joined “${invite.folderName || 'shared folder'}”. Files will sync here.`)
      dismiss()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AnimatePresence>
      {invite && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onClick={dismiss}
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 60,
            display: 'grid',
            placeItems: 'center',
            background: 'rgba(0,0,0,0.45)',
            backdropFilter: 'blur(3px)',
          }}
        >
          <motion.div
            initial={{ scale: 0.96, y: 8 }}
            animate={{ scale: 1, y: 0 }}
            exit={{ scale: 0.96, y: 8 }}
            onClick={(e) => e.stopPropagation()}
            className="card"
            style={{ width: 'min(420px, 92vw)', padding: 22 }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
              <div
                style={{
                  width: 38,
                  height: 38,
                  borderRadius: 12,
                  display: 'grid',
                  placeItems: 'center',
                  background: 'var(--accent-soft)',
                  color: 'var(--accent)',
                  flexShrink: 0,
                }}
              >
                <FolderSync size={20} />
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 15, fontWeight: 750 }}>Shared folder invite</div>
                <div style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>
                  <b>{invite.fromName || 'A friend'}</b> wants to share{' '}
                  <b>“{invite.folderName || 'a folder'}”</b> with you.
                </div>
              </div>
              <button className="icon-btn" onClick={dismiss} aria-label="Decline">
                <X size={16} />
              </button>
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--text-faint)', marginBottom: 16, lineHeight: 1.45 }}>
              Accept and choose a folder on this computer to keep in sync. Anything either of you
              drops in will appear for both.
            </div>
            <div style={{ display: 'flex', gap: 10 }}>
              <button className="btn btn-ghost" style={{ flex: 1 }} onClick={dismiss} disabled={busy}>
                Decline
              </button>
              <button className="btn btn-primary" style={{ flex: 1.4 }} onClick={accept} disabled={busy}>
                {busy ? <Spinner size={15} /> : null}
                Accept &amp; choose folder
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
