import { useQueries, useQuery } from '@tanstack/react-query'
import { Link, Navigate, useSearchParams } from 'react-router-dom'
import { targetsApi, backupsApi, dockerApi, storageApi, schedulesApi, type BackupTarget } from '../api'
import { queryKeys } from '../api/queryKeys'
import { operationOutcome } from '../api/outcomes'
import { QueryFeedback } from '../components/QueryFeedback'
import { EntityTable, FilterBar, PageHeader, QueryState } from '../components/ui/workspace'
import { contextSearch, planId, protectionSearch } from './protectionRoutes'

export default function Protection() {
  const [params, setParams] = useSearchParams()
  const back = `/protection${protectionSearch(params)}`
  const plans = useQuery({ queryKey: queryKeys.targets.list(), queryFn: () => targetsApi.list().then((r) => r.data) })
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
  const storages = useQuery({
    queryKey: queryKeys.storages.list(),
    queryFn: () => storageApi.list().then((r) => r.data),
  })
  const jobs = useQuery({
    queryKey: queryKeys.schedules.jobs(),
    queryFn: () => schedulesApi.getJobs().then((r) => r.data),
  })
  const rows = plans.data || []
  const latest = useQueries({
    queries: rows.map((t) => ({
      queryKey: queryKeys.backups.list({ target_id: t.id, limit: 1 }),
      queryFn: () => backupsApi.list({ target_id: t.id, limit: 1 }).then((r) => r.data),
    })),
  })
  const sourceState = (t: BackupTarget) => {
    const query = t.target_type === 'container' ? containers : t.target_type === 'stack' ? stacks : volumes
    if (t.target_type === 'path') return 'Path checked at execution'
    if (!query.data || query.isError) return 'Source unknown'
    return query.data.some((source) => source.name === (t.container_name || t.stack_name || t.volume_name))
      ? 'Source discovered (not a consistency check)'
      : 'Source missing'
  }
  const q = params.get('q')?.toLowerCase() || '',
    type = params.get('type'),
    status = params.get('status'),
    sort = params.get('sort') || 'name'
  const filtered = rows
    .map((target, index) => ({ target, run: latest[index] }))
    .filter(({ target: t, run }) => {
      if (
        q &&
        !`${t.name} ${t.container_name || t.stack_name || t.volume_name || t.host_path}`.toLowerCase().includes(q)
      )
        return false
      if (type && ['container', 'stack', 'volume', 'path'].includes(type) && t.target_type !== type) return false
      const last = run.data?.[0]
      return status === 'active'
        ? t.enabled
        : status === 'disabled'
          ? !t.enabled
          : status === 'missing'
            ? sourceState(t) === 'Source missing'
            : status === 'unknown'
              ? sourceState(t) === 'Source unknown' || run.isError
              : status === 'never'
                ? !!run.data && !last
                : status === 'completed'
                  ? operationOutcome(last) === 'completed'
                  : status === 'failed'
                    ? operationOutcome(last) === 'failed'
                    : true
    })
    .sort((a, b) =>
      sort === 'type'
        ? a.target.target_type.localeCompare(b.target.target_type) || a.target.name.localeCompare(b.target.name)
        : sort === 'created'
          ? b.target.created_at.localeCompare(a.target.created_at)
          : sort === 'last-backup'
            ? (b.run.data?.[0]?.created_at || '').localeCompare(a.run.data?.[0]?.created_at || '')
            : a.target.name.localeCompare(b.target.name)
    )
  const set = (key: string, value: string) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(new URLSearchParams(protectionSearch(next)))
  }
  const notification = planId(params.get('plan'))
  if (notification) return <Navigate replace to={`/protection/${notification}${contextSearch(back)}`} />
  return (
    <div className="space-y-6">
      <PageHeader
        title="Protection"
        description="Configured plans are not proof of successful backups. Open a plan to inspect scope, edit, or run it."
        actions={
          <Link className="btn-primary" to={`/protection/new${contextSearch(back)}`}>
            Set up protection
          </Link>
        }
      />
      {params.has('plan') && !notification && (
        <p role="alert">Invalid plan notification target. No action submitted.</p>
      )}
      <FilterBar>
        <label>
          Search plans
          <input
            className="input block"
            type="search"
            value={params.get('q') || ''}
            onChange={(e) => set('q', e.target.value)}
          />
        </label>
        <label>
          Source type
          <select className="input block" value={type || ''} onChange={(e) => set('type', e.target.value)}>
            <option value="">All types</option>
            {['container', 'stack', 'volume', 'path'].map((v) => (
              <option key={v}>{v}</option>
            ))}
          </select>
        </label>
        <label>
          Plan status
          <select className="input block" value={status || ''} onChange={(e) => set('status', e.target.value)}>
            <option value="">All states</option>
            {[
              ['active', 'Enabled/configured'],
              ['disabled', 'Disabled'],
              ['completed', 'Latest run completed'],
              ['failed', 'Latest run failed'],
              ['missing', 'Source missing'],
              ['unknown', 'Unknown'],
              ['never', 'No runs yet'],
            ].map(([v, label]) => (
              <option value={v} key={v}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <label>
          Sort plans
          <select className="input block" value={sort} onChange={(e) => set('sort', e.target.value)}>
            {[
              ['name', 'Name'],
              ['type', 'Source type'],
              ['last-backup', 'Latest run'],
              ['created', 'Created'],
            ].map(([v, label]) => (
              <option value={v} key={v}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="btn-secondary" onClick={() => setParams({})}>
          Clear filters
        </button>
      </FilterBar>
      <QueryFeedback name="Plans" query={plans} />
      <QueryFeedback name="Source containers" query={containers} />
      <QueryFeedback name="Source stacks" query={stacks} />
      <QueryFeedback name="Source volumes" query={volumes} />
      <QueryFeedback name="Scheduled jobs" query={jobs} />
      <QueryFeedback name="Destinations" query={storages} />
      {plans.data &&
        (filtered.length ? (
          <EntityTable label="Protection plans">
            <thead>
              <tr>
                {['Plan / source', 'Configuration', 'Latest actual run', 'Next execution', 'Destinations'].map(
                  (label) => (
                    <th className="p-3 text-left" key={label} scope="col">
                      {label}
                    </th>
                  )
                )}
              </tr>
            </thead>
            <tbody>
              {filtered.map(({ target: t, run }) => (
                <tr className="border-t border-dark-600" key={t.id}>
                  <td className="p-3 max-w-sm break-words">
                    <Link className="underline text-primary-300" to={`/protection/${t.id}${contextSearch(back)}`}>
                      {t.name}
                    </Link>
                    <p>
                      {t.target_type}: {t.container_name || t.stack_name || t.volume_name || t.host_path}
                    </p>
                    <p className="text-sm">{sourceState(t)}</p>
                  </td>
                  <td className="p-3">{t.enabled ? 'Enabled · configured' : 'Disabled'}</td>
                  <td className="p-3">
                    <QueryFeedback name={`Latest run for ${t.name}`} query={run} />
                    {run.data &&
                      (run.data[0] ? (
                        <Link className="underline" to={`/history?run=${run.data[0].id}`}>
                          #{run.data[0].id} · {operationOutcome(run.data[0])}
                        </Link>
                      ) : (
                        'No run yet — no successful backup proven'
                      ))}
                  </td>
                  <td className="p-3">
                    {jobs.data
                      ? jobs.data.find((job) => job.id === `backup_target_${t.id}`)?.next_run ||
                        'No active scheduled job'
                      : 'Unknown'}
                  </td>
                  <td className="p-3">
                    Local
                    {t.remote_storage_ids
                      ?.map(
                        (id) => `; ${storages.data?.find((s) => s.id === id)?.name || `remote #${id} (unverified)`}`
                      )
                      .join('')}
                    {t.delete_local_after_sync && ' (remove local after required sync)'}
                  </td>
                </tr>
              ))}
            </tbody>
          </EntityTable>
        ) : (
          <QueryState
            state={rows.length ? 'filtered-empty' : 'empty'}
            title={rows.length ? 'No matching plans' : 'No protection plans configured'}
            description="Retained archives remain independently accessible in History & Restore."
          />
        ))}
      <div className="flex flex-wrap gap-4">
        <Link className="underline" to="/protection/schedules">
          Schedules
        </Link>
        <Link className="underline" to="/protection/retention">
          Retention
        </Link>
        <Link className="underline" to="/history">
          History & Restore (including removed plans)
        </Link>
      </div>
    </div>
  )
}
