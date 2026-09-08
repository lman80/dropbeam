import { motion } from 'framer-motion'
import { FilePlus2, Upload } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'

export function DropZone({
  hovering,
  onPick,
  onPickPhotos,
  busy,
}: {
  hovering: boolean
  onPick: () => void
  onPickPhotos?: () => void
  busy?: boolean
}) {
  const Panel = MOBILE_UI ? motion.div : motion.button
  return (
    <Panel
      onClick={MOBILE_UI ? undefined : onPick}
      data-testid="dropzone"
      animate={{ scale: hovering ? 1.012 : 1 }}
      transition={{ type: 'spring', stiffness: 400, damping: 25 }}
      style={{
        width: '100%',
        border: `2px dashed ${hovering ? 'var(--accent)' : 'var(--border-strong)'}`,
        background: hovering
          ? 'color-mix(in srgb, var(--accent) 8%, var(--surface))'
          : 'var(--surface)',
        borderRadius: 20,
        // A phone screen is short — a 46px-tall pad pushes the transfer list
        // below the fold.
        padding: MOBILE_UI ? '30px 20px' : '46px 24px',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 14,
        cursor: 'default',
        transition: 'background 0.18s, border-color 0.18s',
        boxShadow: hovering ? 'var(--shadow-lg)' : 'var(--shadow)',
      }}
    >
      <motion.div
        animate={hovering ? { y: -4 } : { y: 0 }}
        transition={{ type: 'spring', stiffness: 300, damping: 18 }}
        className={hovering ? 'animate-beam' : ''}
        style={{
          width: 68,
          height: 68,
          borderRadius: 20,
          display: 'grid',
          placeItems: 'center',
          color: 'white',
          background: 'linear-gradient(135deg, var(--accent), var(--accent-2))',
          boxShadow: '0 10px 28px color-mix(in srgb, var(--accent) 40%, transparent)',
        }}
      >
        {hovering && !MOBILE_UI ? <Upload size={30} /> : <FilePlus2 size={28} />}
      </motion.div>
      <div style={{ textAlign: 'center' }}>
        {/* Phone users choose between the photo library and Files. */}
        <div style={{ fontSize: 17, fontWeight: 700, color: 'var(--text)' }}>
          {MOBILE_UI ? 'Choose files to send' : hovering ? 'Drop to send' : 'Drag files here to send'}
        </div>
        <div style={{ fontSize: 13.5, color: 'var(--text-muted)', marginTop: 4 }}>
          {MOBILE_UI ? (
            'Pick photos, videos or documents, then choose who to send them to.'
          ) : (
            <>
              or <span style={{ color: 'var(--accent)', fontWeight: 600 }}>click to choose</span>{' '}
              files &amp; folders
            </>
          )}
        </div>
      </div>
      {MOBILE_UI && (
        <div style={{ display: 'flex', gap: 12 }}>
          <button className="btn btn-primary" disabled={busy} onClick={onPickPhotos} aria-label="Photos & Videos">Photos</button>
          <button className="btn btn-ghost" disabled={busy} onClick={onPick}>Files</button>
        </div>
      )}
    </Panel>
  )
}
