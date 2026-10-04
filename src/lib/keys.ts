import type { KeyboardEvent as ReactKeyboardEvent } from 'react'
import { IS_MAC } from './platform'

/**
 * A plain Enter press — NOT the Enter that confirms a CJK/IME composition.
 * WebKit reports that confirming keydown with isComposing=false but keyCode 229,
 * so both are checked; otherwise typing Chinese/Japanese submits half a word.
 */
export function isEnterKey(e: ReactKeyboardEvent | KeyboardEvent): boolean {
  const n = 'nativeEvent' in e ? e.nativeEvent : e
  return e.key === 'Enter' && !n.isComposing && n.keyCode !== 229
}

/** ⌘ on macOS, Ctrl elsewhere. */
export function isPrimaryMod(e: ReactKeyboardEvent | KeyboardEvent): boolean {
  return IS_MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey
}

/** "⌘," / "Ctrl+," — how a shortcut is written on this OS. */
export function shortcutLabel(key: string, opts: { shift?: boolean } = {}): string {
  if (IS_MAC) return `${opts.shift ? '⇧' : ''}⌘${key.toUpperCase()}`
  return `Ctrl+${opts.shift ? 'Shift+' : ''}${key.toUpperCase()}`
}
