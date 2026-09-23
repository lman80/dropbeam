// Reporting objectionable content or abusive people (App Store guideline 1.2).
// DropBeam has no server, so a report is an email the user sends to the
// developer from their own mail app. It carries ONLY what the user chose to
// include: the reason, optional notes, and — if they ticked it — the text of
// the reported message. Files are never attached automatically.

/** The one place the developer's report/support address lives (the App Store contact on file). */
export const REPORT_EMAIL = 'imamiller64@gmail.com'

export const REPORT_REASONS = [
  { id: 'spam', label: 'Spam or unwanted messages' },
  { id: 'harassment', label: 'Harassment or bullying' },
  { id: 'hate', label: 'Hate speech or threats' },
  { id: 'sexual', label: 'Sexual or explicit content' },
  { id: 'illegal', label: 'Illegal or dangerous content' },
  { id: 'impersonation', label: 'Impersonation or scam' },
  { id: 'other', label: 'Something else' },
] as const
export type ReportReason = (typeof REPORT_REASONS)[number]['id']

export type ReportInput = {
  reason: ReportReason | string
  /** The reported person's display name (as the reporter sees it). */
  personName: string
  /** Their endpoint id, so the developer can tell who is meant. */
  personId?: string | null
  /** What kind of thing is reported. */
  subject: 'person' | 'message' | 'file'
  /** The reported message's text — only when the user chose to include it. */
  messageText?: string | null
  /** File names of a reported file message (names only, never the files). */
  fileNames?: string[] | null
  /** When the reported message was sent (ms). */
  messageTs?: number | null
  notes?: string | null
  /** Whether the reporter also blocked the person. */
  blocked?: boolean
  appVersion?: string | null
  platform?: string | null
}

const MAX_QUOTE = 2000

export function reasonLabel(reason: string): string {
  return REPORT_REASONS.find((r) => r.id === reason)?.label ?? reason
}

/** The email body — plain text, nothing the user didn't choose to include. */
export function reportBody(r: ReportInput): string {
  const lines = [
    `Reason: ${reasonLabel(r.reason)}`,
    `Reported: ${r.subject === 'person' ? 'a person' : r.subject === 'file' ? 'a file' : 'a message'}`,
    `From: ${r.personName.trim() || 'Unknown'}${r.personId ? ` (DropBeam id ${r.personId})` : ''}`,
  ]
  if (r.messageTs) lines.push(`Sent: ${new Date(r.messageTs).toISOString()}`)
  if (r.fileNames?.length) lines.push(`File name${r.fileNames.length > 1 ? 's' : ''}: ${r.fileNames.slice(0, 20).join(', ')}`)
  const quote = r.messageText?.trim()
  if (quote) {
    const clipped = quote.length > MAX_QUOTE ? `${quote.slice(0, MAX_QUOTE)}…` : quote
    lines.push('', 'Message text:', ...clipped.split(/\r?\n/).map((l) => `> ${l}`))
  }
  if (r.notes?.trim()) lines.push('', 'Details:', r.notes.trim())
  lines.push('', `Blocked: ${r.blocked ? 'yes' : 'no'}`)
  if (r.appVersion || r.platform) lines.push(`App: DropBeam ${r.appVersion ?? ''}${r.platform ? ` (${r.platform})` : ''}`.trim())
  return lines.join('\n')
}

export function reportSubject(r: Pick<ReportInput, 'reason' | 'subject'>): string {
  return `DropBeam report: ${reasonLabel(r.reason)}`
}

/** mailto: URL for a report (RFC 6068: percent-encoded, CRLF line breaks). */
export function reportMailto(r: ReportInput): string {
  return mailto(reportSubject(r), reportBody(r))
}

/** mailto: for Settings → Report a Problem / Contact. */
export function contactMailto(appVersion?: string | null, platform?: string | null): string {
  const app = appVersion || platform ? `\n\n—\nDropBeam ${appVersion ?? ''}${platform ? ` (${platform})` : ''}`.trimEnd() : ''
  return mailto('DropBeam: problem or question', app)
}

function mailto(subject: string, body: string): string {
  const enc = (s: string) => encodeURIComponent(s.replace(/\r?\n/g, '\r\n'))
  return `mailto:${REPORT_EMAIL}?subject=${enc(subject)}${body ? `&body=${enc(body)}` : ''}`
}

/** "macOS" / "Windows" / "Linux" / "iOS" from a user agent (for the report footer). */
export function platformLabel(ua: string): string {
  if (/iPhone|iPad|iPod/.test(ua)) return 'iOS'
  if (/Windows/.test(ua)) return 'Windows'
  if (/Mac OS X|Macintosh/.test(ua)) return 'macOS'
  if (/Linux|X11/.test(ua)) return 'Linux'
  return ''
}

/** The iOS Report sheet's email: resolved from the store's friend + message. */
export function nativeReportMail(r: {
  friend: { name: string; endpointId?: string | null }
  message?: { kind: string; text: string; files: string[]; ts: number; deleted?: boolean; gif?: unknown } | null
  reason: string
  includeText: boolean
  notes: string
  alsoBlock: boolean
  appVersion?: string
}): { url: string; to: string; subject: string; body: string } {
  const m = r.message && !r.message.deleted ? r.message : null
  const isFile = m?.kind === 'file' && !m.gif
  const input: ReportInput = {
    reason: r.reason,
    subject: r.message ? (isFile ? 'file' : 'message') : 'person',
    personName: r.friend.name,
    personId: r.friend.endpointId,
    messageText: r.includeText && m && !isFile && !m.gif ? m.text : null,
    fileNames: r.includeText && isFile ? m.files : null,
    messageTs: r.message?.ts ?? null,
    notes: r.notes,
    blocked: r.alsoBlock,
    appVersion: r.appVersion || null,
    platform: 'iOS',
  }
  return { url: reportMailto(input), to: REPORT_EMAIL, subject: reportSubject(input), body: reportBody(input) }
}
