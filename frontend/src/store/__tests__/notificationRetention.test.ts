import { afterEach, expect, it, vi } from 'vitest'
import { useNotificationStore, bindNotificationOwner, resetNotificationOwner, notificationHref } from '../notifications'

const storageKey = 'dockervault-notifications-v1'
afterEach(() => { resetNotificationOwner(); sessionStorage.clear(); vi.useRealTimers() })
const result = { type: 'success' as const, title: 'UNTRUSTED TITLE', message: 'PRIVATE PATH OR PROVIDER ERROR', code: 'run-completed' as const, target: { kind: 'run' as const, id: 7 }, dedupeKey: 'run:7:completed' }
it('retains only allowlisted outcomes after server owner confirmation and preserves read state', () => {
  bindNotificationOwner(11)
  useNotificationStore.getState().addNotification(result)
  const id = useNotificationStore.getState().notifications[0].id
  useNotificationStore.getState().markRead(id)
  const saved = sessionStorage.getItem(storageKey)!
  expect(saved).not.toContain('PRIVATE')
  expect(saved).not.toContain('UNTRUSTED')
  resetNotificationOwner(11)
  expect(useNotificationStore.getState().notifications).toEqual([])
  bindNotificationOwner(11)
  expect(useNotificationStore.getState().notifications).toMatchObject([{ read: true, target: { kind: 'run', id: 7 } }])
  useNotificationStore.getState().addNotification(result)
  expect(useNotificationStore.getState().notifications).toHaveLength(1)
  bindNotificationOwner(12)
  expect(useNotificationStore.getState().notifications).toEqual([])
  expect(sessionStorage.getItem(storageKey)).toBeNull()
})
it('logout removes retention immediately, while malformed and expired records never load', () => {
  bindNotificationOwner(11)
  useNotificationStore.getState().addNotification(result)
  resetNotificationOwner()
  expect(sessionStorage.getItem(storageKey)).toBeNull()
  sessionStorage.setItem(storageKey, '{invalid')
  bindNotificationOwner(11)
  expect(useNotificationStore.getState().notifications).toEqual([])
  useNotificationStore.getState().addNotification(result)
  resetNotificationOwner(11)
  vi.useFakeTimers(); vi.setSystemTime(Date.now() + 8 * 86400000)
  bindNotificationOwner(11)
  expect(useNotificationStore.getState().notifications).toEqual([])
})
it('uses validated domain links, not arbitrary URLs or unsupported routes', () => {
  expect(notificationHref({ kind: 'plan', id: 2 })).toBe('/protection?plan=2')
  expect(notificationHref({ kind: 'run', id: 7 })).toBe('/history?run=7')
  expect(notificationHref({ kind: 'archive', id: 8 })).toBe('/history?archive=8')
  expect(notificationHref({ kind: 'operation', id: 'b658c36b-4872-4364-aafc-72b227142815', storageId: 'local' })).toBe('/storage?operation=b658c36b-4872-4364-aafc-72b227142815&storage=local')
  expect(notificationHref({ kind: 'run', id: -1 })).toBeNull()
})
it('bounded retention survives quota failures without breaking notification controls', () => {
  bindNotificationOwner(11)
  const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('Quota') })
  for (let id = 1; id <= 60; id++) useNotificationStore.getState().addNotification({ ...result, target: { kind: 'run', id }, dedupeKey: `run:${id}:completed` })
  expect(useNotificationStore.getState().notifications).toHaveLength(50)
  useNotificationStore.getState().markAllRead()
  expect(useNotificationStore.getState().unreadCount()).toBe(0)
  useNotificationStore.getState().clearAll()
  expect(useNotificationStore.getState().notifications).toEqual([])
  spy.mockRestore()
})
