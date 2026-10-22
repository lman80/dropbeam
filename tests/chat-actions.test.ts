import { strict as assert } from 'node:assert'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

// #84: on a long message the hover bar (Copy, Reply, React) sat beside the
// middle of the bubble — often off screen. It must sit at the bubble's bottom
// edge and ride up to the bottom of the visible part when the bubble is taller
// than the thread. (Checked in a real browser too; this guards the rules.)
const css = readFileSync(new URL('../src/styles/chat.css', import.meta.url), 'utf8')
const tsx = readFileSync(new URL('../src/views/ChatView.tsx', import.meta.url), 'utf8')
const rule = (sel: string) => css.match(new RegExp(`(^|\\n)${sel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')} \\{([^}]*)\\}`))?.[2] ?? ''

test('the chat hover bar sits at the bottom of the bubble and sticks to the visible bottom (#84)', () => {
  const anchor = rule('.chat-actions-anchor')
  assert.match(anchor, /position:\s*sticky/)
  assert.match(anchor, /bottom:\s*\d+px/)
  assert.match(anchor, /height:\s*0/)
  const bar = rule('.chat-actions')
  assert.match(bar, /position:\s*absolute/)
  assert.match(bar, /bottom:\s*0/)
  assert.doesNotMatch(bar, /top:\s*50%/, 'not the vertical middle any more')
  // Shown on hover anywhere on the bubble, and when a button has keyboard focus.
  assert.match(css, /\.chat-main:hover > \.chat-actions-anchor > \.chat-actions,\s*\n\.chat-actions:focus-within/)
  // The anchor is the bar's parent and the bubble's last child.
  assert.match(tsx, /<div className="chat-actions-anchor">\s*\n\s*<div className=\{`chat-actions/)
})
