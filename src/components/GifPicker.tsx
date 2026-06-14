import { useEffect, useRef, useState } from 'react'
import { Search, X } from 'lucide-react'
import { type GifResult, searchGifs, trendingGifs } from '../lib/gif'

/** A composer popover: search box + a grid of trending/searched GIFs. Click one
 *  to send it. Lightweight — debounced search, lazy-loaded animated thumbnails,
 *  and the Giphy attribution their terms require. */
export function GifPicker({
  apiKey,
  onPick,
  onClose,
  onSetup,
}: {
  apiKey: string
  onPick: (g: GifResult) => void
  onClose: () => void
  onSetup: () => void
}) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<GifResult[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const hasKey = !!apiKey.trim()

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  // Debounced search; trending when the box is empty. Cancels in-flight fetches.
  useEffect(() => {
    if (!hasKey) {
      setLoading(false)
      return
    }
    const ctrl = new AbortController()
    setLoading(true)
    setError(false)
    const run = query.trim()
      ? searchGifs(apiKey, query, ctrl.signal)
      : trendingGifs(apiKey, ctrl.signal)
    const t = setTimeout(
      () => {
        run
          .then((r) => {
            setResults(r)
            setLoading(false)
          })
          .catch((e) => {
            if (e?.name === 'AbortError') return
            setError(true)
            setLoading(false)
          })
      },
      query.trim() ? 280 : 0,
    )
    return () => {
      clearTimeout(t)
      ctrl.abort()
    }
  }, [query, apiKey, hasKey])

  return (
    <div className="gif-picker" onMouseDown={(e) => e.stopPropagation()}>
      <div className="gif-picker-head">
        <Search size={14} className="gif-picker-search-ic" />
        <input
          ref={inputRef}
          className="gif-picker-input"
          value={query}
          placeholder="Search GIFs"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') onClose()
          }}
        />
        <button className="gif-picker-close" onClick={onClose} title="Close">
          <X size={15} />
        </button>
      </div>
      <div className="gif-picker-grid">
        {!hasKey && (
          <div className="gif-picker-note">
            <div style={{ marginBottom: 8 }}>GIFs need a free Giphy key.</div>
            <button className="btn btn-primary" onClick={onSetup}>
              Set it up
            </button>
            <div style={{ marginTop: 8, fontSize: 11, opacity: 0.7 }}>
              Grab one at developers.giphy.com → paste it in Settings.
            </div>
          </div>
        )}
        {hasKey && loading && <div className="gif-picker-note">Loading…</div>}
        {hasKey && error && (
          <div className="gif-picker-note">
            Couldn’t load GIFs — check your Giphy key in Settings.
          </div>
        )}
        {hasKey && !loading && !error && results.length === 0 && (
          <div className="gif-picker-note">No GIFs found.</div>
        )}
        {hasKey &&
          !loading &&
          !error &&
          results.map((g) => (
            <button
              key={g.id}
              className="gif-tile"
              onClick={() => onPick(g)}
              title={g.title || 'Send GIF'}
            >
              <img src={g.thumbUrl} alt={g.title} loading="lazy" />
            </button>
          ))}
      </div>
      <div className="gif-picker-foot">Powered by GIPHY</div>
    </div>
  )
}
