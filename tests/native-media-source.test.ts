import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const root = new URL('../src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/', import.meta.url)
test('native UI never synchronously loads a full-resolution UIImage from a file', () => {
  function check(directory: string) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) check(path)
      else if (entry.name.endsWith('.swift')) assert.doesNotMatch(readFileSync(path, 'utf8'), /UIImage\s*\(\s*contentsOfFile\s*:/, path)
    }
  }
  check(decodeURIComponent(root.pathname))
})
test('document picker uses system directory and retains multiple file imports', () => {
  const swift = readFileSync(new URL('UI/NativeFolderPicker.swift', root), 'utf8')
  assert.doesNotMatch(swift, /\.directoryURL\s*=/)
  assert.match(swift, /asCopy: !folder/)
  assert.match(swift, /allowsMultipleSelection = !folder/)
})
