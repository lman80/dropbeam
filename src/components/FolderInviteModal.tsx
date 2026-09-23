import { useEffect, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { FolderSync } from 'lucide-react'
import { Dialog } from './Dialog'
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

  // Remove a SPECIFIC invite: an accept that finishes after the user already
  // closed the prompt must not pop the NEXT queued invite unseen.
  const drop = (code: string) => setQueue((q) => q.filter((x) => x.code !== code))
  const dismiss = () => { if (!busy && invite) drop(invite.code) }

  const accept = async () => {
    if (!invite || busy) return
    const current = invite
    const folder = await api.pickDirectory()
    if (!folder) return // user cancelled the folder picker — keep the prompt open
    setBusy(true)
    try {
      await api.acceptPair(current.code, folder)
      await reloadPairs()
      toast('success', `Joined “${current.folderName || 'shared folder'}”. Files will sync here.`)
      drop(current.code)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AnimatePresence>
      {invite && (
        <Dialog
          title="Shared folder invite"
          subtitle={<><b>{invite.fromName || 'A friend'}</b> wants to share <b>“{invite.folderName || 'a folder'}”</b> with you.</>}
          icon={<FolderSync size={19} />}
          width={420}
          onClose={dismiss}
          busy={busy}
          footer={
            <>
              <button className="btn btn-ghost" onClick={dismiss} disabled={busy}>
                Decline
              </button>
              <button className="btn btn-primary" onClick={accept} disabled={busy}>
                {busy ? <Spinner size={15} /> : null}
                Accept &amp; choose folder
              </button>
            </>
          }
        >
          <p className="dialog-text" style={{ margin: 0 }}>
            Accept and choose a folder on this computer to keep in sync. Anything either of you
            drops in will appear for both.
          </p>
        </Dialog>
      )}
    </AnimatePresence>
  )
}
