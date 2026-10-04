// Turn whatever an engine call threw into one plain sentence a person can act
// on, keeping the raw text for a "Details" disclosure (support needs it; the
// user mostly doesn't). Messages the engine already writes for people ("That
// folder doesn't exist.") pass through untouched.

export interface HumanError {
  /** One short, plain sentence. */
  message: string
  /** The raw text, when it says more than `message` (null otherwise). */
  details: string | null
}

/** Raw text of anything thrown: Error, string, Tauri error object, … */
export function rawErrorText(e: unknown): string {
  if (e == null) return ''
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message || String(e)
  if (typeof e === 'object') {
    const o = e as { message?: unknown; error?: unknown }
    if (typeof o.message === 'string') return o.message
    if (typeof o.error === 'string') return o.error
    try { return JSON.stringify(e) } catch { /* fall through */ }
  }
  return String(e)
}

// Ordered: the first pattern that matches wins.
const RULES: [RegExp, string][] = [
  [/cancel+ed|aborted by user/i, 'Canceled.'],
  [/no space left|disk (is )?full|os error 28|not enough (free )?space|ENOSPC/i, 'There isn’t enough free space on the disk.'],
  [/file too large|EFBIG|os error 27|FAT32|4 ?GB limit/i, 'That file is too big for the drive it’s going to (FAT32 drives can’t hold files over 4 GB).'],
  [/permission denied|operation not permitted|os error (1|13)\b|EACCES|EPERM|access is denied/i, 'DropBeam doesn’t have permission to use that file or folder.'],
  [/no such file|not ?found.*(file|path|directory)|os error 2\b|ENOENT|cannot find the (file|path)/i, 'That file or folder isn’t there anymore.'],
  [/read-only file system|os error 30|EROFS/i, 'That drive is read-only.'],
  [/(iroh|engine|network|endpoint).{0,20}(not ready|not started|starting)|still starting/i, 'DropBeam is still starting up. Try again in a moment.'],
  [/timed? ?out|timeout|deadline/i, 'The other device took too long to answer. Make sure it’s online and try again.'],
  [/offline|not online|peer (is )?unavailable|no addressing information|could not (reach|connect)|failed to connect|connection (refused|reset|closed|lost|aborted)|unreachable|no route|broken pipe|os error (32|54|60|61|64|65)\b/i, 'Couldn’t reach the other device. Make sure it’s online and try again.'],
  [/dns|resolve host|getaddrinfo|network is down|os error 50\b|no internet/i, 'There’s a network problem. Check your connection and try again.'],
  [/invalid (ticket|code|invite)|ticket.*(invalid|parse|decode)|failed to (parse|decode).*(ticket|code)|unsupported link version|link expired/i, 'That code isn’t valid or has expired. Ask for a new one and try again.'],
]

/** Does this already read like a sentence written for a person? */
function looksHuman(raw: string): boolean {
  if (raw.length > 160) return false
  if (/[{}[\]]|::|\bos error\b|\bErr\(|\bError:|panicked|0x[0-9a-f]{4,}|\b[A-Z][a-z]+Error\b/.test(raw)) return false
  // Starts like a sentence (capital letter) — lower-case starts are usually
  // internal messages ("invalid sender identity").
  return /^[A-Z“"‘']/.test(raw.trim())
}

function sentence(s: string): string {
  const t = s.trim().replace(/^error:\s*/i, '')
  if (!t) return t
  const capped = t[0].toUpperCase() + t.slice(1)
  return /[.!?…”’)]$/.test(capped) ? capped : `${capped}.`
}

/**
 * Map an error to a plain sentence. `fallback` is what to say when the raw
 * text is too technical to show and matches no known pattern.
 */
export function humanError(e: unknown, fallback = 'Something went wrong. Try again.'): HumanError {
  const raw = rawErrorText(e).trim()
  if (!raw) return { message: fallback, details: null }
  const stripped = raw.replace(/^(error|failed|tauri error):\s*/i, '')
  if (looksHuman(stripped)) {
    const message = sentence(stripped)
    return { message, details: null }
  }
  for (const [re, text] of RULES) {
    if (re.test(raw)) return { message: text, details: raw }
  }
  return { message: fallback, details: raw }
}

/** Just the sentence — for inline error text under a field. */
export function errorText(e: unknown, fallback?: string): string {
  return humanError(e, fallback).message
}

/**
 * A sentence with context: "Couldn’t save settings. DropBeam doesn’t have
 * permission…" — the context alone when the cause is unknown/technical.
 */
export function humanErrorIn(context: string, e: unknown): HumanError {
  const h = humanError(e, '')
  const ctx = sentence(context)
  if (!h.message) return { message: ctx, details: h.details ?? (rawErrorText(e) || null) }
  if (h.message === ctx) return h
  return { message: `${ctx} ${h.message}`, details: h.details }
}
