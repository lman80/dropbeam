// Small helpers for friend/peer avatars: monogram initials + a stable gradient
// derived from an id, so the same person always gets the same colors.

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean)
  if (!parts.length) return '○'
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

const AVATAR_GRADIENTS = [
  'linear-gradient(135deg, #6366f1, #a855f7)',
  'linear-gradient(135deg, #0ea5e9, #22d3ee)',
  'linear-gradient(135deg, #f97316, #f43f5e)',
  'linear-gradient(135deg, #10b981, #14b8a6)',
  'linear-gradient(135deg, #ec4899, #8b5cf6)',
  'linear-gradient(135deg, #f59e0b, #ef4444)',
]

export function avatarGradient(id: string): string {
  let h = 0
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0
  return AVATAR_GRADIENTS[h % AVATAR_GRADIENTS.length]
}
