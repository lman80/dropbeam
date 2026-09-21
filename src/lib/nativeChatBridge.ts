import type { ChatMessage, TransferUpdate } from './api'
import type { BridgeArgs } from './nativeBridgeProtocol'

/** Resolve replies against authoritative history; native clients send only an ID. */
export function nativeReply(messages: ChatMessage[], replyTo: unknown): ChatMessage | undefined {
  if (replyTo == null) return undefined
  if (typeof replyTo !== 'string') throw new Error('Invalid replyTo')
  const message = messages.find(m => m.id === replyTo && !m.deleted)
  if (!message) throw new Error('The original message is no longer available.')
  return message
}

export function nativeChatSource(args: BridgeArgs): 'photos' | 'files' {
  if (args.source !== 'photos' && args.source !== 'files') throw new Error('Invalid source')
  return args.source
}

/** Chat batch IDs can differ from engine transfer IDs. Preserve both namespaces
 * without putting chat-only batches into the native Send tab. */
export function nativeTransfers(regular: TransferUpdate[], chat: Record<string, TransferUpdate>) {
  return [
    ...regular.map(t => ({ ...t, chatOnly: false })),
    ...Object.entries(chat).map(([id, t]) => ({ ...t, id, chatOnly: true })),
  ]
}

export function nativeThread(activeChatId: string | null, chats: Record<string, ChatMessage[]>) {
  return activeChatId ? { friendId: activeChatId, messages: chats[activeChatId] ?? [] } : null
}
