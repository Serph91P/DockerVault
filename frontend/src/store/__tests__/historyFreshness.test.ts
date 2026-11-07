import { expect, it } from 'vitest'
import { queryClient, clearSession } from '../session'
import { startQueryFreshness } from '../freshness'
import { queryKeys } from '../../api/queryKeys'
import { useNotificationStore } from '../notifications'
it('records authoritative runs from the history envelope and clears them on logout', () => {
 const stop = startQueryFreshness()
 queryClient.setQueryData(queryKeys.backups.history({view:'runs'}), {items:[{id:904,target_id:1,status:'completed',created_at:'2026-01-01T00:00:00Z'}],total:1})
 expect(useNotificationStore.getState().notifications.some(n=>n.target?.kind==='run'&&n.target.id===904)).toBe(true)
 clearSession()
 expect(useNotificationStore.getState().notifications).toHaveLength(0)
 stop()
})
