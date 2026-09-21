/** JSON-only boundary; shared tests do not need a WebView or Tauri runtime. */
export type BridgeArgs = Record<string, unknown>
export type BridgeHandlers = Record<string, (args: BridgeArgs) => unknown | Promise<unknown>>
export async function dispatchNativeCall(handlers: BridgeHandlers, id: number, name: string, args: BridgeArgs) {
  try {
    if (!Object.hasOwn(handlers, name)) throw new Error(`Unknown native action: ${name}`)
    const result = await handlers[name](args)
    return { id, ok: true, value: JSON.parse(JSON.stringify(result ?? null)) as unknown }
  } catch (error) {
    return { id, ok: false, value: error instanceof Error ? error.message : String(error) }
  }
}
export function changedSnapshots(previous: Map<string, string>, values: Record<string, unknown>) {
  return Object.entries(values).flatMap(([key, value]) => {
    const json = JSON.stringify(value ?? null)
    if (previous.get(key) === json) return []
    previous.set(key, json)
    return [{ key, value: JSON.parse(json) as unknown }]
  })
}

/** Re-push authoritative native state only after Swift receives the reply. */
export async function deliverNativeReply<T>(reply: T, deliver: (reply: T) => Promise<unknown>, resnapshot: () => void) {
  await deliver(reply)
  resnapshot()
}

/** Native UI must bypass api.pickPhotos/pickFiles, which open a web source chooser. */
export async function pickNativeMedia(source: 'photos' | 'files', invoke: (command: string) => Promise<unknown>): Promise<string[]> {
  const result = await invoke(source === 'photos' ? 'pick_photos' : 'plugin:native-ui|pick_files')
  const paths = source === 'photos' ? result : (result as { paths?: unknown } | null)?.paths
  if (!Array.isArray(paths) || !paths.every(path => typeof path === 'string')) throw new Error('The picker returned invalid file paths. Please try again.')
  return paths
}

export function nativeAvatarPath(path: string): string {
  if (!/\.(png|jpe?g|gif|webp|hei[cf]|avif|bmp|tiff?)$/i.test(path)) throw new Error('Choose a photo for your profile picture.')
  return path
}
