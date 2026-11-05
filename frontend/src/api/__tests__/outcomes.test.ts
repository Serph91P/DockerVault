import { expect, it } from 'vitest'
import { operationOutcome } from '../outcomes'

it.each([
  [{}, 'unknown'], [{ success: true }, 'completed'], [{ success: true, warnings: ['provider warning'] }, 'partial'],
  [{ status: 'accepted' }, 'accepted'], [{ status: 'running' }, 'running'],
  [{ status: 'completed', items: [{ outcome: 'succeeded' }, { outcome: 'failed' }] }, 'partial'],
  [{ status: 'completed', succeeded: 1, failed: 0, total: 2 }, 'unknown'],
  [{ status: 'completed', succeeded: 1, failed: 1, total: 2 }, 'partial'],
  [{ status: 'restored', warnings: ['incomplete copy'] }, 'partial'],
  [{ status: 'failed' }, 'failed'], [{ success: false }, 'failed'],
  [{ status: 'failed', remote_sync_details: [{ storage_id: 1, status: 'failed' }] }, 'failed'],
  [{ status: 'failed', remote_sync_details: [{ storage_id: 1, status: 'pending' }] }, 'failed'],
  [{ status: 'completed', remote_sync_details: [{ storage_id: 1, status: 'failed' }] }, 'partial'],
  [{ status: 'completed', remote_sync_details: [{ storage_id: 1, status: 'pending' }] }, 'partial'],
  [{ status: 'completed', remote_sync_details: [{ storage_id: 1, status: 'interrupted' }] }, 'unknown'],
  [{ success: true, items: [{ outcome: 'invented' }] }, 'unknown'],
  [{ success: false, warnings: ['warning'], items: [{ outcome: 'pending' }] }, 'failed'],
  [{ state: 'failed', failed: 1, succeeded: 0, total: 1, items: [{ state: 'failed' }] }, 'failed'],
  [{ state: 'queued' }, 'accepted'], [{ state: 'running' }, 'running'],
  [{ state: 'completed', succeeded: 1, failed: 0, skipped: 1, unknown: 0, total: 2, items: [{ state: 'succeeded' }, { state: 'skipped' }] }, 'partial'],
  [{ state: 'completed', succeeded: 1, failed: 0, skipped: 0, unknown: 1, total: 2, items: [{ state: 'succeeded' }, { state: 'unknown' }] }, 'partial'],
  [{ state: 'completed', succeeded: 1, failed: 0, skipped: 0, unknown: 0, total: 1, items: [{ state: 'succeeded' }] }, 'completed'],
  [{ status: 'completed', items: [{ outcome: 'running' }] }, 'partial'],
])('classifies the business result %j as %s', (data, expected) => expect(operationOutcome(data)).toBe(expected))
