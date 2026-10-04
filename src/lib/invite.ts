// The invite people paste into a text message — the same words the iPhone
// shares (Invite.swift InviteText), so whoever gets it knows what to do.

/** Turns `…/add/#<code>` into a `dropbeam:` link (repo lman80/dropbeam-invite). */
export const INVITE_PAGE = 'https://lman80.github.io/dropbeam-invite/add/'

export function inviteMessage(name: string, code: string): string {
  const who = name.trim()
  return [
    `Add me on DropBeam${who ? ` — I’m ${who}` : ''}.`,
    `Tap to add me: ${INVITE_PAGE}#${code}`,
    'Or copy this whole message, open DropBeam, choose Add a Friend and paste it.',
    'DropBeam connects our devices directly (no server in between), so keep it open until we’re connected.',
    code,
  ].join('\n\n')
}
