// Small helpers for friend/peer avatars: monogram initials + a stable gradient
// derived from an id, so the same person always gets the same colors.

export function initials(name: string): string {
  if (typeof name !== 'string') return '○'
  const parts = name.trim().split(/\s+/).filter(Boolean)
  if (!parts.length) return '○'
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

// Flat, slightly muted system tints — one per person, stable across launches.
const AVATAR_COLORS = [
  '#6e6ee8', // indigo
  '#3a9ad9', // blue
  '#e0764a', // orange
  '#3aa57a', // green
  '#c9609a', // pink
  '#9a6fd6', // purple
  '#d69a2e', // amber
  '#5d8a9e', // slate
]

/** A stable flat background colour for a person's monogram (name kept for the
 *  existing call sites — it no longer returns a gradient). */
export function avatarGradient(id: string): string {
  let h = 0
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0
  return AVATAR_COLORS[h % AVATAR_COLORS.length]
}
export const avatarColor = avatarGradient
