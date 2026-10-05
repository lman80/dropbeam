// What each message state means, in one plain sentence — shown when someone
// clicks/taps the little status under their message ("Delivered", "Waiting
// to send"…). Shared by the desktop chat and the iPhone (via the bridge words).

export type MessageState = 'sending' | 'waiting' | 'notAccepted' | 'held' | 'delivered' | 'read' | 'serverNote'

export function statusExplanation(state: MessageState, friend: string, server?: string | null): string {
  switch (state) {
    case 'sending': return `Sending now. It usually takes a second or two.`
    case 'waiting': return `${friend} isn’t online right now. Your message is saved and goes out by itself as soon as you’re both online with DropBeam open.`
    case 'notAccepted': return `${friend} hasn’t accepted your friend request yet. Your message is saved and arrives as soon as they do.`
    case 'held': return `${friend} isn’t online, so ${server || 'your Transfer Server'} is keeping your message safe. ${friend} gets it the moment they’re back.`
    case 'delivered': return `It’s on ${friend}’s device. You’ll see “Read” once they open the chat (unless they’ve turned that off).`
    case 'read': return `${friend} has opened the chat and seen your message.`
    case 'serverNote': return `Your message is saved on this device and DropBeam keeps trying. You don’t need to do anything.`
  }
}
