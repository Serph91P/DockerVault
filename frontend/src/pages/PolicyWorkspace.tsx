import { useState } from 'react'
import {
  Link,
  useLocation,
  useNavigate,
  useSearchParams,
} from 'react-router-dom'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  schedulesApi,
  retentionApi,
  targetsApi,
  type ScheduleEntity,
  type RetentionPolicy,
  type BackupTarget,
} from '../api'
import { queryKeys } from '../api/queryKeys'
import { ResponseError, responseStatus } from '../api/responses'
import { QueryFeedback } from '../components/QueryFeedback'
import { useValidationFeedback } from '../components/ValidationFeedback'
import {
  ScheduleFields,
  RetentionFields,
  CronPreview,
  type ScheduleDraft,
  type RetentionDraft,
} from '../components/BackupWizard/PolicyFields'
import { retentionDefaults } from '../components/BackupWizard/planEditorModel'
import ConfirmDialog from '../components/ConfirmDialog'
import { Button } from '../components/ui/button'
import { Modal } from '../components/ui/dialog'
import { useNotificationStore } from '../store/notifications'
import { planId, policySearch as filterSearch } from './protectionRoutes'

type Kind = 'schedules' | 'retention'
type Policy = ScheduleEntity | RetentionPolicy
const titles = { schedules: 'Schedules', retention: 'Retention Policies' }
const root = (kind: Kind) => `/protection/${kind}`
const keys = (kind: Kind) =>
  kind === 'schedules' ? queryKeys.schedules : queryKeys.retention
const read = async (kind: Kind, id: number): Promise<Policy> =>
  kind === 'schedules'
    ? (await schedulesApi.get(id)).data
    : (await retentionApi.getPolicy(id)).data
function usageFor(kind: Kind, policy: Policy, targets: BackupTarget[]) {
  return targets.filter((target) =>
    kind === 'schedules'
      ? target.schedule_id === policy.id
      : target.retention_policy_id === policy.id ||
        (policy.name === 'default' &&
          target.retention_policy_id == null &&
          !target.removed_at),
  )
}
function Usage({
  kind,
  policy,
  targets,
  known,
}: {
  kind: Kind
  policy: Policy
  targets: BackupTarget[]
  known: boolean
}) {
  const location = useLocation()
  const used = usageFor(kind, policy, targets)
  const effective = used.filter((target) => !target.removed_at)
  if (!known)
    return (
      <p role="status" className="text-amber-200">
        Usage unknown
      </p>
    )
  return (
    <section className="space-y-2" aria-label="Policy impact">
      <h2 className="font-semibold">Usage and edit impact</h2>
      <p>
        {effective.length}{' '}
        {kind === 'schedules'
          ? 'assigned plans (jobs depend on enabled state)'
          : 'effective plans'}
        ; {used.filter((target) => target.removed_at).length} removed
        references.
      </p>
      {!used.length && <p>Not in use</p>}
      <p>
        Changes affect every listed plan. Disabled plans retain their
        assignment; removed references can still prevent deletion.
      </p>
      <ul className="space-y-2">
        {used.map((target) => (
          <li key={target.id}>
            <Link
              className="text-primary-300 underline break-words"
              to={
                target.removed_at
                  ? '/history'
                  : `/protection/${target.id}?returnTo=${encodeURIComponent(location.pathname + location.search)}`
              }
            >
              {target.name}
            </Link>
            {' — '}
            {target.removed_at
              ? 'Removed plan — reference retained; link opens all retained history'
              : target.enabled
                ? 'Enabled plan'
                : 'Disabled plan'}
            {'; '}
            {kind === 'retention' && target.retention_policy_id == null
              ? 'Default fallback (null assignment)'
              : 'Explicit assignment'}
            {!target.removed_at && (
              <>
                {' '}
                ·{' '}
                <Link
                  className="underline"
                  to={`/protection/${target.id}/edit?returnTo=${encodeURIComponent(location.pathname + location.search)}`}
                >
                  Change assignment for {target.name}
                </Link>
              </>
            )}
          </li>
        ))}
      </ul>
      {kind === 'retention' && (
        <p>
          The policy named default is the fallback for null assignments.
          Clearing a reference does not turn retention off. Renaming default
          changes fallback for all null-assigned plans, including future plans.
          Removed plans are not pruned.
        </p>
      )}
      {kind === 'schedules' && (
        <p>
          Deleting unlinks all assignments, including removed plans. A stored
          legacy cron may then become effective; manual-only requires clearing
          both shared schedule and legacy cron in the plan editor.
        </p>
      )}
    </section>
  )
}

