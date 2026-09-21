import { createRoot } from 'react-dom/client'
import { ActionSheet } from '../mobile/kit'

type Source = 'photos' | 'files'
let picking = false
let requestedSource: Source | null = null

/** Send's explicit source rows bypass only the chooser, using the same backend pickers. */
export async function withMobileFileSource(source: Source, pick: () => Promise<string[]>): Promise<string[]> {
  if (picking) return []
  requestedSource = source
  try { return await pick() } finally { requestedSource = null }
}

/** One modal for every file-picking entry point; native pickers keep ownership
 * of permissions and return durable local paths through the existing commands. */
export async function pickMobileFiles(pickers: Record<Source, () => Promise<string[]>>): Promise<string[]> {
  if (picking) return []
  picking = true
  const previous = document.activeElement
  if (previous instanceof HTMLElement) previous.blur()
  try {
    const preset = requestedSource
    requestedSource = null
    if (preset) return await pickers[preset]()
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

export function MobileFileSheet({ finish }: { finish: (source: Source | null) => void }) {
  return <ActionSheet dismissOnAction={false} title="Choose Files" onClose={() => finish(null)} actions={[
    { label: 'Photos and Videos', onPress: () => finish('photos') },
    { label: 'Files', onPress: () => finish('files') },
  ]} />
}
