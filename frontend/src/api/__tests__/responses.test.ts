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