export default function PolicyWorkspace({ kind }: { kind: Kind }) {
  const location = useLocation()
  const [params, setParams] = useSearchParams()
  const navigate = useNavigate()
  const search = filterSearch(params)
  const path = location.pathname.slice(root(kind).length).replace(/^\//, '')
  const parts = path.split('/')
  const id = planId(parts[0])
  const creating = path === 'new'
  const editing = !!id && parts.length === 2 && parts[1] === 'edit'
  const detail = !!id && parts.length === 1
  const targets = useQuery({
    queryKey: queryKeys.targets.list(true),
    queryFn: () => targetsApi.list(true).then((r) => r.data),
  })
  const list = useQuery<Policy[]>({
    queryKey: keys(kind).list(),
    queryFn: async () =>
      kind === 'schedules'
        ? (await schedulesApi.list()).data
        : (await retentionApi.listPolicies()).data,
  })
  const inventory = useQuery({
    queryKey: queryKeys.schedules.inventory(),
    queryFn: () => schedulesApi.cronInventory().then((r) => r.data),
    enabled: kind === 'schedules',
  })
  const known =
    targets.isSuccess &&
    !targets.isError &&
    !targets.isFetching &&
    targets.data.every((target) => {
      const field = kind === 'schedules' ? 'schedule_id' : 'retention_policy_id'
      const assignment = target[field]
      return (
        Object.prototype.hasOwnProperty.call(target, field) &&
        (assignment === null ||
          (typeof assignment === 'number' &&
            Number.isSafeInteger(assignment) &&
            assignment > 0)) &&
        Object.prototype.hasOwnProperty.call(target, 'removed_at') &&
        (target.removed_at === null || typeof target.removed_at === 'string')
      )
    })
  const policies = list.data ?? []
  const visible = policies
    .filter((policy) => {
      if (
        !`${policy.name} ${'description' in policy ? (policy.description ?? '') : ''}`
          .toLowerCase()
          .includes((params.get('q') ?? '').toLowerCase())
      )
        return false
      const status = params.get('status')
      if (status === 'default' && policy.name !== 'default') return false
      if (
        kind === 'schedules' &&
        status &&
        'enabled' in policy &&
        policy.enabled !== (status === 'enabled')
      )
        return false
      if (params.get('usage')) {
        if (!known) return false
        const used = usageFor(kind, policy, targets.data ?? []).length > 0
        if (used !== (params.get('usage') === 'used')) return false
      }
      return true
    })
    .sort((a, b) => a.name.localeCompare(b.name))
  return (
    <div className="space-y-6 min-w-0">
      <header className="flex flex-wrap justify-between gap-3">
        <h1 className="text-2xl font-bold">
          {creating
            ? kind === 'schedules'
              ? 'New Schedule'
              : 'New Retention Policy'
            : editing
              ? 'Edit policy'
              : titles[kind]}
        </h1>
        {!path && (
          <div className="flex flex-wrap gap-2">
            <Button onClick={() => navigate(`${root(kind)}/new${search}`)}>
              {kind === 'schedules' ? 'New Schedule' : 'New Policy'}
            </Button>
            {kind === 'retention' && <Cleanup />}
          </div>
        )}
      </header>
      {path && (
        <Link className="underline" to={`${root(kind)}${search}`}>
          Back to {titles[kind]}
        </Link>
      )}
      <QueryFeedback name="Plan usage" query={targets} />
      {creating ? (
        <PolicyEditor
          key={`${kind}-new`}
          kind={kind}
          cancel={`${root(kind)}${search}`}
          search={search}
          targets={targets.data ?? []}
          known={known}
        />
      ) : editing || detail ? (
        <PolicyResource
          key={`${kind}-${id}-${editing}`}
          kind={kind}
          id={id!}
          editing={editing}
          search={search}
          targets={targets.data ?? []}
          known={known}
        />
      ) : path ? (
        <p role="alert">
          Policy not found. This address is not a valid policy route.
        </p>
      ) : (
        <>
          <div className="flex flex-wrap gap-3" role="search">
            <label className="min-w-0">
              Search policies
              <input
                type="search"
                className="input w-full"
                value={params.get('q') ?? ''}
                onChange={(e) => {
                  const next = new URLSearchParams(search)
                  next.set('q', e.target.value)
                  setParams(next)
                }}
              />
            </label>
            <label>
              Status
              <select
                className="input"
                value={params.get('status') ?? ''}
                onChange={(e) => {
                  const next = new URLSearchParams(search)
                  next.set('status', e.target.value)
                  setParams(next)
                }}
              >
                <option value="">All states</option>
                {kind === 'schedules' ? (
                  <>
                    <option value="enabled">Enabled</option>
                    <option value="disabled">Disabled</option>
                  </>
                ) : (
                  <option value="default">Default</option>
                )}
              </select>
            </label>
            <label>
              Usage
              <select
                className="input"
                value={params.get('usage') ?? ''}
                onChange={(e) => {
                  const next = new URLSearchParams(search)
                  next.set('usage', e.target.value)
                  setParams(next)
                }}
              >
                <option value="">All usage</option>
                <option value="used">Used</option>
                <option value="unused">Unused</option>
              </select>
            </label>
          </div>
          <QueryFeedback
            name={kind === 'retention' ? 'Retention policies' : titles[kind]}
            query={list}
          />
          {list.isPending && <p role="status">Loading policies...</p>}
          {list.isSuccess && policies.length === 0 && (
            <p>
              No {kind === 'schedules' ? 'schedules' : 'retention policies'}{' '}
              configured.
            </p>
          )}
          {list.isSuccess && policies.length > 0 && visible.length === 0 && (
            <p>
              {params.get('usage') && !known
                ? 'Usage filter unavailable until usage is known.'
                : 'No matching policies.'}
            </p>
          )}
          <div className="grid gap-4 md:grid-cols-2">
            {visible.map((policy) => (
              <article className="card p-4 space-y-3 min-w-0" key={policy.id}>
                <h2 className="text-lg font-semibold break-words">
                  <Link
                    className="underline"
                    to={`${root(kind)}/${policy.id}${search}`}
                  >
                    {policy.name}
                  </Link>
                </h2>
                <PolicySummary policy={policy} />
                <Usage
                  kind={kind}
                  policy={policy}
                  targets={targets.data ?? []}
                  known={known}
                />
                <div className="flex flex-wrap gap-3">
                  <Link
                    className="underline"
                    aria-label={
                      kind === 'retention'
                        ? `Edit policy ${policy.name}`
                        : `Edit schedule ${policy.name}`
                    }
                    to={`${root(kind)}/${policy.id}/edit${search}`}
                  >
                    Edit
                  </Link>
                  <PolicyActions
                    kind={kind}
                    policy={policy}
                    targets={targets.data ?? []}
                    known={known}
                    search={search}
                  />
                </div>
              </article>
            ))}
          </div>
          {kind === 'schedules' && (
            <>
              <QueryFeedback name="Schedule compatibility" query={inventory} />
              {!!inventory.data?.items.length && (
                <section role="alert">
                  <h2>Stored schedule compatibility - review intent</h2>
                  <p>
                    {inventory.data.contract} Timezone:{' '}
                    {inventory.data.timezone}
                  </p>
                  {inventory.data.items.map((item) => (
                    <div key={`${item.kind}-${item.id}`}>
                      <p>
                        {item.kind}: {item.name} — {item.expression}
                      </p>
                      {item.warnings.map((warning) => (
                        <p key={warning}>{warning}</p>
                      ))}
                    </div>
                  ))}
                </section>
              )}
              <CronHelp />
            </>
          )}
        </>
      )}
    </div>
  )
}
function PolicySummary({ policy }: { policy: Policy }) {
  return 'cron_expression' in policy ? (
    <div className="space-y-2">
      <p>{policy.description}</p>
      <p>{policy.enabled ? 'Enabled' : 'Disabled'}</p>
      <p className="break-all">Cron: {policy.cron_expression}</p>
      <p>
        Calculated next run:{' '}
        {policy.next_run
          ? new Date(policy.next_run).toLocaleString()
          : 'Not scheduled'}
        . Actual jobs are shown separately.
      </p>
    </div>
  ) : (
    <div>
      <p>
        Keep Last: {policy.keep_last}; GFS daily / weekly / monthly / yearly:{' '}
        {policy.keep_daily} / {policy.keep_weekly} / {policy.keep_monthly} /{' '}
        {policy.keep_yearly}
      </p>
      <p>
        Bucket age cutoff: {policy.max_age_days} days. Keep Last independently
        protects newest backups. GFS buckets overlap; this is a rule
        explanation, not a destructive dry run.
      </p>
      {policy.name === 'default' && <p>Default policy (name-based fallback)</p>}
    </div>
  )
}
function PolicyResource({
  kind,
  id,
  editing,
  search,
  targets,
  known,
}: {
  kind: Kind
  id: number
  editing: boolean
  search: string
  targets: BackupTarget[]
  known: boolean
}) {
  const query = useQuery({
    queryKey: [...keys(kind).all, 'detail', id],
    queryFn: () => read(kind, id),
    refetchOnMount: 'always',
    retry: false,
  })
  const [baseline, setBaseline] = useState<Policy | null>(null)
  if (
    editing &&
    !baseline &&
    query.isFetchedAfterMount &&
    query.isSuccess &&
    !query.isFetching
  )
    setBaseline(query.data)
  // Only the initial fresh read controls mounting; later refetches must not destroy dirty inputs.
  if (editing && baseline)
    return (
      <>
        <Usage kind={kind} policy={baseline} targets={targets} known={known} />
        <PolicyEditor
          kind={kind}
          initial={baseline}
          cancel={`${root(kind)}/${id}${search}`}
          search={search}
          targets={targets}
          known={known}
        />
      </>
    )
  if (query.isError)
    return (
      <>
        <QueryFeedback name="Policy detail" query={query} />
        {responseStatus(query.error) === 404 && (
          <p role="alert">Policy not found.</p>
        )}
      </>
    )
  if (!query.data || (editing && !baseline))
    return <p role="status">Loading fresh policy...</p>
  return (
    <section className="space-y-4">
      <h2 className="text-xl font-semibold break-words">{query.data.name}</h2>
      <PolicySummary policy={query.data} />
      <Usage kind={kind} policy={query.data} targets={targets} known={known} />
      <div className="flex flex-wrap gap-3">
        <Link className="underline" to={`${root(kind)}/${id}/edit${search}`}>
          Edit
        </Link>
        <PolicyActions
          kind={kind}
          policy={query.data}
          targets={targets}
          known={known}
          search={search}
        />
      </div>
      {kind === 'schedules' && 'cron_expression' in query.data && (
        <>
          <CronPreview expression={query.data.cron_expression} />
          <Jobs />
          <CronHelp />
        </>
      )}
    </section>
  )
}
function retentionDraft(policy: RetentionPolicy): RetentionDraft {
  return {
    name: policy.name,
    keepLast: policy.keep_last,
    keepDaily: policy.keep_daily,
    keepWeekly: policy.keep_weekly,
    keepMonthly: policy.keep_monthly,
    keepYearly: policy.keep_yearly,
    maxAgeDays: policy.max_age_days,
  }
}
function PolicyEditor({
  kind,
  initial,
  cancel,
  search,
  targets,
  known,
}: {
  kind: Kind
  initial?: Policy
  cancel: string
  search: string
  targets: BackupTarget[]
  known: boolean
}) {
  const navigate = useNavigate()
  const client = useQueryClient()
  const [schedule, setSchedule] = useState<ScheduleDraft>(() =>
    initial && 'cron_expression' in initial
      ? {
          name: initial.name,
          cronExpression: initial.cron_expression,
          description: initial.description ?? '',
          enabled: initial.enabled,
        }
      : {
          name: '',
          cronExpression: '0 2 * * *',
          description: '',
          enabled: true,
        },
  )
  const [retention, setRetention] = useState<RetentionDraft>(() =>
    initial && 'keep_last' in initial
      ? retentionDraft(initial)
      : { ...retentionDefaults },
  )
  const [localError, setLocalError] = useState<unknown>(null)
  const [uncertain, setUncertain] = useState(false)
  const [savedId, setSavedId] = useState<number | null>(null)
  const mutation = useMutation({
    mutationFn: async () => {
      const payload =
        kind === 'schedules'
          ? {
              name: schedule.name,
              cron_expression: schedule.cronExpression,
              description: schedule.description ?? '',
              enabled: schedule.enabled !== false,
            }
          : {
              name: retention.name,
              keep_last: retention.keepLast,
              keep_daily: retention.keepDaily,
              keep_weekly: retention.keepWeekly,
              keep_monthly: retention.keepMonthly,
              keep_yearly: retention.keepYearly,
              max_age_days: retention.maxAgeDays,
            }
      if (kind === 'schedules') {
        try {
          await schedulesApi.preview(schedule.cronExpression)
        } catch (error) {
          if (responseStatus(error) === 400)
            throw new ResponseError(422, {
              cron_expression:
                'Invalid cron expression. Correct it and check the server preview.',
            })
          throw error
        }
      }
      let policy: Policy
      try {
        policy =
          kind === 'schedules'
            ? (
                await (initial
                  ? schedulesApi.update(initial.id, payload)
                  : schedulesApi.create(
                      payload as Parameters<typeof schedulesApi.create>[0],
                    ))
              ).data
            : (
                await (initial
                  ? retentionApi.updatePolicy(initial.id, payload)
                  : retentionApi.createPolicy(payload))
              ).data
      } catch (error) {
        const status = responseStatus(error)
        if (status === undefined || status >= 500) setUncertain(true)
        throw error
      }
      setSavedId(policy.id)
      setUncertain(true) // A successful write followed by a failed read must not be replayed.
      const fresh = await read(kind, policy.id)
      if (
        !Object.entries(payload).every(
          ([key, value]) =>
            (fresh as unknown as Record<string, unknown>)[key] === value,
        )
      )
        throw new ResponseError()
      await client.invalidateQueries({ queryKey: keys(kind).all })
      await client.invalidateQueries({ queryKey: queryKeys.targets.all })
      if (kind === 'schedules')
        await client.fetchQuery({
          queryKey: queryKeys.schedules.jobs(),
          queryFn: () => schedulesApi.getJobs().then((r) => r.data),
          staleTime: 0,
        })
      navigate(`${root(kind)}/${fresh.id}${search}`)
    },
  })
  const validation = useValidationFeedback(localError || mutation.error)
  const reconcile = useMutation({
    mutationFn: async () => {
      if (kind === 'schedules') {
        await schedulesApi.reload()
        await schedulesApi.getJobs()
      }
      await client.invalidateQueries({ queryKey: keys(kind).all })
      await client.invalidateQueries({ queryKey: queryKeys.targets.all })
      // Unknown creates are never attributed by a non-unique intent/name match.
      navigate(
        savedId || initial
          ? `${root(kind)}/${savedId ?? initial!.id}${search}`
          : `${root(kind)}${search}`,
      )
    },
  })
  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (mutation.isPending || uncertain) return
    const fields: Record<string, string> = {}
    if (!(kind === 'schedules' ? schedule.name : retention.name).trim())
      fields.name = 'Enter a policy name.'
    if (kind === 'schedules' && !schedule.cronExpression.trim())
      fields.cron_expression = 'Enter a cron expression.'
    if (kind === 'retention')
      for (const [key, value] of Object.entries({
        keep_last: retention.keepLast,
        keep_daily: retention.keepDaily,
        keep_weekly: retention.keepWeekly,
        keep_monthly: retention.keepMonthly,
        keep_yearly: retention.keepYearly,
        max_age_days: retention.maxAgeDays,
      })) {
        const min = key === 'keep_last' || key === 'max_age_days' ? 1 : 0
        const max = key === 'max_age_days' ? 36500 : 1000
        if (!Number.isInteger(value) || value < min || value > max)
          fields[key] = `Enter a whole number from ${min} to ${max}.`
      }
    setLocalError(
      Object.keys(fields).length ? new ResponseError(422, fields) : null,
    )
    if (Object.keys(fields).length) {
      queueMicrotask(() =>
        document.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus(),
      )
      return
    }
    mutation.mutate(undefined, {
      onError: () =>
        queueMicrotask(() =>
          document.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus(),
        ),
    })
  }
  return (
    <form noValidate onSubmit={submit} className="space-y-4 max-w-3xl">
      {validation.summary}
      {kind === 'retention' &&
        retention.name !== initial?.name &&
        (retention.name === 'default' || initial?.name === 'default') && (
          <section className="space-y-2">
            <h2>Proposed fallback impact</h2>
            <p>
              {retention.name === 'default'
                ? 'Null-assigned active plans will inherit this rule.'
                : 'Null-assigned active plans will no longer find the default policy by this name.'}{' '}
              This also affects future null-assigned plans.
            </p>
            {!known ? (
              <p>Fallback impact unknown</p>
            ) : (
              <ul>
                {targets
                  .filter(
                    (target) =>
                      !target.removed_at && target.retention_policy_id === null,
                  )
                  .map((target) => (
                    <li key={target.id}>
                      <Link
                        className="underline"
                        to={`/protection/${target.id}`}
                      >
                        {target.name}
                      </Link>
                    </li>
                  ))}
              </ul>
            )}
          </section>
        )}
      {kind === 'schedules' ? (
        <ScheduleFields
          legend={initial ? 'Edit shared schedule' : 'New shared schedule'}
          value={schedule}
          onChange={setSchedule}
          feedback={validation.field}
        />
      ) : (
        <RetentionFields
          legend={
            initial
              ? 'Edit shared retention policy'
              : 'New shared retention policy'
          }
          value={retention}
          onChange={setRetention}
          feedback={validation.field}
        />
      )}
      {uncertain && (
        <section role="alert">
          <p>
            Write outcome requires verification. Do not submit again. Inspect
            persisted policies and assignments; a scheduler reconciliation
            failure can occur after commit. Unknown creates cannot safely be
            identified by name alone.
          </p>
          <Button
            type="button"
            disabled={reconcile.isPending}
            onClick={() => reconcile.mutate()}
          >
            Reload and inspect server state
          </Button>
          {reconcile.isError && (
            <p>Readback/reload failed. Your input is retained.</p>
          )}
        </section>
      )}
      <div className="flex flex-wrap gap-3">
        <Button type="submit" disabled={mutation.isPending || uncertain}>
          {mutation.isPending ? 'Saving...' : 'Save'}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={mutation.isPending}
          onClick={() => navigate(cancel)}
        >
          Cancel
        </Button>
      </div>
    </form>
  )
}
function Jobs() {
  const query = useQuery({
    queryKey: queryKeys.schedules.jobs(),
    queryFn: () => schedulesApi.getJobs().then((r) => r.data),
  })
  return (
    <section>
      <h2>Actual scheduler jobs</h2>
      <QueryFeedback name="Scheduler jobs" query={query} />
      {query.isSuccess && (
        <ul>
          {query.data.map((job) => (
            <li key={job.id}>
              {job.name}: {job.next_run ?? 'No next run'}
            </li>
          ))}
        </ul>
      )}
      <p>Job inventory is server state, not proof of completed backups.</p>
    </section>
  )
}
function CronHelp() {
  const query = useQuery({
    queryKey: ['cron-help'],
    queryFn: () => schedulesApi.getCronHelp().then((r) => r.data),
  })
  return (
    <details>
      <summary>Cron Help</summary>
      <QueryFeedback name="Cron help" query={query} />
      {query.data && (
        <>
          <p>
            {query.data.format}. {query.data.contract} Timezone:{' '}
            {query.data.timezone}.
          </p>
          {query.data.examples?.map(
            (example: { expression: string; description: string }) => (
              <p key={example.expression}>
                {example.expression} — {example.description}
              </p>
            ),
          )}
        </>
      )}
    </details>
  )
}
function PolicyActions({
  kind,
  policy,
  targets,
  known,
  search,
}: {
  kind: Kind
  policy: Policy
  targets: BackupTarget[]
  known: boolean
  search: string
}) {
  const client = useQueryClient()
  const navigate = useNavigate()
  const [confirm, setConfirm] = useState(false)
  const [result, setResult] = useState('')
  const used = usageFor(kind, policy, targets)
  const blocked =
    !known ||
    (kind === 'retention' && (policy.name === 'default' || used.length > 0))
  const refresh = async () => {
    await client.invalidateQueries({ queryKey: keys(kind).all })
    await client.invalidateQueries({ queryKey: queryKeys.targets.all })
    if (kind === 'schedules')
      await client.fetchQuery({
        queryKey: queryKeys.schedules.jobs(),
        queryFn: () => schedulesApi.getJobs().then((r) => r.data),
        staleTime: 0,
      })
  }
  const deletion = useMutation({
    mutationFn: async () => {
      const response =
        kind === 'schedules'
          ? await schedulesApi.delete(policy.id)
          : await retentionApi.deletePolicy(policy.id)
      if (response.data?.status !== 'deleted') throw new ResponseError()
      try {
        await read(kind, policy.id)
      } catch (error) {
        if (responseStatus(error) !== 404) throw error
      }
      // Confirm absence and every unlink from a complete fresh inventory, including removed FKs.
      const list =
        kind === 'schedules'
          ? (await schedulesApi.list()).data
          : (await retentionApi.listPolicies()).data
      if (list.some((row) => row.id === policy.id)) throw new ResponseError()
      const inventory = (await targetsApi.list(true)).data
      if (
        kind === 'schedules' &&
        inventory.some((target) => target.schedule_id === policy.id)
      )
        throw new ResponseError()
      await refresh()
      setConfirm(false)
      navigate(`${root(kind)}${search}`)
    },
  })
  const toggle = useMutation({
    mutationFn: async () => {
      if (!('enabled' in policy)) return
      await schedulesApi.update(policy.id, { enabled: !policy.enabled })
      const fresh = await schedulesApi.get(policy.id)
      if (fresh.data.enabled !== !policy.enabled) throw new ResponseError()
      await refresh()
    },
  })
  const run = useMutation({
    mutationFn: async () => {
      const { data } = await schedulesApi.triggerAll(policy.id)
      if (
        data?.status !== 'triggered' ||
        !Array.isArray(data.triggered_targets) ||
        !Array.isArray(data.skipped_targets)
      )
        throw new ResponseError()
      setResult(
        `Accepted ${data.triggered_targets.length} runs, skipped ${data.skipped_targets.length}. Completion is not confirmed.`,
      )
      await client.invalidateQueries({ queryKey: queryKeys.backups.all })
    },
  })
  const reload = useMutation({
    mutationFn: async () => {
      await schedulesApi.reload()
      await refresh()
      setResult(
        'Scheduler reloaded. Inspect persisted state and jobs before retrying any action.',
      )
    },
  })
  return (
    <>
      {'enabled' in policy && (
        <>
          <Button
            variant="outline"
            disabled={toggle.isPending}
            aria-label={`${policy.enabled ? 'Disable' : 'Enable'} schedule ${policy.name}`}
            onClick={() => toggle.mutate()}
          >
            {policy.enabled ? 'Disable' : 'Enable'}
          </Button>
          <Button
            variant="outline"
            disabled={!policy.enabled || run.isPending || run.isError}
            onClick={() => run.mutate()}
          >
            Run All
          </Button>
          <Link className="underline" to="/history">
            Inspect runs
          </Link>
        </>
      )}
      <Button
        variant="destructive"
        aria-label={
          kind === 'schedules'
            ? `Delete schedule ${policy.name}`
            : `Delete policy ${policy.name}`
        }
        disabled={blocked || deletion.isPending}
        onClick={() => setConfirm(true)}
      >
        Delete
      </Button>
      {blocked && (
        <p>
          {!known
            ? 'Deletion blocked: usage unknown.'
            : 'Deletion blocked: default policy or retained plan references. Change active assignments in their plan editors first; removed references require lifecycle resolution.'}
        </p>
      )}
      {(toggle.isError || run.isError || deletion.isError) && (
        <p role="alert">
          Action result not confirmed. Inspect current server state before
          repeating.
        </p>
      )}
      {kind === 'schedules' && (toggle.isError || deletion.isError) && (
        <Button disabled={reload.isPending} onClick={() => reload.mutate()}>
          Reload scheduler and read back
        </Button>
      )}
      {reload.isError && <p role="alert">Scheduler reload/readback failed.</p>}
      {result && <p role="status">{result}</p>}
      <ConfirmDialog
        isOpen={confirm}
        title={
          kind === 'schedules' ? 'Delete Schedule' : 'Delete Retention Policy'
        }
        message={`Delete ${policy.name}? ${used.length} retained references. ${kind === 'schedules' ? 'All will be unlinked; legacy cron can become effective.' : 'Only an unassigned non-default policy can be deleted.'}`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deletion.isPending}
        onConfirm={() => deletion.mutate()}
        onClose={() => {
          setConfirm(false)
          deletion.reset()
        }}
      >
        {deletion.isError && (
          <p role="alert">
            Failed to verify deletion. Inspect server state before retrying.
          </p>
        )}
      </ConfirmDialog>
    </>
  )
}
function Cleanup() {
  const [open, setOpen] = useState(false)
  const client = useQueryClient()
  const mutation = useMutation({
    mutationFn: async () => {
      const { data } = await retentionApi.cleanupOrphaned()
      await client.invalidateQueries({ queryKey: queryKeys.backups.all })
      await client.invalidateQueries({ queryKey: ['storage-files'] })
      await client.invalidateQueries({ queryKey: ['storage-size'] })
      return data
    },
    onSuccess: (data) => {
      const phase =
        data.outcome === 'empty' || data.outcome === 'success'
          ? 'completed'
          : data.outcome === 'failure'
            ? 'failed'
            : 'partial'
      useNotificationStore.getState().addNotification({
        type:
          phase === 'completed'
            ? 'success'
            : phase === 'failed'
              ? 'error'
              : 'warning',
        phase,
        title: 'Cleanup result',
        message: `Backend reports ${data.deleted} deleted files; ${data.failed} failed. Overall cleanup: ${data.outcome}. UI-session observation only.`,
      })
    },
    onError: () => {
      useNotificationStore.getState().addNotification({
        type: 'warning',
        phase: 'unknown',
        title: 'Cleanup result',
        message: 'Cleanup result unknown. Inspect files before repeating.',
      })
    },
  })
  return (
    <>
      <Button
        variant="outline"
        onClick={() => {
          mutation.reset()
          setOpen(true)
        }}
      >
        Cleanup
      </Button>
      <Modal
        open={open}
        title="Cleanup Orphaned Files"
        pending={mutation.isPending}
        onClose={() => setOpen(false)}
      >
        <div className="p-6 space-y-4">
          <h2>Cleanup Orphaned Files</h2>
          <p>
            Global backup-base directory scan for *.tar* files without catalog
            references. Cataloged and active artifacts are protected. This is
            not retention pruning, is not limited to a selected policy, and has
            no file dry run. Deletions cannot be undone.
          </p>
          {mutation.isPending && (
            <p role="status">Scanning and settling file deletions...</p>
          )}
          {mutation.isError && (
            <p role="alert">
              Cleanup failed or result unknown. Inspect files before another
              cleanup; no automatic retry.
            </p>
          )}
          {mutation.data && (
            <section
              role="status"
              tabIndex={0}
              ref={(node) => {
                node?.focus()
              }}
            >
              <h3>Cleanup result: {mutation.data.outcome}</h3>
              <p>
                {mutation.data.deleted} deleted; {mutation.data.failed} failed;{' '}
                {mutation.data.protected} protected; {mutation.data.freed_human}{' '}
                freed.
              </p>
              <p>
                Scan {mutation.data.scan_complete ? 'complete' : 'incomplete'};{' '}
                {mutation.data.scan_errors} scan errors. This is the backend
                result of this request, not a preview of another cleanup.
              </p>
            </section>
          )}
          <div className="flex flex-wrap gap-3">
            {!mutation.data && !mutation.isError && (
              <Button
                variant="destructive"
                disabled={mutation.isPending}
                onClick={() => mutation.mutate()}
              >
                {mutation.isPending ? 'Processing...' : 'Cleanup'}
              </Button>
            )}
            <Button
              variant="outline"
              disabled={mutation.isPending}
              onClick={() => setOpen(false)}
            >
              {mutation.data || mutation.isError ? 'Close' : 'Cancel'}
            </Button>
          </div>
        </div>
      </Modal>
    </>
  )
}
