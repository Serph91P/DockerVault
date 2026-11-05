import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import api, { dockerApi, targetsApi, schedulesApi, retentionApi, storageApi, type BackupTarget } from '../../api'
import type { PrivateTargetConfig } from '../../api/targetPrivacy'
import { responseStatus, ResponseError } from '../../api/responses'
import { queryKeys } from '../../api/queryKeys'
import { useAuthStore } from '../../store/auth'
import { sessionSignal } from '../../store/session'
import { useNotificationStore } from '../../store/notifications'
import { usePrivateTargetEditor } from '../usePrivateTargetEditor'
import { QueryFeedback } from '../QueryFeedback'
import { useValidationFeedback } from '../ValidationFeedback'
import { PageHeader } from '../ui/workspace'
import StepTargetSelect from './StepTargetSelect'
import StepVolumeConfig from './StepVolumeConfig'
import StepDependencies from './StepDependencies'
import StepSchedule from './StepSchedule'
import StepRetention from './StepRetention'
import StepStorage from './StepStorage'
import StepOptions from './StepOptions'
import StepSummary from './StepSummary'
import {
  initialData,
  targetToWizardData,
  changeSource,
  serializeCreate,
  serializeUpdate,
  validateEditor,
  topologyErrors,
  availableVolumes,
  type WizardData,
} from './planEditorModel'
import { DraftWriter, readDraft } from './planEditorDraft'
export type { WizardData } from './planEditorModel'

interface Props {
  isOpen: boolean
  onClose: () => void
  onSaved?: (id: number) => void
  editTarget?: BackupTarget | null
  preselectedType?: WizardData['targetType']
  preselectedTarget?: string | null
}
const GROUPS = ['Source & scope', 'Strategy', 'Execution & review']
const same = (a: unknown, b: unknown): boolean => {
  if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((v, i) => same(v, b[i]))
  if (a && typeof a === 'object')
    return (
      !!b &&
      typeof b === 'object' &&
      Object.keys(a).length === Object.keys(b).length &&
      Object.entries(a).every(([key, value]) => same(value, (b as Record<string, unknown>)[key]))
    )
  return a === b
}
export default function BackupWizard(props: Props) {
  const owner = useAuthStore((s) => s.user?.id)
  return props.isOpen ? <PlanEditor key={`${owner}:${props.editTarget?.id ?? 'new'}`} {...props} /> : null
}

