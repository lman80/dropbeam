import { useState } from 'react'
import { Share2 } from 'lucide-react'
import { api } from '../lib/api'
import { useStore } from '../store'

/** Share the actual received files, including multi-file transfers. */
export function ShareFilesButton({ outDir, fileNames }: { outDir: string; fileNames: string[] }) {
  const [busy, setBusy] = useState(false)
  return (
    <button className="btn btn-ghost" disabled={busy || !fileNames.length} title="Share files or save photos and videos" onClick={async () => {
      setBusy(true)
      try {
        await api.shareFiles(fileNames.map((name) => `${outDir}/${name}`))
      } catch (error) {
        useStore.getState().toast('error', String(error))
      } finally {
        setBusy(false)
      }
    }}>
      <Share2 size={15} /> Share
    </button>
  )
}
