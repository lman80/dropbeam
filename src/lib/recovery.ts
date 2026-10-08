// Recovery code (docs/RECOVERY-CODE.md): pure helpers shared by the desktop
// screens and the iPhone bridge. The words themselves are checked in Rust.

export type RecoveryOldDevice = {
  endpointId: string
  kind?: string | null
  os?: string | null
  model?: string | null
  /** The friend who still knew this device. */
  via: string
}
export type RecoveryFolderNote = { name: string; with: string }
export type RecoveryRestoreView = {
  restoredAt: number
  friendsSynced: number
  returned: string[]
  oldDevices: RecoveryOldDevice[]
  folders: RecoveryFolderNote[]
}
export type RecoveryStatus = {
  /** The code of the account this device uses was written down and checked. */
  saved: boolean
  hasAccount: boolean
  laterAt: number
  restore?: RecoveryRestoreView | null
}
export type RecoveryCheck = { count: number; unknown: number[]; complete: boolean; valid: boolean; problem?: string | null }
export type RecoveryReveal = { words: string[]; qr: string }

/** Plain warnings, shown wherever the words are. */
export const RECOVERY_WARNING = 'Anyone with these words can become you. Keep them somewhere safe, like with your important papers.'
export const RECOVERY_WHY = 'If you ever lose all your phones and computers, these words bring back your account on a new one. Your friends will recognize you again and send back your chats.'
export const RECOVERY_NEVER = 'DropBeam will never ask for them except when you set up a new device yourself. Don’t send them to anyone, not even to us.'

/** "3rd", "12th" — for "Which word is 3rd?". */
export function ordinal(n: number): string {
  const s = n % 100 >= 11 && n % 100 <= 13 ? 'th' : ({ 1: 'st', 2: 'nd', 3: 'rd' } as Record<number, string>)[n % 10] ?? 'th'
  return `${n}${s}`
}

export type QuizQuestion = { index: number; options: string[] }

/**
 * Two "which word is number N?" questions with four choices each (the right
 * one plus three other words from the code, so every choice looks familiar
 * and only the paper tells them apart). `rand` returns [0, 1).
 */
export function makeQuiz(words: string[], rand: () => number = Math.random, count = 2): QuizQuestion[] {
  const n = words.length
  if (n < 4) return []
  const pick = (exclude: Set<number>) => {
    for (;;) { const i = Math.floor(rand() * n); if (!exclude.has(i)) return i }
  }
  const asked = new Set<number>()
  const out: QuizQuestion[] = []
  for (let q = 0; q < Math.min(count, n); q++) {
    const index = pick(asked)
    asked.add(index)
    const choices = new Set<string>([words[index]])
    const used = new Set<number>([index])
    while (choices.size < 4 && used.size < n) { const i = pick(used); used.add(i); choices.add(words[i]) }
    const options = [...choices]
    for (let i = options.length - 1; i > 0; i--) { const j = Math.floor(rand() * (i + 1)); [options[i], options[j]] = [options[j], options[i]] }
    out.push({ index, options })
  }
  return out
}

/** Spread pasted text over the word boxes: "1. apple 2. bread …" → words. */
export function splitWords(text: string): string[] {
  const body = text.trim().toLowerCase().replace(/^dropbeamrecover1:/, '')
  return body.split(/[^a-z]+/).filter(Boolean)
}

/** A scanned QR that holds a recovery code. */
export function isRecoveryQr(text: string): boolean {
  return /^dropbeamrecover1:/i.test(text.trim())
}

/** Whether to remind the person to save their code (not every launch). */
export function shouldRemind(status: RecoveryStatus | null | undefined, now: number): boolean {
  if (!status || status.saved) return false
  return now - (status.laterAt || 0) > 14 * 24 * 3600 * 1000
}

/** "Your Mac", "Your iPhone 12" for a device from before the restore. */
export function oldDeviceName(d: Pick<RecoveryOldDevice, 'kind' | 'os' | 'model'>): string {
  if (d.model) return `Your ${d.model}`
  if (d.os === 'ios') return d.kind === 'tablet' ? 'Your iPad' : 'Your iPhone'
  if (d.os === 'macos') return 'Your Mac'
  if (d.os === 'windows') return 'Your PC'
  if (d.os === 'linux') return 'Your Linux computer'
  return d.kind === 'phone' ? 'Your phone' : 'Your computer'
}

/** One line on how the restore is going, for the Devices page. */
export function restoreSummary(r: RecoveryRestoreView): string {
  const n = Math.max(r.friendsSynced, r.returned.length)
  if (n === 0) return 'Your friends will find you as their DropBeam opens — this can take a few hours. Nothing to do here.'
  const who = r.returned.length ? ` (${listNames(r.returned)})` : ''
  return `${n === 1 ? '1 friend has' : `${n} friends have`} found you again${who}. Others will as their DropBeam opens.`
}

export function listNames(names: string[], max = 3): string {
  if (names.length <= max) return names.length === 2 ? names.join(' and ') : names.join(', ').replace(/, ([^,]*)$/, ', and $1')
  return `${names.slice(0, max).join(', ')} and ${names.length - max} more`
}

/** The printable sheet, as plain text lines (the desktop adds the QR). */
export function sheetLines(words: string[]): string[] {
  const half = Math.ceil(words.length / 2)
  return Array.from({ length: half }, (_, i) => {
    const a = `${String(i + 1).padStart(2, ' ')}. ${words[i]}`
    const b = words[i + half] ? `${String(i + half + 1).padStart(2, ' ')}. ${words[i + half]}` : ''
    return b ? `${a.padEnd(20, ' ')}${b}` : a
  })
}