function PlanEditor({ onClose, onSaved, editTarget, preselectedType, preselectedTarget }: Props) {
  const owner = useAuthStore((s) => s.user?.id)
  const intent = editTarget ? `target:${editTarget.id}` : 'new'
  const [draft, setDraft] = useState(() => (owner ? readDraft(owner, intent) : null))
  const [baseline, setBaseline] = useState(editTarget?.updated_at ?? draft?.revision ?? undefined)
  const writer = useMemo(
    () => (owner ? new DraftWriter(owner, intent, baseline ?? null) : null),
    [owner, intent, baseline]
  )
  const [data, setData] = useState<WizardData>(() => {
    if (editTarget) return targetToWizardData(editTarget)
    if (!preselectedType || !preselectedTarget) return initialData
    return {
      ...initialData,
      targetType: preselectedType,
      targetName: `${preselectedTarget} Backup`,
      ...{
        container: { containerName: preselectedTarget },
        stack: { stackName: preselectedTarget },
        volume: { volumeName: preselectedTarget },
        path: { hostPath: preselectedTarget },
      }[preselectedType],
    }
  })
  const [group, setGroup] = useState(0)
  const [touched, setTouched] = useState(false)
  const [errors, setErrors] = useState<string[]>([])

  const [conflict, setConflict] = useState<BackupTarget | null>(null)
  const [candidates, setCandidates] = useState<Array<{ id: number; label: string }>>([])
  const [candidateError, setCandidateError] = useState(false)
  const [reconciled, setReconciled] = useState(false)
  const finished = useRef(false)
  const errorFocus = useRef<HTMLDivElement>(null)
  const editor = usePrivateTargetEditor(editTarget?.id)
  const validation = useValidationFeedback(editor.saveFailure)
  const client = useQueryClient()
  const targets = useQuery({ queryKey: queryKeys.targets.list(), queryFn: () => targetsApi.list().then((r) => r.data) })
  const containers = useQuery({
    queryKey: queryKeys.containers.list(),
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
  })
  const volumes = useQuery({
    queryKey: queryKeys.volumes.list(),
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
  })
  const stacks = useQuery({
    queryKey: queryKeys.stacks.list(),
    queryFn: () => dockerApi.listStacks().then((r) => r.data),
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

  useEffect(() => {
    if (touched && !draft && !finished.current) writer?.queue(data, group)
    return () => writer?.cancel()
  }, [data, group, touched, draft, writer])
  useEffect(() => {
    const signal = sessionSignal()
    const revoke = () => {
      finished.current = true
      writer?.clear()
    }
    signal.addEventListener('abort', revoke, { once: true })
    return () => {
      writer?.cancel()
      signal.removeEventListener('abort', revoke)
    }
  }, [writer])
  useEffect(() => {
    if (errors.length || editor.saveError) errorFocus.current?.focus()
  }, [errors, editor.saveError])

  const updateData = (updates: Partial<WizardData>) => {
    setTouched(true)
    setData((prev) =>
      ['targetType', 'containerName', 'stackName', 'volumeName', 'hostPath'].some(
        (key) => key in updates && updates[key as keyof WizardData] !== prev[key as keyof WizardData]
      )
        ? changeSource(prev, updates)
        : { ...prev, ...updates }
    )
  }
  const checkpoint = (next: WizardData, revision = baseline) => {
    if (finished.current) return
    setData(next)
    setTouched(true)
    writer?.checkpoint(next, group, revision)
  }
  const cancel = () => {
    writer?.cancel()
    editor.close()
    onClose()
  }
  const discard = () => {
    writer?.clear()
    editor.retry()
    setErrors([])
    setDraft(null)
    setTouched(false)
    setData(editTarget ? targetToWizardData(editTarget) : initialData)
    setGroup(0)
  }
  const restore = () => {
    if (!draft) return
    if (editTarget && draft.revision !== editTarget.updated_at) {
      setConflict(editTarget)
      return
    }
    // Immutable source identity comes from the freshly loaded target, never the draft.
    const source = editTarget
      ? {
          targetType: editTarget.target_type,
          containerName: editTarget.container_name || '',
          stackName: editTarget.stack_name || '',
          volumeName: editTarget.volume_name || '',
          hostPath: editTarget.host_path || '',
        }
      : {}
    setData({ ...draft.data, ...source })
    setGroup(draft.group)
    setDraft(null)
    setTouched(true)
  }
  const readCandidates = async () => {
    setCandidateError(false)
    setCandidates([])
    try {
      const kind = data.unconfirmed
      if (kind === 'target') {
        const rows = (await targetsApi.list()).data
        setCandidates(
          rows
            .filter((t) => t.name === data.targetName)
            .map((t) => ({
              id: t.id,
              label: `${t.name} (#${t.id}) — ${t.target_type}; ${t.container_name || t.stack_name || t.volume_name || t.host_path}; updated ${t.updated_at || 'unknown'}`,
            }))
        )
      } else if (kind === 'schedule') {
        const rows = (await schedulesApi.list()).data
        setCandidates(
          rows
            .filter((s) => s.name === data.newSchedule?.name)
            .map((s) => ({
              id: s.id,
              label: `${s.name} (#${s.id}): ${s.cron_expression}; ${s.description || ''}; enabled ${s.enabled}`,
            }))
        )
      } else if (kind === 'retention') {
        const rows = (await retentionApi.listPolicies()).data
        setCandidates(
          rows
            .filter((p) => p.name === data.newRetentionPolicy?.name)
            .map((p) => ({
              id: p.id,
              label: `${p.name} (#${p.id}): last ${p.keep_last}, daily ${p.keep_daily}, weekly ${p.keep_weekly}, monthly ${p.keep_monthly}, yearly ${p.keep_yearly}, age ${p.max_age_days}`,
            }))
        )
      }
    } catch {
      setCandidateError(true)
    }
  }
  const chooseCandidate = async (id: number) => {
    await editor.run(
      async (signal, current) => {
        const kind = data.unconfirmed
        if (kind === 'target') {
          await api.get(`/targets/${id}`, { signal })
          if (!current()) return
          // Explicit user choice opens the canonical detail, never silently overwrites a candidate.
          finished.current = true
          writer?.clear()
          onSaved?.(id)
          if (!onSaved) onClose()
        } else {
          await api.get(`/${kind === 'schedule' ? 'schedules' : 'retention'}/${id}`, { signal })
          if (current())
            checkpoint({
              ...data,
              unconfirmed: null,
              ...(kind === 'schedule'
                ? { scheduleId: id, scheduleMode: 'shared', newSchedule: null }
                : { retentionPolicyId: id, newRetentionPolicy: null }),
            })
        }
      },
      () => {
        void client.invalidateQueries({ queryKey: queryKeys.schedules.all })
        void client.invalidateQueries({ queryKey: queryKeys.retention.all })
      }
    )
  }

  const reloadScheduler = () =>
    editor.run(
      async (signal, current) => {
        await api.post('/schedules/reload', undefined, { signal })
        await api.get('/schedules/jobs', { signal })
        if (current()) setReconciled(true)
      },
      () => {
        void client.invalidateQueries({ queryKey: queryKeys.schedules.all })
      }
    )

  const save = async () => {
    const invalid = validateEditor(data)
    if (data.unconfirmed) invalid.push('Resolve unconfirmed creation before saving. No automatic replay is safe.')
    if (!owner) invalid.push('Confirmed account identity is required before saving.')
    setErrors(invalid)
    if (invalid.length || finished.current) return
    let savedId = editTarget?.id ?? data.confirmedTargetId ?? undefined
    let revision = 0
    await editor.run(
      async (signal, current) => {
        let working = data
        // Refresh only relevant discovery. Failed discovery is unknown, never empty/all.
        const c = data.targetType === 'container' || data.dependencies.length ? await containers.refetch() : containers
        const s = data.targetType === 'stack' ? await stacks.refetch() : stacks
        const v = data.targetType === 'volume' ? await volumes.refetch() : volumes
        if (
          ((data.targetType === 'container' || data.dependencies.length) && c.isError) ||
          (data.targetType === 'stack' && s.isError) ||
          (data.targetType === 'volume' && v.isError)
        )
          throw new ResponseError()
        const scopeErrors = topologyErrors(data, c.data || [], s.data || [], v.data || [])
        if (scopeErrors.length) {
          setErrors(scopeErrors)
          throw new ResponseError()
        }
        if (savedId && baseline) {
          const latest = (await api.get<BackupTarget>(`/targets/${savedId}`, { signal })).data
          if (!current()) return
          if (latest.updated_at !== baseline) {
            setConflict(latest)
            throw new ResponseError()
          }
        }
        const post = async (kind: 'schedule' | 'retention' | 'target', payload: unknown) => {
          working = { ...working, unconfirmed: kind }
          checkpoint(working)
          try {
            return await api.post(
              kind === 'schedule' ? '/schedules' : kind === 'retention' ? '/retention' : '/targets',
              payload,
              { signal }
            )
          } catch (error) {
            if (current()) {
              const status = responseStatus(error)
              if (status !== undefined && status >= 400 && status < 500) {
                working = { ...working, unconfirmed: null }
                checkpoint(working)
              }
            }
            throw error
          }
        }
        if (working.newSchedule) {
          const p = working.newSchedule
          const response = await post('schedule', {
            name: p.name,
            cron_expression: p.cronExpression,
            description: p.description,
            enabled: p.enabled !== false,
          })
          if (!current()) return
          working = {
            ...working,
            scheduleId: response.data.id,
            scheduleMode: 'shared',
            newSchedule: null,
            unconfirmed: null,
          }
          checkpoint(working)
        }
        if (working.newRetentionPolicy) {
          const p = working.newRetentionPolicy
          const response = await post('retention', {
            name: p.name,
            keep_last: p.keepLast,
            keep_daily: p.keepDaily,
            keep_weekly: p.keepWeekly,
            keep_monthly: p.keepMonthly,
            keep_yearly: p.keepYearly,
            max_age_days: p.maxAgeDays,
          })
          if (!current()) return
          working = { ...working, retentionPolicyId: response.data.id, newRetentionPolicy: null, unconfirmed: null }
          checkpoint(working)
        }
        const safePayload = savedId ? serializeUpdate(working) : serializeCreate(working)
        const payload = {
          ...safePayload,
          ...(editor.loaded
            ? { pre_backup_command: editor.hooks.pre || null, post_backup_command: editor.hooks.post || null }
            : {}),
        }
        let schedulerFailed = false
        if (savedId) {
          try {
            await api.put(`/targets/${savedId}`, payload, { signal })
          } catch (error) {
            if (responseStatus(error) !== 503) throw error
            schedulerFailed = true
          }
        } else {
          const response = await post('target', payload)
          if (!current()) return
          if (!Number.isSafeInteger(response.data.id) || response.data.id <= 0) throw new ResponseError()
          savedId = response.data.id
          working = { ...working, confirmedTargetId: savedId!, unconfirmed: null }
          setBaseline(response.data.updated_at)
          checkpoint(working, response.data.updated_at)
        }
        if (!current()) return
        const config: PrivateTargetConfig = { signal, privateTargetHooks: true }
        const stored = (await api.get<BackupTarget>(`/targets/${savedId}`, config)).data
        const matches = Object.entries(payload).every(([key, value]) =>
          same(value, stored[key as keyof BackupTarget] ?? (value === null ? null : undefined))
        )
        if (!matches) {
          setErrors(['Saved configuration differs from the reviewed intent. Reload and compare before continuing.'])
          throw new ResponseError()
        }
        if (schedulerFailed) {
          await api.post('/schedules/reload', undefined, { signal })
          await api.get('/schedules/jobs', { signal })
          setReconciled(true)
        }
        revision = Date.parse(stored.updated_at) || 0
      },
      () => {
        if (!savedId) return
        finished.current = true
        writer?.clear()
        useNotificationStore
          .getState()
          .addNotification({
            code: 'plan-saved',
            target: { kind: 'plan', id: savedId },
            type: 'info',
            title: '',
            message: '',
            revision,
          })
        void client.invalidateQueries({ queryKey: queryKeys.targets.all })
        void client.invalidateQueries({ queryKey: queryKeys.schedules.all })
        void client.invalidateQueries({ queryKey: queryKeys.retention.all })
        editor.close()
        if (onSaved) onSaved(savedId)
        else onClose()
      }
    )
  }
  return (
    <div className="space-y-6 max-w-4xl">
      <PageHeader
        title={editTarget ? `Edit protection: ${editTarget.name}` : 'Create protection plan'}
        description="Review configuration, save it, then run separately."
      />
      <nav aria-label="Plan editor groups" className="flex flex-wrap gap-2">
        {GROUPS.map((name, i) => (
          <button
            type="button"
            className="btn-secondary"
            aria-current={group === i ? 'step' : undefined}
            key={name}
            onClick={() => setGroup(i)}
          >
            {name}
          </button>
        ))}
      </nav>
      {draft && (
        <section aria-label="Unsaved draft" className="space-y-2 border border-dark-600 p-4">
          <p>
            Unsaved safe intent found for this account and {editTarget ? 'plan' : 'new plan'}. Hooks must be entered or
            loaded again. Recovery rechecks current resources.
          </p>
          <button type="button" className="btn-secondary" onClick={restore}>
            Restore draft
          </button>
          <button type="button" className="btn-secondary" onClick={discard}>
            Discard draft
          </button>
        </section>
      )}
      {conflict && (
        <section role="alert" className="space-y-2">
          <p>
            Server revision changed. No changes were saved. Compare current configuration with your intent before
            continuing.
          </p>
          <pre className="whitespace-pre-wrap break-all">
            {JSON.stringify(serializeUpdate(targetToWizardData(conflict)), null, 2)}
          </pre>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => {
              setBaseline(conflict.updated_at)
              setData((prev) => ({
                ...(draft?.data || prev),
                targetType: conflict.target_type,
                containerName: conflict.container_name || '',
                stackName: conflict.stack_name || '',
                volumeName: conflict.volume_name || '',
                hostPath: conflict.host_path || '',
              }))
              setDraft(null)
              setTouched(true)
              setConflict(null)
            }}
          >
            Keep safe intent against current revision
          </button>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => {
              setData({ ...targetToWizardData(conflict), confirmedTargetId: editTarget ? null : conflict.id })
              setBaseline(conflict.updated_at)
              setConflict(null)
              setDraft(null)
              writer?.clear()
              setTouched(false)
              editor.retry()
            }}
          >
            Use server configuration
          </button>
        </section>
      )}
      <div ref={errorFocus} tabIndex={-1} role={errors.length || editor.saveError ? 'alert' : undefined}>
        {errors.map((error) => (
          <p key={error}>{error}</p>
        ))}
        {validation.summary}
        {editor.saveError && (
          <p>
            Could not save backup configuration. Safe inputs and confirmed policy IDs are retained. Check current server
            state; cancellation does not undo a committed write.
          </p>
        )}
      </div>
      {data.unconfirmed && (
        <section role="alert" className="space-y-3">
          <h2 className="text-lg font-semibold">Unconfirmed {data.unconfirmed} creation</h2>
          <p>
            No automatic replay or name-based ownership assumption is safe. Read back candidates, compare all fields,
            and explicitly choose the resource. Discarding the draft does not delete server resources.
          </p>
          <button
            type="button"
            className="btn-secondary"
            disabled={editor.pending}
            onClick={() => void readCandidates()}
          >
            Read back candidates
          </button>
          <button
            type="button"
            className="btn-secondary"
            disabled={editor.pending}
            onClick={() => void reloadScheduler()}
          >
            Reload scheduler and read jobs
          </button>
          {candidateError && <p>Candidate readback unavailable. Retry; no new resource has been requested.</p>}
          <ul>
            {candidates.map((candidate) => (
              <li key={candidate.id} className="space-y-2 break-all">
                <p>{candidate.label}</p>
                <button type="button" className="btn-secondary" onClick={() => void chooseCandidate(candidate.id)}>
                  {data.unconfirmed === 'target' ? 'Open and inspect' : 'Use explicitly selected'} #{candidate.id}
                </button>
              </li>
            ))}
          </ul>
          {data.unconfirmed !== 'target' && (
            <p>
              Policies are shared. Selecting a candidate does not prove it was created by this editor and never modifies
              or deletes it.
            </p>
          )}
        </section>
      )}
      {reconciled && (
        <p role="status">
          Scheduler reloaded and jobs read back. Unconfirmed creation still requires explicit candidate selection.
        </p>
      )}
      {editor.loading && <p role="status">Loading private hook commands…</p>}
      {editor.loadError && (
        <p role="alert">
          Could not load hook commands. Existing commands will be preserved.{' '}
          <button type="button" className="btn-secondary" onClick={editor.retry}>
            Retry hook load
          </button>
        </p>
      )}
      <fieldset disabled={editor.pending} className="space-y-6 min-w-0">
        {group === 0 && (
          <>
            <QueryFeedback name="Existing plans" query={targets} />
            {editTarget || data.confirmedTargetId ? (
              <section className="space-y-3">
                <p className="break-all">
                  Source (read-only): {data.targetType} —{' '}
                  {data.containerName || data.stackName || data.volumeName || data.hostPath}. To change source, create a
                  new plan; existing archives keep their identity.
                </p>
                <label className="block">
                  Plan name
                  <input
                    className="input w-full"
                    value={data.targetName}
                    {...validation.field('name')}
                    onChange={(e) => updateData({ targetName: e.target.value })}
                  />
                </label>
              </section>
            ) : (
              <StepTargetSelect
                data={data}
                updateData={updateData}
                containers={containers.data || []}
                stacks={stacks.data || []}
                volumes={volumes.data || []}
                existingTargets={targets.data || []}
                nameFeedback={validation.field('name')}
                unavailableTypes={[
                  ...(containers.isError || containers.isPending ? ['container'] : []),
                  ...(stacks.isError || stacks.isPending ? ['stack'] : []),
                  ...(volumes.isError || volumes.isPending ? ['volume'] : []),
                ]}
              />
            )}
            <QueryFeedback name="Containers" query={containers} />
            <QueryFeedback name="Stacks" query={stacks} />
            <QueryFeedback name="Volumes" query={volumes} />
            {data.targetType && (
              <StepVolumeConfig
                data={data}
                updateData={updateData}
                containers={containers.data || []}
                stacks={stacks.data || []}
              />
            )}
          </>
        )}
        {group === 1 && (
          <>
            <QueryFeedback name="Schedule choices" query={schedules} />
            <StepSchedule
              feedback={validation.field}
              data={data}
              updateData={updateData}
              schedules={schedules.data || []}
              isLoadingSchedules={schedules.isPending}
            />
            <QueryFeedback name="Retention choices" query={retention} />
            <StepRetention
              feedback={validation.field}
              data={data}
              updateData={updateData}
              policies={retention.data || []}
              isLoadingPolicies={retention.isPending}
            />
            <QueryFeedback name="Storage choices" query={storages} />
            <StepStorage
              data={data}
              updateData={updateData}
              storages={storages.data || []}
              isLoadingStorages={storages.isPending}
            />
          </>
        )}
        {group === 2 && (
          <>
            <p>
              Stack members follow the engine stop/start topology even when source-stop is disabled. Explicit
              dependencies are stopped independently; original stopped states are preserved. Listed order is not a
              custom execution guarantee.
            </p>
            <StepDependencies
              data={data}
              updateData={updateData}
              containers={containers.data || []}
              stacks={stacks.data || []}
            />
            <StepOptions
              hooksDisabled={editor.loading || !editor.loaded || editor.pending}
              data={{ ...data, preCommand: editor.hooks.pre, postCommand: editor.hooks.post }}
              updateData={(updates) => {
                if (updates.preCommand !== undefined) editor.setHooks((prev) => ({ ...prev, pre: updates.preCommand! }))
                if (updates.postCommand !== undefined)
                  editor.setHooks((prev) => ({ ...prev, post: updates.postCommand! }))
                const { preCommand: _pre, postCommand: _post, ...safe } = updates
                if (Object.keys(safe).length) updateData(safe)
              }}
            />
            <StepSummary
              data={data}
              hooks={editor.loaded ? editor.hooks : undefined}
              schedules={schedules.data || []}
              policies={retention.data || []}
              storages={storages.data || []}
              availableVolumes={availableVolumes(data, containers.data || [], stacks.data || [])}
              isCreating={editor.pending}
              onGoToStep={setGroup}
            />
          </>
        )}
      </fieldset>
      <footer className="flex flex-wrap gap-3 border-t border-dark-600 pt-4">
        <button type="button" className="btn-secondary" onClick={cancel}>
          Cancel
        </button>
        <button type="button" className="btn-secondary" disabled={editor.pending} onClick={discard}>
          Discard changes
        </button>
        {group < 2 ? (
          <button type="button" className="btn-primary" onClick={() => setGroup(group + 1)}>
            Next: {GROUPS[group + 1]}
          </button>
        ) : (
          <button
            type="button"
            className="btn-primary"
            disabled={editor.pending || !!data.unconfirmed || !!draft || !!conflict}
            onClick={() => void save()}
          >
            {editor.pending ? 'Saving…' : 'Save plan'}
          </button>
        )}
      </footer>
    </div>
  )
}
