import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useSearchParams } from 'react-router-dom'
import { useState } from 'react'
import { backupsApi, targetsApi, type Backup, type BackupTarget } from '../api'
import { operationsApi } from '../api/operations'
import { queryKeys } from '../api/queryKeys'
import { operationOutcome } from '../api/outcomes'
import { sessionGeneration } from '../store/session'
import { notificationHref } from '../store/notifications'
import { QueryFeedback } from './QueryFeedback'
import BackupLogViewer from './BackupLogViewer'
import StorageOperationResults from './StorageOperationResults'

const routeId = (raw: string | null) => raw !== null && /^[1-9]\d*$/.test(raw) ? Number(raw) : NaN

export default function NotificationContext({ domain, onPlan, onArchive }: { domain: 'backups' | 'storage'; onPlan?: (plan: BackupTarget) => void; onArchive?: (archive: Backup) => void }) {
  const [params, setParams] = useSearchParams()
  const client = useQueryClient()
  const [logs, setLogs] = useState(false)
  const kind = domain === 'storage' ? 'operation' : params.has('plan') ? 'plan' : params.has('run') ? 'run' : 'archive'
  const raw = params.get(kind), id = routeId(raw)
  const storage = params.get('storage') === 'local' ? 'local' : routeId(params.get('storage'))
  const target = kind === 'operation' ? { kind, id: raw ?? '', storageId: storage } as const : { kind, id } as const
  const valid = raw !== null && notificationHref(target) !== null
  const plan = useQuery({ queryKey: queryKeys.targets.detail(id), queryFn: () => targetsApi.get(id).then(r => r.data), enabled: valid && kind === 'plan', retry: false })
  const archive = useQuery({ queryKey: queryKeys.backups.detail(id), queryFn: () => backupsApi.get(id).then(r => r.data), enabled: valid && (kind === 'run' || kind === 'archive'), retry: false })
  const operation = useQuery({ queryKey: queryKeys.operationDetail(raw, storage, sessionGeneration()), queryFn: async ({ signal }) => {
    const job = (await operationsApi.get(raw!, signal)).data
    if (job.storage_id !== storage) throw new Error('Operation storage context unavailable')
    return job
  }, enabled: valid && kind === 'operation', retry: false, refetchInterval: query => (query.state.data?.state === 'queued' || query.state.data?.state === 'running') && !client.getQueryCache().getAll().some(other => other.queryKey[0] === 'storage-operations' && other.queryKey[1] === storage && other.isActive() && Array.isArray(other.state.data) && other.state.data.some(job => job && (job.state === 'queued' || job.state === 'running'))) ? 2000 : false })
  if (raw === null) return null
  const query = kind === 'plan' ? plan : kind === 'operation' ? operation : archive
  const label = `${kind[0].toUpperCase()}${kind.slice(1)} ${kind === 'operation' ? raw : `#${raw}`}`
  const close = () => { const next = new URLSearchParams(params); next.delete(kind); if (kind === 'operation') next.delete('storage'); setParams(next) }
  return <section className="rounded-xl border border-primary-500/50 p-4 space-y-3" aria-label={`${label} context`} role={!valid || query.isError ? 'alert' : undefined}>
    <h2 className="font-semibold break-all">{label}</h2>
    <button className="btn-secondary" onClick={close}>Dismiss context (does not cancel work)</button>
    {!valid ? <p>Invalid notification target. No action was submitted.</p> : <>
      <QueryFeedback name={label} query={query} />
      {query.isError && <p>The object was removed, is unavailable, or access was denied. You can retry or use the current list below.</p>}
      {kind === 'plan' && plan.data && <><p>{plan.data.name} - {plan.data.removed_at ? 'Removed plan; retained history remains available' : plan.data.enabled ? 'Enabled plan (not a run result)' : 'Disabled plan'}</p>{!plan.data.removed_at && <button className="btn-secondary" onClick={() => onPlan?.(plan.data)}>Open existing plan editor</button>}</>}
      {(kind === 'run' || kind === 'archive') && archive.data && <><p>{archive.data.target_name ?? `Plan #${archive.data.target_id}`} - {operationOutcome(archive.data)}</p><p>Started: {archive.data.started_at ?? 'Unknown'}; updated: {archive.data.completed_at ?? 'Not final'}</p><ul>{archive.data.remote_sync_details?.map(copy => <li key={copy.storage_id}>Copy #{copy.storage_id}: {copy.status}</li>)}</ul><button className="btn-secondary" onClick={() => setLogs(true)}>Open run logs</button>{kind === 'archive' && <button className="btn-secondary" onClick={() => onArchive?.(archive.data)}>Browse archive</button>}{logs && <BackupLogViewer backupId={id} onClose={() => setLogs(false)} />}</>}
      {kind === 'operation' && operation.data && <StorageOperationResults operations={[operation.data]} loading={operation.isPending} stale={operation.isError} pending={false} error={false} onRefresh={() => void operation.refetch()} readOnly />}
    </>}
  </section>
}
