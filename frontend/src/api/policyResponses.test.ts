import { describe, expect, it } from 'vitest'
import { validateRead, validateWrite, ResponseError } from './responses'
const empty = {
  outcome: 'empty',
  deleted: 0,
  failed: 0,
  protected: 2,
  scan_complete: true,
  scan_errors: 0,
  freed_bytes: 0,
  freed_human: '0.00 B',
}
describe('policy response trust boundaries', () => {
  it.each([
    empty,
    { ...empty, outcome: 'success', deleted: 1, freed_bytes: 4 },
    { ...empty, outcome: 'partial', deleted: 1, failed: 1 },
    { ...empty, outcome: 'failure', failed: 1 },
    { ...empty, outcome: 'failure', scan_complete: false, scan_errors: 1 },
  ])('accepts truthful cleanup result %j', (data) =>
    expect(() =>
      validateWrite('/retention/cleanup-orphaned', data),
    ).not.toThrow(),
  )
  it.each([
    { deleted: 0 },
    { ...empty, failed: 1 },
    { ...empty, outcome: 'success' },
    { ...empty, outcome: 'partial', deleted: 1 },
    { ...empty, scan_complete: false },
    { ...empty, failed: -1 },
    { ...empty, freed_bytes: '0' },
  ])(
    'rejects malformed or contradictory cleanup, never assumes empty %j',
    (data) =>
      expect(() => validateWrite('/retention/cleanup-orphaned', data)).toThrow(
        ResponseError,
      ),
  )
  it.each(['/schedules/8', '/retention/7'])(
    'validates policy detail before editing %s',
    (url) => expect(() => validateRead(url, { id: 7 })).toThrow(ResponseError),
  )
})
