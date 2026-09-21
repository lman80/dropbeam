// Pure model: safe to import in Node without loading Tauri or the app store.
export const NATIVE_TABS = ['send', 'friends', 'chat', 'history', 'settings'] as const

export function nativeTabBarModel(snapshot: {
  view: string
  transfers: Record<string, { state: string }>
  chatUnread: Record<string, number>
  activeChatId: string | null
}, keyboardVisible = false) {
  const index = snapshot.view === 'locations' ? 1 : NATIVE_TABS.findIndex(tab => tab === snapshot.view)
  return {
    index: index < 0 ? 0 : index,
    sendBadge: Object.values(snapshot.transfers).filter(t =>
      ['starting', 'waitingForPeer', 'connecting', 'waitingForAccept', 'transferring'].includes(t.state),
    ).length,
    chatBadge: Object.values(snapshot.chatUnread).reduce((sum, count) => sum + count, 0),
    hidden: keyboardVisible || (snapshot.view === 'chat' && snapshot.activeChatId !== null),
  }
}
