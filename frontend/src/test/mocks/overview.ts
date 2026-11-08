// Synthetic catalog response, never a runtime/provider acceptance substitute.
export const mockOverview = {
  observed_at: '2026-11-08T12:00:00Z', dataset: 'retained_catalog_and_current_process',
  window: { after: '2026-11-07T12:00:00Z', before: '2026-11-08T12:00:00Z', time_field: 'created_at' },
  plans: { total: 2, enabled: 2, disabled: 0, manual: 2, automatic_enabled: 0, automatic_paused: 0, without_recorded_run: { total: 0, enabled: 0, items: [] }, without_recorded_completed_run: { total: 0, enabled: 0, items: [] } },
  backup_activity: { recorded_pending: 0, recorded_running: 0, live: 0, total: 0, items: [] },
  storage_activity: { total: 0, items: [] },
  recorded_copy_failures: { backups: 0, total: 0, items: [] },
}
export function mockHistoryRecord(row: Record<string, unknown>) {
  return { source_type: 'path', source_identity: '/fixture', encrypted: false, local_available: true, remote_synced: false, plan_removed: false, retention_context: 'retained', archive_state: 'recorded', local_state: 'available', can_restore: true, can_sync: false, remote_sync_details: [], ...row }
}
