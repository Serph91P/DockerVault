import type { StorageOperation } from '../api/operations'
import type { Backup } from '../api'
import { operationActive } from '../api/operations'
import { useNotificationStore, type NotificationCode } from './notifications'
import { operationOutcome } from '../api/outcomes'

export function recordRunResults(backups: Backup[]) {
  for (const backup of backups) {
    const outcome = operationOutcome(backup)
    const code: NotificationCode | undefined = outcome === 'completed' ? 'run-completed' : outcome === 'partial' ? 'run-partial' : outcome === 'failed' ? 'run-failed' : outcome === 'cancelled' ? 'run-cancelled' : undefined
    if (code) useNotificationStore.getState().addNotification({ code, target: { kind: 'run', id: backup.id }, revision: Date.parse(backup.completed_at ?? backup.created_at) || 0, type: 'info', title: '', message: '' })
  }
}
export function recordStorageResults(operations: StorageOperation[]) {
  for (const job of operations) {
    if (operationActive(job)) continue
    const outcome = operationOutcome(job)
    const code: NotificationCode = outcome === 'failed' ? 'operation-failed' : outcome === 'completed' ? 'operation-completed' : 'operation-partial'
    useNotificationStore.getState().addNotification({ code, target: { kind: 'operation', id: job.id, storageId: job.storage_id }, revision: Date.parse(job.updated_at) || 0, counts: { succeeded: job.succeeded, failed: job.failed, total: job.total }, type: 'info', title: '', message: '' })
  }
}
