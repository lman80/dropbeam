import { useEffect, useRef } from 'react'
import { createRoot } from 'react-dom/client'
import { File, Images } from 'lucide-react'

type Source = 'photos' | 'files'
let picking = false

/** One modal for every file-picking entry point; native pickers keep ownership
 * of permissions and return durable local paths through the existing commands. */
export async function pickMobileFiles(pickers: Record<Source, () => Promise<string[]>>): Promise<string[]> {
  if (picking) return []
  picking = true
  const previous = document.activeElement
  if (previous instanceof HTMLElement) previous.blur()
  try {
    const source = await new Promise<Source | null>((resolve) => {
      const host = document.createElement('div')
      document.body.append(host)
      const root = createRoot(host)
      const finish = (choice: Source | null) => {
        root.unmount()
        host.remove()
        resolve(choice)
      }
      root.render(<MobileFileSheet finish={finish} />)
    })
    return source ? await pickers[source]() : []
  } finally {
    picking = false
    // Do not reopen the keyboard after attaching a file.
    if (previous instanceof HTMLButtonElement && previous.isConnected) previous.focus({ preventScroll: true })
  }
}

function MobileFileSheet({ finish }: { finish: (source: Source | null) => void }) {
  const dialog = useRef<HTMLDialogElement>(null)
  useEffect(() => { dialog.current?.showModal() }, [])
  return (
    <dialog ref={dialog} className="mobile-file-sheet" aria-labelledby="file-sheet-title"
      onCancel={(event) => { event.preventDefault(); finish(null) }}
      onClick={(event) => { if (event.target === event.currentTarget) finish(null) }}>
      <div className="mobile-file-sheet-body">
        <h2 id="file-sheet-title">Choose photos or files</h2>
        <button className="btn btn-ghost" onClick={() => finish('photos')}><Images size={24} /> Photos <span>Photos and videos</span></button>
        <button className="btn btn-ghost" onClick={() => finish('files')}><File size={24} /> Files <span>Documents and other files</span></button>
        <button className="btn btn-ghost" onClick={() => finish(null)}>Cancel</button>
      </div>
    </dialog>
  )
}
