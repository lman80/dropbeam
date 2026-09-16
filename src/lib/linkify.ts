/** Splitting message text into plain text + clickable links (GitHub #17).
 *
 *  Deliberately paranoid: a chat message is attacker-controlled text, so the ONLY
 *  thing that ever becomes an anchor is an absolute http:// or https:// URL. We
 *  never match bare "www." (upgrading it to a scheme guesses the user's intent and
 *  is a classic phishing vector) and we never match javascript:, data:, file:,
 *  mailto: or any other scheme — they simply aren't in the pattern, so there is no
 *  blocklist to bypass. Output is rendered as React children, never as HTML, so
 *  escaping is the renderer's job and dangerouslySetInnerHTML is never involved.
 */

export type LinkSegment = { t: 'text'; v: string } | { t: 'link'; v: string }

/** Anything longer is almost certainly not a link a human means to click, and a
 *  megabyte-long "URL" in an anchor is just a way to wreck the bubble layout. */
export const MAX_URL_LEN = 2048

/** \b keeps "xhttps://…" from matching; [^\s<>"] stops at whitespace and at the
 *  characters that would let a URL swallow surrounding markup-looking text. */
const URL_RE = /\bhttps?:\/\/[^\s<>"]+/gi
/** Scheme + a non-empty authority. Rejects "https://" and "http:///path". */
const HAS_HOST = /^https?:\/\/[^\s/?#]+/i

const PUNCT = '.,;:!?\'"'
const CLOSERS = ')]}'
const OPENERS = '([{'

const countOf = (s: string, ch: string) => {
  let n = 0
  for (const c of s) if (c === ch) n += 1
  return n
}

/** Peel sentence punctuation and UNBALANCED closing brackets off the end of a
 *  match: "see https://x.dev/a." and "(https://x.dev/a)" both end in characters
 *  that belong to the sentence, while a balanced pair — Wikipedia's
 *  ".../Foo_(bar)" — is part of the URL and must be kept. */
export function trimTrailing(url: string): { url: string; trailing: string } {
  let end = url.length
  while (end > 0) {
    const ch = url[end - 1]
    if (PUNCT.includes(ch)) {
      end -= 1
      continue
    }
    const close = CLOSERS.indexOf(ch)
    if (close >= 0) {
      const body = url.slice(0, end)
      if (countOf(body, ch) > countOf(body, OPENERS[close])) {
        end -= 1
        continue
      }
    }
    break
  }
  return { url: url.slice(0, end), trailing: url.slice(end) }
}

/** Split `text` into ordered segments. Adjacent plain text is merged, so the
 *  segments always alternate and re-joining every `v` reproduces the input
 *  exactly — nothing is dropped, duplicated or rewritten. */
export function linkify(text: string): LinkSegment[] {
  const out: LinkSegment[] = []
  if (typeof text !== 'string' || !text) return out
  let pending = ''
  const flush = () => {
    if (pending) out.push({ t: 'text', v: pending })
    pending = ''
  }
  let last = 0
  for (const match of text.matchAll(URL_RE)) {
    const start = match.index ?? 0
    pending += text.slice(last, start)
    last = start + match[0].length
    const { url, trailing } = trimTrailing(match[0])
    // A match that survives trimming but has no host (or is absurdly long) stays
    // plain text — better an unclickable URL than a bogus one.
    if (!url || url.length > MAX_URL_LEN || !HAS_HOST.test(url)) {
      pending += match[0]
      continue
    }
    flush()
    out.push({ t: 'link', v: url })
    pending = trailing
  }
  pending += text.slice(last)
  flush()
  return out
}
