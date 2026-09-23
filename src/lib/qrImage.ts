import jsQR from 'jsqr'

/** Read a QR code out of an image (a screenshot, a photo, a pasted clipboard
 *  image) — the camera-less way to "scan". Tries full resolution first (a small
 *  QR inside a big screenshot needs every pixel) then a downscale (huge phone
 *  photos are slow and noisy at full size), each with normal + inverted
 *  (dark-mode) colors. Returns null when no QR is found. */
export async function decodeQrFromImage(blob: Blob): Promise<string | null> {
  const url = URL.createObjectURL(blob)
  try {
    const img = new Image()
    img.decoding = 'async'
    img.src = url
    await img.decode()
    const w = img.naturalWidth, h = img.naturalHeight
    if (!w || !h) return null
    const canvas = document.createElement('canvas')
    const ctx = canvas.getContext('2d', { willReadFrequently: true })
    if (!ctx) return null
    const longest = Math.max(w, h)
    const targets = [...new Set([2400, 1000, 600].map((t) => Math.min(longest, t)))]
    for (const target of targets) {
      const scale = target / longest
      canvas.width = Math.max(1, Math.round(w * scale)); canvas.height = Math.max(1, Math.round(h * scale))
      // White under transparent PNG pixels, or a transparent QR reads as all-black.
      ctx.fillStyle = '#fff'; ctx.fillRect(0, 0, canvas.width, canvas.height)
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height)
      const data = ctx.getImageData(0, 0, canvas.width, canvas.height)
      const qr = jsQR(data.data, canvas.width, canvas.height, { inversionAttempts: 'attemptBoth' })
      if (qr?.data) return qr.data
    }
    return null
  } finally {
    URL.revokeObjectURL(url)
  }
}

/** First image in a paste/drop payload, if any. */
export function imageFromTransfer(dt: DataTransfer | null): File | null {
  if (!dt) return null
  const files = Array.from(dt.files ?? [])
  const file = files.find((f) => f.type.startsWith('image/'))
  if (file) return file
  for (const item of Array.from(dt.items ?? [])) {
    if (item.kind === 'file' && item.type.startsWith('image/')) return item.getAsFile()
  }
  return null
}

// The desktop shell swallows OS file drops (they arrive as paths via Tauri, not
// as DOM drop events), so while a scanner is open App hands dropped paths to it
// instead of starting a send.
let scannerDrop: ((paths: string[]) => void) | null = null
/** Register the open scanner's drop handler; returns the unregister function. */
export function setScannerDrop(handler: (paths: string[]) => void): () => void {
  scannerDrop = handler
  return () => { if (scannerDrop === handler) scannerDrop = null }
}
/** true = an open scanner took the drop (App must not treat it as a send). */
export function routeDropToScanner(paths: string[]): boolean {
  if (!scannerDrop) return false
  scannerDrop(paths)
  return true
}
