import { expect, it } from 'vitest'
import { validateRead, validateWrite } from './responses'
const providers = ['local','ssh','sftp','webdav','s3','ftp','rclone']
it('treats malformed scheduled-job inventory as unknown rather than no scheduled work', () => {
  const job = { id: 'backup_target_1', name: 'fixture', next_run: '2026-11-06T00:00:00Z' }
  expect(() => validateRead('/schedules/jobs', [job])).not.toThrow()
  expect(() => validateRead('/schedules/jobs', [{...job,id:'retention_cleanup'}, {...job,id:'session_cleanup'}])).not.toThrow()
  for (const value of [{}, null, [{...job,next_run:'invalid'}], [{...job,id:1}]]) expect(() => validateRead('/schedules/jobs', value)).toThrow()
})
it.each(providers)('rejects array-coerced %s storage discriminator on GET/POST/PUT', type => {
  const value = { id: 1, name: 'fixture', storage_type: [type], enabled: false, base_path: '/fixture' }
  expect(() => validateRead('/storage', [value])).toThrow()
  expect(() => validateWrite('/storage', value)).toThrow()
  expect(() => validateWrite('/storage/1', value)).toThrow()
})
it('rejects array-coerced source, run and operation/item enums without losing valid controls', () => {
  const target = { id: 1, name: 'fixture', enabled: true, target_type: 'container' }
  expect(() => validateRead('/targets', [target])).not.toThrow()
  expect(() => validateRead('/targets', [{ ...target, target_type: ['container'] }])).toThrow()
  const run = { id: 1, target_id: 1, status: 'completed', created_at: '2026-11-05T00:00:00Z' }
  expect(() => validateRead('/backups', [run])).not.toThrow()
  expect(() => validateRead('/backups', [{ ...run, status: ['completed'] }])).toThrow()
  const job = { id: 'fixture', action: 'storage_delete', state: 'queued', storage_id: 'local', total: 1, completed: 0, succeeded: 0, failed: 0, skipped: 0, unknown: 0, created_at: '', updated_at: '', path: '', items: [{ path: 'fixture', state: 'pending' }] }
  expect(() => validateRead('/operations', [job])).not.toThrow()
  expect(() => validateRead('/operations', [{ ...job, state: ['queued'] }])).toThrow()
  expect(() => validateRead('/operations', [{ ...job, items: [{ path: 'fixture', state: ['pending'] }] }])).toThrow()
})
