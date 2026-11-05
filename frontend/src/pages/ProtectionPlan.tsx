import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { targetsApi, backupsApi, schedulesApi, retentionApi, storageApi, dockerApi } from '../api'
import { queryKeys } from '../api/queryKeys'
import { operationOutcome } from '../api/outcomes'
import { responseStatus } from '../api/responses'
import { QueryFeedback } from '../components/QueryFeedback'
import { PageHeader } from '../components/ui/workspace'
import RemovalDialog from '../components/RemovalDialog'
import BackupWizard from '../components/BackupWizard'
import StepSummary from '../components/BackupWizard/StepSummary'
import { targetToWizardData, availableVolumes } from '../components/BackupWizard/planEditorModel'
import { planId, contextSearch, returnPath } from './protectionRoutes'

export default function ProtectionPlan({ editing = false }: { editing?: boolean }) {
  const { id: raw } = useParams()
  const id = planId(raw)
  const [params] = useSearchParams()
  const back = returnPath(params.get('returnTo'))
  const navigate = useNavigate()
  const client = useQueryClient()
  const [removing, setRemoving] = useState(false)
  const [runId, setRunId] = useState<number | null>(null)
  const target = useQuery({
    queryKey: queryKeys.targets.detail(id || 0),
    queryFn: () => targetsApi.get(id!).then((r) => r.data),
    enabled: !!id,
    retry: false,
  })
  const schedules = useQuery({
    queryKey: queryKeys.schedules.list(),
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })
  const retention = useQuery({
    queryKey: queryKeys.retention.list(),
    queryFn: () => retentionApi.listPolicies().then((r) => r.data),
  })
  const storages = useQuery({
    queryKey: queryKeys.storages.list(),
    queryFn: () => storageApi.list().then((r) => r.data),
  })
  const containers = useQuery({
    queryKey: queryKeys.containers.list(),
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
  })
  const stacks = useQuery({
    queryKey: queryKeys.stacks.list(),
    queryFn: () => dockerApi.listStacks().then((r) => r.data),
  })
  const volumes = useQuery({
    queryKey: queryKeys.volumes.list(),
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
  })
  const latest = useQuery({
    queryKey: queryKeys.backups.list({ target_id: id || 0, limit: 1 }),
    queryFn: () => backupsApi.list({ target_id: id!, limit: 1 }).then((r) => r.data),
    enabled: !!id,
  })
  const run = useQuery({
    queryKey: queryKeys.backups.detail(runId || 0),
    queryFn: () => backupsApi.get(runId!).then((r) => r.data),
    enabled: !!runId,
    refetchInterval: (q) => (['accepted', 'running'].includes(operationOutcome(q.state.data)) ? 2000 : false),
  })
  const trigger = useMutation({
    mutationFn: () => backupsApi.create(id!, 'full'),
    onSuccess: (response) => {
      if (Number.isSafeInteger(response.data?.id) && response.data.id > 0) setRunId(response.data.id)
      void client.invalidateQueries({ queryKey: queryKeys.backups.all })
    },
  })
  if (!id)
    return (
      <section className="space-y-3">
        <h1 className="text-2xl">Invalid protection plan address</h1>
        <p>No plan was requested.</p>
        <Link to={back}>Return to plans</Link>
      </section>
    )
  const t = target.data
  const detail = `/protection/${id}${contextSearch(back)}`
  if (editing && t && !t.removed_at)
    return (
      <div className="space-y-4">
        <Link className="underline" to={detail}>
          Return to plan without saving
        </Link>
        <BackupWizard
          editTarget={t}
          isOpen
          onClose={() => navigate(detail)}
          onSaved={(saved) => navigate(`/protection/${saved}${contextSearch(back)}`)}
        />
      </div>
    )
  const data = t ? targetToWizardData(t) : null
  const sourceQuery = t?.target_type === 'container' ? containers : t?.target_type === 'stack' ? stacks : volumes
  const sourceName = t?.container_name || t?.stack_name || t?.volume_name
  const missing = t?.target_type !== 'path' && sourceQuery.data && !sourceQuery.data.some((s) => s.name === sourceName)
  return (
    <div className="space-y-6" aria-label={`Plan #${id} context`}>
      <Link className="underline text-primary-300" to={back}>
        Return to protection list
      </Link>
      <PageHeader
        title={t ? t.name : 'Protection plan'}
        description="Authoritative stored configuration, independently of existing archive copies."
      />
      <QueryFeedback name={`Plan #${id}`} query={target} />
      {target.isError && (
        <section role="alert">
          <h2>Protection plan unavailable</h2>
          <p>
            {responseStatus(target.error) === 404
              ? 'No plan at this address. Retained archives may still exist.'
              : responseStatus(target.error) === 403
                ? 'Access denied.'
                : 'Could not read this plan. Retry without losing your return context.'}
          </p>
          <Link to="/history">Open retained history</Link>
        </section>
      )}
      {t && data && (
        <>
          {t.removed_at ? (
            <p role="status">
              Removed plan: automation is disabled; retained archives remain in History & Restore. Configuration is
              read-only.
            </p>
          ) : (
            <div className="flex flex-wrap gap-3">
              <Link className="btn-primary" to={`/protection/${id}/edit${contextSearch(back)}`}>
                Edit plan
              </Link>
              <button
                type="button"
                className="btn-secondary"
                disabled={trigger.isPending || !t.enabled || !!missing}
                onClick={() => trigger.mutate()}
              >
                Run now
              </button>
              <button type="button" className="btn-secondary" onClick={() => setRemoving(true)}>
                Remove plan (retain archives)
              </button>
            </div>
          )}
          <p>
            {t.enabled && !t.removed_at
              ? 'Enabled/configured — not a health guarantee.'
              : 'Disabled or removed — manual and scheduled runs blocked.'}
          </p>
          {t.target_type === 'path' ? (
            <p>Path availability is checked by the backend at execution.</p>
          ) : (
            <>
              <QueryFeedback name="Source discovery" query={sourceQuery} />
              <p>
                {sourceQuery.isError || !sourceQuery.data
                  ? 'Source availability unknown.'
                  : missing
                    ? 'Source missing. Scope has not been widened; retained archives are still accessible.'
                    : 'Source discovered. This is not a consistency or provider reachability check.'}
              </p>
            </>
          )}
          <QueryFeedback name="Latest run" query={latest} />
          {latest.data && (
            <p>
              {latest.data[0]
                ? `Latest run #${latest.data[0].id}: ${operationOutcome(latest.data[0])}`
                : 'No run yet. No successful backup has been proven.'}
            </p>
          )}
          {trigger.isPending && <p role="status">Requesting run acceptance…</p>}
          {trigger.isError && (
            <p role="alert">
              Run was not confirmed.{' '}
              {responseStatus(trigger.error) === 409
                ? 'Plan is disabled or unavailable.'
                : 'Read current History before retrying; a lost response is not proof no run was created.'}
            </p>
          )}
          {trigger.isSuccess && !runId && <p role="alert">Run identity unknown. Inspect History before retrying.</p>}
          {runId && (
            <section className="space-y-2" aria-label="Requested run">
              <p role="status">Request accepted as run #{runId}; acceptance is not successful completion.</p>
              <QueryFeedback name={`Run #${runId}`} query={run} />
              {run.data && (
                <p>
                  Actual backend outcome: {operationOutcome(run.data)}. Started: {run.data.started_at || 'not started'};
                  finished: {run.data.completed_at || 'not final'}.
                </p>
              )}
              <Link className="underline" to={`/history?run=${runId}`}>
                Open run and logs
              </Link>
            </section>
          )}
          <QueryFeedback name="Schedule configuration" query={schedules} />
          <QueryFeedback name="Retention configuration" query={retention} />
          <QueryFeedback name="Destinations" query={storages} />
          <StepSummary
            data={data}
            schedules={schedules.data || []}
            policies={retention.data || []}
            storages={storages.data || []}
            availableVolumes={availableVolumes(data, containers.data || [], stacks.data || [])}
            isCreating={false}
          />
          <Link className="btn-secondary" to={`/history?plan=${id}`}>
            Archives, logs, browse and restore for this plan
          </Link>
          {removing && (
            <RemovalDialog
              intent={{ kind: 'plan', targetId: id, name: t.name }}
              onClose={() => setRemoving(false)}
              onChanged={() => {
                void client.invalidateQueries({ queryKey: queryKeys.targets.all })
                void client.invalidateQueries({ queryKey: queryKeys.schedules.all })
              }}
            />
          )}
        </>
      )}
    </div>
  )
}
export function ProtectionNew() {
  const navigate = useNavigate()
  const [params] = useSearchParams()
  const back = returnPath(params.get('returnTo'))
  return (
    <div className="space-y-4">
      <Link className="underline" to={back}>
        Return to protection list
      </Link>
      <BackupWizard
        isOpen
        onClose={() => navigate(back)}
        onSaved={(id) => navigate(`/protection/${id}${contextSearch(back)}`)}
      />
    </div>
  )
}
