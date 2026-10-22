import { Ellipsis } from 'lucide-react'
import { FileIcon, fileKindColor } from '../../components/FileIcon'
import { ContextMenu, type MenuAction } from '../kit'
import '../files.css'

export function FileTile({ name }: { name: string }) {
  return <span className="files-tile" style={{ background: `color-mix(in srgb, ${fileKindColor(name)} 13%, transparent)` }}><FileIcon name={name} size={21} /></span>
}

/** The kit reserves touch taps for row activation. Explicit menu buttons also open on tap. */
export function FilesMenu({ actions, label = 'More actions' }: { actions: MenuAction[]; label?: string }) {
  return <ContextMenu label={label} actions={actions}><span className="files-menu-glyph" onClick={event => {
    if (event.detail > 0 && !window.matchMedia('(pointer: fine)').matches && (event.nativeEvent as PointerEvent).pointerType !== 'mouse') {
      event.stopPropagation()
      event.currentTarget.parentElement?.click()
    }
  }}><Ellipsis size={24} /></span></ContextMenu>
}
