// GIF search/trending via Giphy. Tenor's API shuts down June 30 2026, so Giphy
// is the durable choice — and Giphy explicitly sanctions shipping the API key in
// a client (their keys are not secrets). The key comes from Settings (a free key
// from developers.giphy.com); this is a thin provider so swapping the source
// later is a one-file change.
const BASE = 'https://api.giphy.com/v1/gifs'

export interface GifResult {
  id: string
  title: string
  /** Tiny animated thumbnail for the picker grid. */
  thumbUrl: string
  /** Size-capped GIF we actually download + send (the `downsized` rendition). */
  sendUrl: string
  /** The giphy.com page (for attribution). */
  pageUrl: string
  w: number
  h: number
}

function normalize(data: any[]): GifResult[] {
  return (data || [])
    .map((g) => {
      const img = g?.images ?? {}
      const thumb = img.fixed_width_small ?? img.preview_gif ?? img.fixed_width
      const send = img.downsized ?? img.fixed_width ?? img.original
      if (!thumb?.url || !send?.url) return null
      return {
        id: String(g.id ?? ''),
        title: String(g.title ?? ''),
        thumbUrl: thumb.url,
        sendUrl: send.url,
        pageUrl: String(g.url ?? ''),
        w: Number(send.width ?? img.original?.width ?? 0),
        h: Number(send.height ?? img.original?.height ?? 0),
      } as GifResult
    })
    .filter((x): x is GifResult => !!x && !!x.id)
}

async function fetchGifs(url: string, signal?: AbortSignal): Promise<GifResult[]> {
  const r = await fetch(url, { signal })
  if (!r.ok) throw new Error(`giphy ${r.status}`)
  const j = await r.json()
  return normalize(j.data)
}

export function trendingGifs(key: string, signal?: AbortSignal): Promise<GifResult[]> {
  const u = `${BASE}/trending?api_key=${encodeURIComponent(key)}&limit=24&rating=pg-13`
  return fetchGifs(u, signal)
}

export function searchGifs(key: string, query: string, signal?: AbortSignal): Promise<GifResult[]> {
  const q = encodeURIComponent(query.trim())
  if (!q) return trendingGifs(key, signal)
  const u = `${BASE}/search?api_key=${encodeURIComponent(key)}&q=${q}&limit=24&rating=pg-13&lang=en`
  return fetchGifs(u, signal)
}
