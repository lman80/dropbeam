import { Channel, invoke } from '@tauri-apps/api/core'
import { api, HAS_TAURI } from './api'
import { IS_IOS } from './platform'

type ChatAction = {
  actionId?: string
  notification?: { extra?: { chatPeerId?: unknown } }
}

/** Ignore dismissals and notifications unrelated to a conversation. */
export function notificationChatPeer(action: ChatAction): string | null {
  const peer = action.notification?.extra?.chatPeerId
  return action.actionId === 'tap' && typeof peer === 'string' && peer.trim() ? peer : null
}

let listening: Promise<void> | undefined
export function listenForChatNotifications(openChat: (peerId: string) => Promise<void>) {
  if (!IS_IOS || !HAS_TAURI) return
  // One channel for the app lifetime, including React's development remounts.
  listening ??= (async () => {
    const handler = new Channel<ChatAction>()
    handler.onmessage = (action) => {
      const peer = notificationChatPeer(action)
      if (peer) void openChat(peer)
    }
    await invoke('plugin:notification|watch_actions', { handler })
    void api.frontendLog('Chat notification tap listener ready')
  })().catch((error) => {
    listening = undefined
    void api.frontendLog(`Could not listen for chat notification taps: ${String(error)}`)
  })
  return listening
}
