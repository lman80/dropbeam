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
