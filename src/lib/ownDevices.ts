import { useMemo } from 'react'
import { useStore } from '../store'
import { ownDeviceLabels } from './deviceIcons'

/** friend id → "Your iPhone" / "Your Mac" for the user's own linked devices. */
export function useOwnDeviceLabels(): Record<string, string> {
  const friends = useStore(s => s.friends)
  const account = useStore(s => s.myDevice?.account_pub)
  return useMemo(() => account ? ownDeviceLabels(friends.filter(f => f.accountPub === account)) : {}, [friends, account])
}
