import { expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import { recoveryApi, recoveryTables, parseRecoveryResult } from '../recovery'

const counts = Object.fromEntries(Object.keys(recoveryTables).map(key => [key, 0]))
const result = () => ({ counts: { ...counts }, storages: [{ id: 1, type: 'local', state: 'connected' }], komodo: { state: 'disabled' }, catalog: [{ id: 1, local: 'available', copies: [{ storage_id: 1, state: 'unavailable' }] }], archives_modified: false })

it.each(['local', 'ssh', 'sftp', 'webdav', 's3', 'ftp', 'rclone'])('accepts the actual product provider %s in preview and canonical result', async type => {
  server.use(http.post('/api/v1/recovery/preview', () => HttpResponse.json({ counts, storages: [{ id: 1, type, state: 'not_tested' }], exported_at: '2026-11-09T12:00:00+00:00', archives_included: false, archive_private_keys_included: false })))
  expect((await recoveryApi.preview('encrypted', 'not-persisted', new AbortController().signal)).storages[0].type).toBe(type)
  expect(parseRecoveryResult({ ...result(), storages: [{ id: 1, type, state: 'offline' }] }).storages[0].type).toBe(type)
})
it.each([
  null,
  { ...result(), archives_modified: true },
  { ...result(), counts: { ...counts, backups: -1 } },
  { ...result(), counts: { ...counts, backups: '0' } },
  { ...result(), counts: {} },
  { ...result(), storages: null },
  { ...result(), storages: [{ id: 0, type: 'local', state: 'connected' }] },
  { ...result(), storages: [{ id: 1, type: 'unknown', state: 'connected' }] },
  { ...result(), storages: [{ id: 1, type: 'local', state: 'maybe' }] },
  { ...result(), komodo: { state: 'maybe' } },
  { ...result(), catalog: null },
  { ...result(), catalog: [{ id: 1, local: 'maybe', copies: [] }] },
  { ...result(), catalog: [{ id: 1, local: 'available', copies: null }] },
  { ...result(), catalog: [{ id: 1, local: 'available', copies: [{ storage_id: 1, state: 'connected' }] }] },
  { ...result(), ssh_public_keys: {} },
  { ...result(), ssh_public_keys: [{ name: '../escape', public_key: 'ssh-ed25519 AAAA' }] },
  { ...result(), ssh_public_keys: [{ name: 'safe', public_key: 'never-render-private-canary' }] },
])('rejects malformed/unknown result data without promoting it to a connection or activation pass %#', value => {
  expect(() => parseRecoveryResult(value)).toThrow('Invalid recovery response')
})
it('only retains typed redacted observations, not unknown server fields', () => {
  const parsed = parseRecoveryResult({ ...result(), debug: 'must-not-reach-ui-or-cache', catalog: [{ id: 1, local: 'credentials_required', copies: [{ storage_id: 1, state: 'credentials_required' }], secret: 'must-not-reach-ui-or-cache' }] })
  expect(JSON.stringify(parsed)).not.toContain('must-not-reach-ui-or-cache')
})
