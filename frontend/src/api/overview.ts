import api from './index'

interface PlanGap { total: number; enabled: number; items: { plan_id: number; name: string }[] }
export interface OverviewSummary {
  observed_at: string
  dataset: 'retained_catalog_and_current_process'
  window: { after: string; before: string; time_field: 'created_at' }
  plans: { total: number; enabled: number; disabled: number; manual: number; automatic_enabled: number; automatic_paused: number; without_recorded_run: PlanGap; without_recorded_completed_run: PlanGap }
  backup_activity: { recorded_pending: number; recorded_running: number; live: number; total: number; items: { backup_id: number; plan_id: number | null; name: string | null; catalog_status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled' | null; stage: string | null; live_worker: boolean }[] }
  storage_activity: { total: number; items: { operation_id: string; storage_id: string; state: 'queued' | 'running'; stage: string }[] }
  recorded_copy_failures: { total: number; backups: number; items: { backup_id: number; storage_id: number; storage_name: string | null; status: 'failed' }[] }
}
export const overviewApi = {
  summary: (limit = 5, signal?: AbortSignal) => api.get<OverviewSummary>('/overview/summary', { params: { limit }, signal }),
}
