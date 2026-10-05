// Shared-folder vocabulary in everyday words, in ONE place so the create
// dialog, folder settings and the incoming-invite prompt say the same thing
// (docs/GLOSSARY.md; the iOS app uses the same strings). Pure — unit-tested.

export type FolderMode = 'mirror' | 'twoway' | 'oneway'

export const FOLDER_MODES: Record<FolderMode, { title: string; desc: string }> = {
  mirror: {
    title: 'Fully synced',
    desc: 'Adding, changing and deleting files happens for everyone. Deleted files can be brought back from Recoverable Files.',
  },
  twoway: {
    title: 'Add and edit only',
    desc: 'Everyone can add and change files. Deleting a file only removes your own copy.',
  },
  oneway: {
    title: 'Only I can change it',
    desc: 'Others get a copy that updates when you change yours.',
  },
}

export const ROLE_WORDS = {
  editor: 'Can add, change and delete files',
  viewer: 'Can open and copy files, but not change them',
} as const

export function folderMode(p: { mirror: boolean; twoWay: boolean }): FolderMode {
  return p.mirror ? 'mirror' : p.twoWay ? 'twoway' : 'oneway'
}

/** What a shared-folder invite (`dropbeam1:<base64url JSON>`) allows, read from
 *  the code itself — null if it can't be read. Only the mode flags are used. */
export function inviteMode(code: string): FolderMode | null {
  const m = /^dropbeam1:([A-Za-z0-9_-]+)$/i.exec(code.trim())
  if (!m) return null
  try {
    const b64 = m[1].replace(/-/g, '+').replace(/_/g, '/')
    const padded = b64 + '='.repeat((4 - (b64.length % 4)) % 4)
    const json = JSON.parse(atob(padded)) as { tw?: unknown; mir?: unknown }
    if (json.mir === true) return 'mirror'
    if (json.tw === true) return 'twoway'
    if (json.tw === false) return 'oneway'
    return null
  } catch {
    return null
  }
}

/** For the person JOINING: what the invite means for them. */
export function inviteModeForJoiner(mode: FolderMode): string {
  switch (mode) {
    case 'mirror': return 'Everyone in it can add, change and delete files — including deleting them for you. Deleted files can be brought back from Recoverable Files.'
    case 'twoway': return 'Everyone can add and change files. If someone deletes a file, your copy stays.'
    case 'oneway': return 'You’ll get a copy that updates when they change theirs.'
  }
}

/** A folder too broad to share whole (home, or a top-level one like Documents). */
export function isBroadFolder(path: string, home: string | null): boolean {
  const norm = (p: string) => p.replace(/[\\/]+$/, '').toLowerCase()
  const p = norm(path)
  if (!p || /^[a-z]:$/.test(p) || p === '' || p === '/') return true
  if (home) {
    const h = norm(home)
    if (p === h) return true
    const rest = p.startsWith(h) ? p.slice(h.length).replace(/^[\\/]/, '') : null
    if (rest && !/[\\/]/.test(rest) && ['desktop', 'documents', 'downloads', 'pictures', 'movies', 'music', 'videos'].includes(rest)) return true
  }
  return false
}
