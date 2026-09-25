import { AnimatePresence, motion } from 'framer-motion'
import { FilePlus2, FileUp, Upload } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'

export function DropZone({
  hovering,
  picking = false,
  onPick,
  onPickPhotos,
  compact = false,
}: {
  hovering: boolean
  picking?: boolean
  onPick: () => void
  onPickPhotos?: () => void
  /** Transfers are listed below: no big target, just the drag overlay. */
  compact?: boolean
}) {
  if (MOBILE_UI) return (
    <section className="mobile-send-hero" data-testid="dropzone" aria-busy={picking}>
      <div className="mobile-file-icon"><FilePlus2 size={32} /></div>
      <h2 className="ios-title2">Choose files to send</h2>
      <p className="ios-sub">Pick photos, videos or documents, then choose who to send them to.</p>
      <div className="mobile-pickers" role="group" aria-label="Choose files to send">
        <button disabled={picking} onClick={onPickPhotos}>Photos</button>
        <button disabled={picking} onClick={onPick}>Files</button>
      </div>
    </section>
  )
  return (
    <>
      {!compact && (
        <button
          onClick={onPick}
          disabled={picking}
          aria-busy={picking}
          data-testid="dropzone"
          className={`dropzone${hovering ? ' hovering' : ''}`}
          aria-label="Choose files to send"
        >
          <FileUp className="dropzone-glyph" strokeWidth={1.5} />
          <span className="dropzone-title">Drop files here to send</span>
          <span className="dropzone-hint">
            Send to a friend by name, or to anyone with a code. Files sent to you appear here.
          </span>
        </button>
      )}
      {/* While a drag is over the window: one calm target over the content. */}
      <AnimatePresence>
        {hovering && (
          <motion.div
            className="drop-overlay"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.12 }}
            aria-hidden
          >
            <div className="drop-overlay-label">
              <Upload size={18} /> Drop to Send
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  )
}
