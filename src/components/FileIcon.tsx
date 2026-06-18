import {
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Folder,
} from 'lucide-react'

type Kind = 'image' | 'video' | 'audio' | 'doc' | 'sheet' | 'archive' | 'code' | 'folder' | 'file'

const EXT: Record<string, Kind> = {}
const add = (kind: Kind, exts: string[]) => exts.forEach((e) => (EXT[e] = kind))
add('image', ['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'heif', 'svg', 'bmp', 'tiff', 'cr2', 'raw', 'dng', 'arw'])
add('video', ['mp4', 'mov', 'mkv', 'avi', 'webm', 'm4v', 'wmv', 'flv'])
add('audio', ['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg', 'aiff'])
add('doc', ['pdf', 'doc', 'docx', 'txt', 'md', 'rtf', 'pages', 'key', 'odt'])
add('sheet', ['csv', 'xls', 'xlsx', 'numbers', 'ods'])
add('archive', ['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'dmg', 'pkg', 'iso'])
add('code', ['js', 'ts', 'tsx', 'jsx', 'rs', 'py', 'go', 'java', 'c', 'cpp', 'h', 'css', 'html', 'json', 'sh', 'rb', 'swift', 'fig'])

/** Best-effort file kind from a name/relative path (uses the extension). */
export function fileKind(name: string): Kind {
  const clean = name.split(/[\\/]/).pop() ?? name
  if (!clean.includes('.')) return 'file'
  const ext = clean.split('.').pop()?.toLowerCase() ?? ''
  return EXT[ext] ?? 'file'
}

const META: Record<Kind, { Icon: typeof File; color: string }> = {
  image: { Icon: FileImage, color: '#34c2a8' },
  video: { Icon: FileVideo, color: '#e0719a' },
  audio: { Icon: FileAudio, color: '#b08cf0' },
  doc: { Icon: FileText, color: '#5b9bf0' },
  sheet: { Icon: FileSpreadsheet, color: '#3fae6e' },
  archive: { Icon: FileArchive, color: '#d99b3f' },
  code: { Icon: FileCode, color: '#7c87f5' },
  folder: { Icon: Folder, color: '#5b9bf0' },
  file: { Icon: File, color: 'var(--text-muted)' },
}

/** A file-type icon tinted by kind — the files-app affordance for a row. */
export function FileIcon({ name, size = 17 }: { name: string; size?: number }) {
  const { Icon, color } = META[fileKind(name)]
  return <Icon size={size} color={color} />
}

export function fileKindColor(name: string): string {
  return META[fileKind(name)].color
}
