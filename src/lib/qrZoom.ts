/**
 * Zoom for the QR scanner (#85). A laptop camera can't get close to a phone
 * screen, so the code is small in the frame — a few pixels per square, too few
 * to read. Like Alipay, we zoom in for the reader: each frame is read whole AND
 * as a centre crop at full camera resolution (×1.5 / ×2 / ×3 in turn, enlarged
 * before reading), so a small code far away still reads without the person
 * doing anything. The slider zooms the picture (and, where the camera can,
 * zooms optically), and the reader then looks at just that part.
 */

/** Centre crops tried in turn when the person hasn't chosen a zoom. */
export const AUTO_ZOOMS = [1.5, 2, 3] as const
/** Digital zoom the slider offers when the camera can't zoom itself. */
export const MAX_DIGITAL_ZOOM = 4

export interface Crop { sx: number; sy: number; sw: number; sh: number; dw: number; dh: number }

/** The centre crop of a w×h frame at `zoom` (1 = the whole frame), and the size
 *  to draw it at for reading: at most `maxSide` wide, enlarged up to ×2 so each
 *  square of a small code spans enough pixels. */
export function zoomCrop(w: number, h: number, zoom: number, maxSide = 960): Crop {
  const z = Number.isFinite(zoom) ? Math.max(1, zoom) : 1
  const sw = Math.max(1, Math.round(w / z)), sh = Math.max(1, Math.round(h / z))
  const scale = Math.min(maxSide / sw, 2)
  return { sx: Math.round((w - sw) / 2), sy: Math.round((h - sh) / 2), sw, sh, dw: Math.max(1, Math.round(sw * scale)), dh: Math.max(1, Math.round(sh * scale)) }
}

/** Which zooms to read on frame number `tick`: the whole frame plus one crop
 *  in turn (two reads a frame keeps a laptop cool), or — once the person picked
 *  a zoom — that view first, then a closer one in turn. */
export function zoomsForTick(tick: number, chosen = 1): number[] {
  if (chosen > 1.05) {
    const closer = AUTO_ZOOMS.map(z => z * chosen).filter(z => z <= MAX_DIGITAL_ZOOM * 2)
    return closer.length && tick % 2 ? [chosen, closer[Math.floor(tick / 2) % closer.length]] : [chosen]
  }
  return [1, AUTO_ZOOMS[tick % AUTO_ZOOMS.length]]
}

/** The camera's own zoom range, when it has one (some webcams, phones). */
export function hardwareZoom(track: MediaStreamTrack | undefined): { min: number; max: number; step: number } | null {
  // `zoom` isn't in the DOM typings (Chromium only; WebKit has no getCapabilities).
  const t = track as unknown as { getCapabilities?: () => { zoom?: { min?: number; max?: number; step?: number } } } | undefined
  const caps = t?.getCapabilities?.()
  const z = caps?.zoom
  if (!z || typeof z.max !== 'number' || typeof z.min !== 'number' || z.max <= z.min) return null
  return { min: z.min, max: z.max, step: z.step && z.step > 0 ? z.step : 0.1 }
}
