import { expect, it } from 'vitest'
import { validateRead } from '../responses'
const operation = { id: 'b658c36b-4872-4364-aafc-72b227142815', action: 'storage_delete', storage_id: 'local', state: 'completed', total: 1, completed: 1, succeeded: 1, failed: 0, skipped: 0, unknown: 0, path: '', created_at: '2026-01-01', updated_at: '2026-01-01', items: [{ path: 'synthetic', state: 'succeeded' }] }
it.each([
  { ...operation, total: -1 }, { ...operation, completed: 0 }, { ...operation, items: [] },
])('malformed operation counters/details are unavailable, not confirmed completion: %j', value => {
  expect(() => validateRead('/operations', [value])).toThrow()
})
it('valid canonical item counts remain readable', () => expect(validateRead('/operations', [operation])).toEqual([operation]))
it('a malformed nested copy collection is not a successful archive read', () => expect(() => validateRead('/backups/1', { id: 1, target_id: 1, status: 'completed', created_at: '2026-01-01', remote_sync_details: {} })).toThrow())
it('a malformed archive date is unavailable before a relative-date renderer can crash', () => expect(() => validateRead('/backups', [{ id: 1, target_id: 1, status: 'completed', created_at: 'not-a-date' }])).toThrow())
it('accepts the native populated SSH list with numeric filesystem timestamps', () => {
  const keys = [{ name: 'synthetic_key', public_key: 'ssh-ed25519 synthetic', private_path: '/app/data/ssh_keys/synthetic_key', fingerprint: 'synthetic fingerprint', created_at: 1793900000.25 }]
  expect(validateRead('/storage/ssh-keys', keys)).toEqual(keys)
})
it.each([NaN, Infinity, '2026-11-01T00:00:00Z'])('rejects non-native SSH timestamps: %s', created_at => {
  expect(() => validateRead('/storage/ssh-keys', [{ name: 'synthetic_key', public_key: 'synthetic', created_at }])).toThrow()
})
it('keeps numeric timestamps invalid for archive resources', () => {
  expect(() => validateRead('/backups', [{ id: 1, target_id: 1, status: 'completed', created_at: 1793900000.25 }])).toThrow()
})
const inventoryItem = { kind: 'schedule', id: 1, name: 'Synthetic schedule', expression: '0 2 * * *', warnings: ['Review intent'] }
it('accepts native populated schedule compatibility items', () => {
  const inventory = { contract: 'synthetic contract', timezone: 'UTC', items: [inventoryItem, { ...inventoryItem, kind: 'target_legacy' }] }
  expect(validateRead('/schedules/compatibility/inventory', inventory)).toEqual(inventory)
})
it.each([null, { ...inventoryItem, warnings: null }, { ...inventoryItem, warnings: [null] }, { ...inventoryItem, name: {} }, { ...inventoryItem, expression: [] }, { ...inventoryItem, kind: 'unknown' }, { ...inventoryItem, id: -1 }])('rejects malformed consumed inventory items: %j', item => {
  expect(() => validateRead('/schedules/compatibility/inventory', { contract: 'synthetic contract', timezone: 'UTC', items: [item] })).toThrow()
})
