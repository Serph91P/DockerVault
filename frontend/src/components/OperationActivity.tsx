import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { operationsApi, operationActive } from '../api/operations'
import { backupsApi } from '../api'
import { queryKeys } from '../api/queryKeys'
import { sessionGeneration } from '../store/session'
import { notificationHref } from '../store/notifications'
import { recordRunResults, recordStorageResults } from '../store/resultNotifications'
import { operationOutcome } from '../api/outcomes'
import { QueryFeedback } from './QueryFeedback'
import StorageOperationResults from './StorageOperationResults'


export default function OperationActivity() {
  const client = useQueryClient()
  const jobs = useQuery({ queryKey: queryKeys.operationList(sessionGeneration()), queryFn: async ({ signal }) => (await operationsApi.list(undefined, signal)).data, retry: false, staleTime: 0,
    refetchInterval: query => query.state.data?.some(operationActive) && !client.getQueryCache().getAll().some(other => other.queryHash !== query.queryHash && other.queryKey[0] === 'storage-operations' && other.queryKey[1] !== 'list' && other.isActive() && (Array.isArray(other.state.data) ? other.state.data : [other.state.data]).some(job => job && operationActive(job))) ? 2000 : false })
  const runs = useQuery({ queryKey: queryKeys.backups.list({ limit: 10 }), queryFn: () => backupsApi.list({ limit: 10 }).then(r => r.data), retry: false })
  useEffect(() => { if (!jobs.isError && jobs.data) recordStorageResults(jobs.data) }, [jobs.data, jobs.isError])
  useEffect(() => { if (!runs.isError && runs.data) recordRunResults(runs.data) }, [runs.data, runs.isError])
  return <section aria-label="Operations and results" className="space-y-3 border-b border-dark-600 p-3">
    <h2 className="font-semibold">Operations and results</h2>
    <p className="text-sm text-dark-300">Storage jobs and runs below are backend records. Synchronous restore, test and cleanup results are UI-session observations only; closing this panel does not cancel work.</p>
    <QueryFeedback name="Storage operations" query={jobs} />
    {jobs.data && <><StorageOperationResults operations={jobs.data} loading={jobs.isPending} stale={jobs.isError} pending={false} error={false} onRefresh={() => void jobs.refetch()} readOnly /><ul>{jobs.data.map(job => <li key={job.id}><Link className="text-primary-300 underline break-all" to={notificationHref({ kind: 'operation', id: job.id, storageId: job.storage_id })!}>Inspect operation {job.id}</Link></li>)}</ul></>}
    <QueryFeedback name="Recent runs" query={runs} />
    <ul>{runs.data?.map(run => <li key={run.id} className="py-2"><Link className="text-primary-300 underline" to={`/history?run=${run.id}`}>Run #{run.id}</Link>: {operationOutcome(run)}; started {run.started_at ?? 'unknown'}, updated {run.completed_at ?? 'not final'}. {run.remote_sync_details?.map(copy => <span key={copy.storage_id}>Copy #{copy.storage_id}: {copy.status}. </span>)}</li>)}</ul>
  </section>
}
