import { expect, it } from 'vitest'
import { validateRead } from './responses'
const row = { id: 1, target_id: 1, status: 'failed', created_at: '2026-01-01T00:00:00Z', source_type: 'path', source_identity: '/fixture', encrypted: false, local_available: false, remote_synced: false, plan_removed: false, retention_context: 'default', archive_state: 'never_created', local_state: 'not_recorded', can_restore: false, can_sync: false, remote_sync_details: [] }
const page = { items: [row], total: 501, offset: 0, limit: 50, snapshot: 501, dataset: 'Catalog' }
it('validates full history envelopes and detail before caching', () => {
 expect(() => validateRead('/backups/history', page)).not.toThrow()
 expect(() => validateRead('/backups/history/1', row)).not.toThrow()
 for (const bad of [{...page, total: '501'}, {...page, total: 0}, {...page, items: [{}]}, {...page, snapshot: -1}, {...page, items: [{...row, can_restore: 'yes'}]}, {...page, items: [{...row, local_state: 'invented'}]}]) expect(() => validateRead('/backups/history', bad)).toThrow()
 expect(() => validateRead('/backups/history/1', {...row, archive_state: 'bad'})).toThrow()
})
