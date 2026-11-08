import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import api, { backupsApi, dockerApi, schedulesApi, settingsApi, storageApi, targetsApi } from '../api'
import { overviewApi } from '../api/overview'
import { queryKeys } from '../api/queryKeys'
import { QueryFeedback } from '../components/QueryFeedback'
import { useAuthStore } from '../store/auth'

const link = 'inline-flex min-h-11 items-center underline underline-offset-4 text-primary-300 break-words'
const section = 'space-y-3 border-t border-dark-700 pt-5 min-w-0'
type FailureSnapshot = { page: Awaited<ReturnType<typeof backupsApi.history>>['data']; window: { after: string; before: string } }
const failureSnapshots = new Map<number | undefined, FailureSnapshot>()
function bytes(value: number) {
  if (!value) return '0 B'
  const power = Math.min(4, Math.floor(Math.log(value) / Math.log(1024)))
  return `${(value / 1024 ** power).toFixed(1)} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][power]}`
}
export default function Dashboard() {
  const owner = useAuthStore(state => state.user?.id)
  const summaryQuery = useQuery({ queryKey: queryKeys.overview.summary(5, owner), queryFn: ({ signal }) => overviewApi.summary(5, signal).then(r => r.data), retry: false })
  const summary = summaryQuery.data
  const targets = useQuery({ queryKey: queryKeys.targets.list(), queryFn: () => targetsApi.list().then(r => r.data) })
  const containers = useQuery({ queryKey: queryKeys.containers.list(), queryFn: () => dockerApi.listContainers().then(r => r.data) })
  const volumes = useQuery({ queryKey: queryKeys.volumes.list(), queryFn: () => dockerApi.listVolumes().then(r => r.data) })
  const stacks = useQuery({ queryKey: queryKeys.stacks.list(), queryFn: () => dockerApi.listStacks().then(r => r.data) })
  const schedules = useQuery({ queryKey: queryKeys.schedules.list(), queryFn: () => schedulesApi.list().then(r => r.data) })
  const jobs = useQuery({ queryKey: queryKeys.schedules.jobs(), queryFn: () => schedulesApi.getJobs().then(r => r.data) })
  const storages = useQuery({ queryKey: queryKeys.storages.list(), queryFn: () => storageApi.list().then(r => r.data) })
  const health = useQuery({ queryKey: ['docker-health'], queryFn: () => dockerApi.getHealth().then(r => r.data.status === 'healthy'), retry: false })
  const encryption = useQuery({ queryKey: ['encryption-status'], queryFn: ({ signal }) => api.get<{ setup_completed: boolean; encryption_enabled: boolean; onboarding_deferred?: boolean }>('/encryption/status', { signal }).then(r => r.data), retry: false })
  const system = useQuery({ queryKey: queryKeys.systemInfo, queryFn: () => settingsApi.getSystemInfo().then(r => r.data), refetchInterval: 60000 })
  const archiveParams = { view: 'archives', limit: 5 }
  const archives = useQuery({ queryKey: queryKeys.backups.history(archiveParams), queryFn: ({ signal }) => backupsApi.history(archiveParams, signal).then(r => r.data), retry: false })
  const runParams = { view: 'runs', limit: 1 }
  const runs = useQuery({ queryKey: queryKeys.backups.history(runParams), queryFn: ({ signal }) => backupsApi.history(runParams, signal).then(r => r.data), retry: false })
  const completedParams = { view: 'runs', status: 'completed', limit: 1 }
  const completed = useQuery({ queryKey: queryKeys.backups.history(completedParams), queryFn: ({ signal }) => backupsApi.history(completedParams, signal).then(r => r.data), retry: false })
  const failureParams = { view: 'runs', status: 'failed', limit: 5, after: summary?.window.after ?? '', before: summary?.window.before ?? '' }
  // Keep the last confirmed failure page and its window so a later error for a
  // newer window shows the old snapshot (with original bounds) as stale. The
  // snapshot is owner-scoped so a session/owner change can never relabel or
  // resurrect another operator's confirmed failures.
  const failures = useQuery({ queryKey: queryKeys.backups.history(failureParams), queryFn: ({ signal }) => backupsApi.history(failureParams, signal).then(r => {
    failureSnapshots.set(owner, { page: r.data, window: { after: failureParams.after, before: failureParams.before } })
    return r.data
  }), enabled: Boolean(summary), retry: false })
  const snapshot = failureSnapshots.get(owner) ?? null
  const failureData = failures.data ?? snapshot?.page
  const failureStale = failures.isError && Boolean(snapshot)
  const failureWindow = (failureStale || (failures.data == null && snapshot)) && snapshot ? snapshot.window : { after: summary?.window.after ?? '', before: summary?.window.before ?? '' }
  const failureSearch = new URLSearchParams({ view: 'runs', status: 'failed', limit: '5', after: failureWindow.after, before: failureWindow.before }).toString()
  const empty = summary?.plans.total === 0 && runs.data?.total === 0 && !summaryQuery.isError && !runs.isError
  const disk = system.data
  const low = disk?.disk_status === 'available' && disk.disk_free < 1024 ** 3
  const noRecordedRun = summary?.plans.without_recorded_run
  const noCompletedRun = summary?.plans.without_recorded_completed_run
  const nextJobs = jobs.data?.filter(job => job.next_run).sort((a, b) => a.next_run!.localeCompare(b.next_run!) || a.id.localeCompare(b.id))
  const readinessIncomplete = encryption.data && !encryption.data.setup_completed && !encryption.data.onboarding_deferred
  const problems = summaryQuery.isError || failures.isError || health.isError || health.data !== true || system.isError || encryption.isError || !encryption.data || readinessIncomplete || low || (summary?.recorded_copy_failures.total ?? 0) > 0 || (noCompletedRun?.total ?? 0) > 0 || (failureData?.total ?? 0) > 0 || disk?.disk_status !== 'available'
  return <div className="space-y-6 min-w-0 break-words">
    <header className="flex flex-wrap items-start justify-between gap-3">
      <div><h1 className="text-2xl font-bold">Overview</h1><p className="text-dark-300">Overview of your Docker backups</p></div>
      <Link className="min-h-11 inline-flex items-center rounded-lg border border-transparent bg-primary-700 px-4 py-2 text-sm font-medium text-white hover:bg-primary-800" to="/protection/new">New backup plan</Link>
    </header>
    <section aria-labelledby="overview-attention" className={section}>
      <h2 id="overview-attention" className="text-xl font-semibold">Needs attention</h2>
      <QueryFeedback name="Overview summary" query={summaryQuery} />
      {empty && <p className="font-semibold">Start with your first successful backup. Follow the readiness sequence below; a saved plan is not an archive.</p>}
      {low && <p role="alert">Low local capacity: {bytes(disk.disk_free)} free. The engine requires at least 1 GiB (1,073,741,824 bytes). <Link className={link} to="/storage?storage=local">Inspect local storage</Link></p>}
      {!empty && summary && failureData && !problems && <p>No attention items recorded in these scopes. This is not a health score or evidence of restore readiness.</p>}
      {readinessIncomplete && <p>Encryption decision incomplete — confirm custody or explicitly defer.</p>}
      {summary && <p className="text-sm text-dark-300">Retained catalog and current process observed <time dateTime={summary.observed_at}>{summary.observed_at}</time>. Retention can remove earlier evidence; this is not lifetime history or a restore guarantee.</p>}
      {summary && <QueryFeedback name="Failed runs in window" query={failures} stale={failureStale} />}
      {failureData && failureData.total > 0 && <div className="space-y-2">
        <h3 className="font-semibold">{failureData.total} failed runs created in the UTC 24-hour window</h3>
        <p className="text-sm">{failureWindow.after} through {failureWindow.before}; created_at, not the time failure occurred.</p>
        {failureData.items.map(run => <div key={run.id}><Link className={link} to={`/history/runs/${run.id}?${failureSearch}`}>Failed run #{run.id}: {run.target_name ?? 'Unknown plan'}</Link></div>)}
        <Link className={link} to={`/history?${failureSearch}`}>View all failed runs in this exact window</Link>
      </div>}
      {summary && summary.recorded_copy_failures.total > 0 && <section aria-label="Recorded copy failures" className="space-y-2">
        <h3 className="font-semibold">Recorded copy failures</h3>
        <p>{summary.recorded_copy_failures.total} failed copy records across {summary.recorded_copy_failures.backups} retained runs</p>
        <p className="text-sm text-dark-300">Historical sync results, not current provider observations. One successful copy is not full coverage.</p>
        {summary.recorded_copy_failures.items.map(copy => <div className="flex flex-wrap gap-x-4" key={`${copy.backup_id}-${copy.storage_id}`}><Link className={link} to={`/history/runs/${copy.backup_id}`}>Run #{copy.backup_id}</Link><Link className={link} to={`/history/archives/${copy.backup_id}`}>Archive context #{copy.backup_id}</Link><Link className={link} to={`/storage?storage=${copy.storage_id}`}>{copy.storage_name ?? 'Unknown or unavailable location'} (#{copy.storage_id})</Link></div>)}
        <p className="text-sm">Currently shown {summary.recorded_copy_failures.items.length} of {summary.recorded_copy_failures.total} copy records.</p>
      </section>}
      {!!noRecordedRun?.total && <div><h3 className="font-semibold">{noRecordedRun.total} plans without a recorded run ({noRecordedRun.enabled} enabled)</h3><p>Not proof that these plans have never run. Open a plan to review and run it.</p>{noRecordedRun.items.map(plan => <Link className={`${link} mr-4`} key={plan.plan_id} to={`/protection/${plan.plan_id}`}>{plan.name}</Link>)}<p className="text-sm">Currently shown {noRecordedRun.items.length} of {noRecordedRun.total} plans.</p></div>}
      {!!noCompletedRun?.total && <div><h3 className="font-semibold">{noCompletedRun.total} plans without a recorded completed run ({noCompletedRun.enabled} enabled)</h3><p>Retained-catalog evidence only. Completed runs do not establish copy availability.</p>{noCompletedRun.items.map(plan => <Link className={`${link} mr-4`} key={plan.plan_id} to={`/protection/${plan.plan_id}`}>Review {plan.name}</Link>)}<p className="text-sm">Currently shown {noCompletedRun.items.length} of {noCompletedRun.total} plans.</p></div>}
      {disk && disk.disk_status !== 'available' && <p>Local capacity unavailable — no measured free-space claim. <Link className={link} to="#overview-capacity">Review capacity and retry</Link></p>}
      <div className="flex flex-wrap gap-x-5"><Link className={link} to="/settings#connection">Check Docker connection{health.isError ? ' — unavailable' : health.data ? ' — healthy response' : ' — unknown'}</Link><Link className={link} to="/settings#encryption">{encryption.isError ? 'Encryption status unavailable' : !encryption.data ? 'Encryption status unknown' : encryption.data.encryption_enabled && encryption.data.setup_completed ? 'Encryption enabled; custody confirmed' : encryption.data.onboarding_deferred ? 'Encryption deliberately deferred' : 'Review encryption and key custody'}</Link></div>
      <QueryFeedback name="Docker connection" query={health} /><QueryFeedback name="Encryption readiness" query={encryption} />
      {empty && <section aria-label="First backup readiness" className="space-y-2">
        <h3 className="font-semibold">Start with your first successful backup</h3>
        <p>No active plans or retained runs. Account creation and saved plans do not create an archive.</p>
        <ol className="list-decimal pl-5 space-y-2"><li><Link className={link} to="/settings#connection">Check connection</Link>: {health.data && !health.isError ? 'Docker responded healthy.' : 'not confirmed.'}</li><li><Link className={link} to="/storage?storage=local">Choose local or remote storage</Link>. Local storage is available as a choice; configured locations are not verified copies.</li><li><Link className={link} to="/settings#encryption">Confirm key custody or deliberately defer encryption</Link>.</li><li><Link className={link} to="/protection/new">Create first backup plan</Link>, then use Run now on its detail page.</li><li><Link className={link} to="/history?view=archives">Inspect the first archive and its contents</Link> after the real run finishes.</li></ol>
      </section>}
    </section>
    <section aria-labelledby="overview-work" className={section}>
      <h2 id="overview-work" className="text-xl font-semibold">Current work</h2>
      {summary && <><p>{summary.backup_activity.live} live backup workers in this process; {summary.backup_activity.recorded_pending} pending and {summary.backup_activity.recorded_running} running catalog records. Union: {summary.backup_activity.total} runs.</p>
        {summary.backup_activity.items.map(run => <div key={run.backup_id} className="py-2"><Link className={link} to={`/history/runs/${run.backup_id}`}>{run.name ?? `Unknown plan — run #${run.backup_id}`}</Link><p>{run.live_worker ? `Live worker · ${run.stage ?? 'stage unknown'}` : 'No live worker observed — recorded status is not proof of active work'}. Catalog: {run.catalog_status ?? 'unknown'}.</p>{run.plan_id && <Link className={link} to={`/protection/${run.plan_id}`}>Plan #{run.plan_id}</Link>}</div>)}
        <p className="text-sm">Currently shown {summary.backup_activity.items.length} of {summary.backup_activity.total} runs. Live observation resets on process restart.</p>
        <p>{summary.storage_activity.total} own queued/running storage operations.</p>{summary.storage_activity.items.map(job => <div key={job.operation_id}><Link className={link} to={`/storage?operation=${job.operation_id}&storage=${job.storage_id}`}>Open operation #{job.operation_id}</Link><p>{job.state} · {job.stage}</p></div>)}<p className="text-sm">Currently shown {summary.storage_activity.items.length} of {summary.storage_activity.total} own operations.</p>
      </>}
      {!summary && <p>Current work unknown until the summary can be confirmed.</p>}
    </section>
    <section aria-labelledby="overview-archives" className={section}>
      <h2 id="overview-archives" className="text-xl font-semibold">Recent archives</h2><QueryFeedback name="Archives" query={archives} />
      {archives.data && <><p>Currently shown {archives.data.items.length} of {archives.data.total} retained archives</p><p className="text-sm">Cataloged artifacts, not attempted runs or guaranteed recoverable copies.</p>{archives.data.items.map(archive => <div key={archive.id} className="py-3 border-b border-dark-700"><Link className={link} to={`/history/archives/${archive.id}`}>{archive.target_name ?? `Archive #${archive.id}`}</Link><p>Run: {archive.status} · Local: {archive.local_state} · Remote availability: unknown</p><p>{archive.file_size_human ?? 'Size unknown'} · {archive.duration_seconds != null ? `${archive.duration_seconds}s` : 'Duration unknown'} · Created {archive.created_at}</p></div>)}{archives.data.total === 0 && <p>No recorded archives yet. Run a plan, then inspect the resulting archive.</p>}</>}
      <Link className={link} to="/history?view=archives">View all archives</Link>
    </section>
    <section aria-labelledby="overview-next" className={section}>
      <h2 id="overview-next" className="text-xl font-semibold">Next registered executions</h2><QueryFeedback name="Registered executions" query={jobs} />
      <p>Registered next times are scheduler observations, not an execution guarantee. Shared schedule definitions alone do not prove a registered job.</p>
      {nextJobs?.slice(0, 5).map(job => <div key={job.id}>{/^backup_target_[1-9]\d*$/.test(job.id) ? <Link className={link} to={`/protection/${job.id.slice('backup_target_'.length)}`}>{job.name}</Link> : <p>{job.name} (system job)</p>}<p>{job.next_run}</p></div>)}
      {nextJobs?.length === 0 && !jobs.isError && <p>No registered next executions.</p>}
      <QueryFeedback name="Schedules" query={schedules} />
      <p>{schedules.data ? `${schedules.data.filter(s => s.enabled).length} enabled of ${schedules.data.length} schedule definitions` : 'Schedule count unknown'}</p>
      {schedules.data?.map(schedule => <div key={schedule.id}><Link className={link} to={`/protection/schedules/${schedule.id}`}>{schedule.name}</Link><p>{schedule.cron_expression} · {schedule.enabled ? 'enabled' : 'paused'}</p></div>)}
      <Link className={link} to="/protection/schedules">Manage schedules</Link>
    </section>
    <section aria-labelledby="overview-context" className={section}>
      <h2 id="overview-context" className="text-xl font-semibold">Resource and plan context</h2>
      <p>Inventory and configured scope, not backed-up coverage. Empty selected volumes means all matching volumes; expansion may be unknown until discovery.</p>
      <dl className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <div><dt>Containers</dt><dd>{containers.data ? `${containers.data.filter(c => c.status === 'running').length}/${containers.data.length}` : 'Unknown'} running / discovered</dd></div>
        <div><dt>Volumes</dt><dd>{volumes.data?.length ?? 'Unknown'}</dd></div><div><dt>Stacks</dt><dd>{stacks.data?.length ?? 'Unknown'}</dd></div>
        <div><dt>Plans (not removed)</dt><dd>{summary ? `${summary.plans.total} total · ${summary.plans.enabled} enabled · ${summary.plans.disabled} disabled · ${summary.plans.manual} manual · ${summary.plans.automatic_enabled} automatic enabled · ${summary.plans.automatic_paused} automatic paused` : 'Unknown'}</dd></div>
        <div><dt>Retained runs</dt><dd>{runs.data?.total ?? 'Unknown'}</dd></div><div><dt>Completed retained runs</dt><dd>{completed.data?.total ?? 'Unknown'} (catalog status, not restore verification)</dd></div>
      </dl>
      <QueryFeedback name="Plans" query={targets} />
      {targets.data?.map(plan => <div key={plan.id}><Link className={link} to={`/protection/${plan.id}`}>{plan.name}</Link><p>Configured {plan.target_type}: {plan.container_name ?? plan.volume_name ?? plan.stack_name ?? (plan.host_path ? 'path configured' : 'identity unknown')} · {plan.enabled ? 'enabled' : 'disabled'}{['container', 'stack'].includes(plan.target_type) ? ` · volumes: ${plan.selected_volumes?.length ? plan.selected_volumes.join(', ') : 'all matching volumes (expansion unknown)'}` : ''}</p></div>)}
      <Link className={link} to="/protection">Manage plans and source scope</Link>
      <QueryFeedback name="Containers" query={containers} /><QueryFeedback name="Volumes" query={volumes} /><QueryFeedback name="Stacks" query={stacks} /><QueryFeedback name="Runs" query={runs} /><QueryFeedback name="Completed runs" query={completed} /><QueryFeedback name="Storage locations" query={storages} />
      <p>{storages.data ? `${storages.data.filter(s => s.enabled).length} enabled of ${storages.data.length} configured remote locations` : 'Remote locations unknown'}. Configuration is not proof of available copies.</p>
    </section>
    <section id="overview-capacity" tabIndex={-1} aria-labelledby="overview-capacity-title" className={section}>
      <h2 id="overview-capacity-title" className="text-xl font-semibold">Local capacity</h2><QueryFeedback name="System information" query={system} />
      <p>Filesystem containing BACKUP_BASE_PATH, in bytes; not remote quota or archive size.</p>
      {disk?.disk_status === 'available' ? <><p>{bytes(disk.disk_used)} used of {bytes(disk.disk_total)} · {bytes(disk.disk_free)} free</p><p>{disk.disk_free.toLocaleString('en-US')} free bytes. Engine minimum: 1 GiB (1,073,741,824 bytes).</p><meter className="w-full" aria-label="Local filesystem bytes used" min={0} max={Math.max(1, disk.disk_total)} value={disk.disk_used} /></> : <p>Local capacity unavailable; usage is unknown, not zero.</p>}
      <Link className={link} to="/storage?storage=local">Inspect local storage</Link>
    </section>
  </div>
}
