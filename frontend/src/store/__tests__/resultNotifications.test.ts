import { afterEach, expect, it } from 'vitest'
import type { Backup } from '../../api'
import type { StorageOperation } from '../../api/operations'
import { bindNotificationOwner, resetNotificationOwner, useNotificationStore } from '../notifications'
import { recordRunResults, recordStorageResults } from '../resultNotifications'

const storageKey = 'dockervault-notifications-v1'
const epoch = Date.now()
const time = (id: number) => new Date(epoch - (1000 - id) * 1000).toISOString()
const runs = (size: number): Backup[] => Array.from({ length: size }, (_, index) => {
  const id = size - index
  return { id, target_id: 1, status: 'completed', backup_type: 'full', size_bytes: 1, created_at: time(id), completed_at: time(id), remote_sync_details: [] }
})
const jobs = (size: number): StorageOperation[] => Array.from({ length: size }, (_, index) => {
  const id = size - index
  return { id: `00000000-0000-4000-8000-${String(id).padStart(12, '0')}`, action: 'storage_delete', storage_id: 'local', state: 'completed', stage: 'completed', path: '', total: 1, completed: 1, succeeded: 1, failed: 0, skipped: 0, unknown: 0, cancel_requested: false, created_at: time(id), updated_at: time(id), finished_at: time(id), parent_id: null, items: [{ path: 'synthetic', state: 'succeeded', error: null }] }
})
const state = () => useNotificationStore.getState()
const reload = () => { resetNotificationOwner(11); bindNotificationOwner(11) }
afterEach(() => { resetNotificationOwner(); sessionStorage.clear() })

for (const domain of ['runs', 'storage'] as const) {
  it.each(['read', 'clear'] as const)(`${domain}: identical oversized refetch preserves %s after reload`, action => {
    bindNotificationOwner(11)
    const record = () => domain === 'runs' ? recordRunResults(runs(60)) : recordStorageResults(jobs(100))
    record()
    if (action === 'read') state().markAllRead()
    else state().clearAll()
    reload()
    record()
    expect(state().unreadCount()).toBe(0)
    if (action === 'clear') expect(state().notifications).toEqual([])
  })
  it(`${domain}: >50 newest-first results retain the newest 50, independent of replay order`, () => {
    bindNotificationOwner(11)
    const snapshot = domain === 'runs' ? runs(60) : jobs(100)
    const record = () => domain === 'runs' ? recordRunResults(snapshot as Backup[]) : recordStorageResults(snapshot as StorageOperation[])
    record()
    expect(state().notifications.map(n => n.target?.id)).toEqual(snapshot.slice(0, 50).map(n => n.id))
    state().markAllRead()
    const ids = state().notifications.map(n => n.id)
    record()
    expect(state().unreadCount()).toBe(0)
    expect(state().notifications.map(n => n.id)).toEqual(ids)
    reload()
    snapshot.reverse()
    record()
    expect(state().unreadCount()).toBe(0)
    expect(state().notifications.map(n => n.id)).toEqual(ids)
    state().removeNotification(ids[0])
    record()
    expect(state().notifications).toHaveLength(49)
    expect(state().unreadCount()).toBe(0)
    state().clearAll()
    record()
    expect(state().notifications).toEqual([])
    reload()
    record()
    expect(state().notifications).toEqual([])
    expect(JSON.parse(sessionStorage.getItem(storageKey)!).items).toHaveLength(50)
  })
}
it('coordinates the single cap across run and storage snapshots including dismissed results', () => {
  bindNotificationOwner(11)
  const runSnapshot = runs(60)
  const jobSnapshot = jobs(100)
  recordRunResults(runSnapshot)
  recordStorageResults(jobSnapshot)
  expect(state().notifications).toHaveLength(50)
  expect(state().notifications[0].target).toEqual({ kind: 'operation', id: jobSnapshot[0].id, storageId: 'local' })
  expect(state().notifications.map(n => n.revision)).toEqual([...runSnapshot.map(n => Date.parse(n.completed_at!)), ...jobSnapshot.map(n => Date.parse(n.updated_at))].sort((a, b) => b - a).slice(0, 50))
  state().markAllRead()
  recordRunResults(runSnapshot)
  recordStorageResults(jobSnapshot)
  expect(state().unreadCount()).toBe(0)
  state().clearAll()
  reload()
  recordStorageResults(jobSnapshot)
  recordRunResults(runSnapshot)
  expect(state().notifications).toEqual([])
  const changed = { ...runSnapshot[0], completed_at: new Date().toISOString() }
  recordRunResults([changed])
  expect(state().notifications).toMatchObject([{ read: false, target: { kind: 'run', id: changed.id } }])
  expect(JSON.parse(sessionStorage.getItem(storageKey)!).items).toHaveLength(50)
})
it('equal final timestamps have deterministic bounded membership after clear and reload', () => {
  bindNotificationOwner(11)
  const snapshot = runs(60).map(n => ({ ...n, completed_at: time(60) }))
  recordRunResults(snapshot)
  expect(state().notifications.map(n => n.target?.id)).toEqual(snapshot.slice(0, 50).map(n => n.id))
  state().clearAll()
  reload()
  recordRunResults(snapshot.reverse())
  expect(state().notifications).toEqual([])
})
