import { useEffect, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { Dialog } from './Dialog'
import { api, onFolderInvite, type FolderInvite } from '../lib/api'
import { useStore } from '../store'
import { Spinner } from './bits'

// There's no "declined" message in the protocol yet, so a Decline is remembered
// HERE: the inviter's re-sent beacon for the same invite never pops it again.
// (The inviter isn't told; their invite simply stays pending.)
const DECLINED_KEY = 'dropbeam-declined-folder-invites'
function declinedCodes(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(DECLINED_KEY) || '[]')
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
  } catch { return [] }
}
function rememberDeclined(code: string): void {
  try { localStorage.setItem(DECLINED_KEY, JSON.stringify([...declinedCodes().filter((c) => c !== code), code].slice(-100))) } catch { /* best-effort */ }
}

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
      // Ignore a duplicate beacon for an invite already queued (same code), and
      // one the user already declined.
      setQueue((q) => (q.some((x) => x.code === i.code) || declinedCodes().includes(i.code) ? q : [...q, i])),
    )
    return () => {
      un.then((f) => f()).catch(() => {})
    }
  }, [])

  // Remove a SPECIFIC invite: an accept that finishes after the user already
  // closed the prompt must not pop the NEXT queued invite unseen.
  const drop = (code: string) => setQueue((q) => q.filter((x) => x.code !== code))
  // × / Esc just put it away (it can come back with the next beacon); Decline sticks.
  const dismiss = () => { if (!busy && invite) drop(invite.code) }
  const decline = () => { if (!busy && invite) { rememberDeclined(invite.code); drop(invite.code) } }

  const accept = async () => {
    if (!invite || busy) return
    const current = invite
    // Busy from the moment the folder picker opens: a second click (or Enter)
    // must not open a second picker behind the first.
    setBusy(true)
    let folder: string | null = null
    try {
      folder = await api.pickDirectory()
    } catch (e) {
      toast('error', e)
    }
    if (!folder) { setBusy(false); return } // picker cancelled — keep the prompt open
    try {
      await api.acceptPair(current.code, folder)
      await reloadPairs()
      toast('success', `Joined “${current.folderName || 'shared folder'}”`)
      drop(current.code)
    } catch (e) {
      toast('error', e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <AnimatePresence>
      {invite && (
        <Dialog
          title={`Join “${invite.folderName || 'a shared folder'}”?`}
          width={400}
          onClose={dismiss}
          busy={busy}
          footer={
            <>
              <button className="btn btn-secondary" onClick={decline} disabled={busy}>
                Decline
              </button>
              <button className="btn btn-primary" onClick={accept} disabled={busy}>
                {busy ? <Spinner size={13} /> : null}
                Accept…
              </button>
            </>
          }
        >
          <p className="dialog-text" style={{ margin: 0 }}>
            {invite.fromName || 'A friend'} wants to share this folder with you. Choose where to keep it.
          </p>
        </Dialog>
      )}
    </AnimatePresence>
  )
}
