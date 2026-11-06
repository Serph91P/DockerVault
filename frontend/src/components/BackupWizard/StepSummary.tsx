import type { WizardData } from './planEditorModel'
import type { ScheduleEntity, RemoteStorage, RetentionPolicy } from '../../api'
import { CronPreview } from './PolicyFields'

export default function StepSummary({
  data,
  schedules,
  storages,
  policies,
  onGoToStep,
  availableVolumes = [],
  hooks,
}: {
  data: WizardData
  schedules: ScheduleEntity[]
  storages: RemoteStorage[]
  policies: RetentionPolicy[]
  isCreating: boolean
  onGoToStep?: (step: number) => void
  availableVolumes?: string[]
  hooks?: { pre: string; post: string }
}) {
  const schedule = schedules.find((s) => s.id === data.scheduleId)
  const retention =
    policies.find((p) => p.id === data.retentionPolicyId) ||
    (!data.retentionPolicyId ? policies.find((p) => p.name === 'default') : undefined)
  const expression =
    data.newSchedule?.cronExpression ||
    schedule?.cron_expression ||
    (data.scheduleMode === 'legacy' ? data.scheduleCron : null)
  return (
    <section className="space-y-4" aria-label="Reviewed configuration">
      <h3 className="text-lg font-semibold">Review plan</h3>
      <p>Saving configures protection. It does not run a backup or prove that a restore works.</p>
      <div className="space-y-3 break-words">
        <h4 className="font-semibold">Source & scope</h4>
        <p>
          {data.targetName}: {data.targetType} —{' '}
          {data.containerName || data.stackName || data.volumeName || data.hostPath}
        </p>
        {['container', 'stack'].includes(data.targetType || '') && (
          <p>
            {data.volumeMode === 'all'
              ? `All named volumes (dynamic): ${availableVolumes.join(', ') || 'not confirmed'}`
              : `Explicit subset: ${data.selectedVolumes.join(', ') || 'INVALID: no selection'}`}
            . Bind mounts/rootfs are not included.
          </p>
        )}
        <p>
          Global includes: {data.includePaths.join(', ') || 'all paths'}; excludes:{' '}
          {data.excludePaths.join(', ') || 'none'}.
        </p>
        {Object.entries(data.perVolumeRules).map(([key, rule]) => (
          <p key={key} className="break-all">
            Rule {key}:{' '}
            {Object.keys(rule).length
              ? `includes ${rule.includePaths?.join(', ') || 'all'}; excludes ${rule.excludePaths?.join(', ') || 'none'} (replaces globals)`
              : 'inherit globals'}
            {key.startsWith('volume:') &&
              data.volumeMode === 'subset' &&
              !data.selectedVolumes.includes(key.slice(7)) &&
              ' — dormant, not selected'}
          </p>
        ))}
        {onGoToStep && (
          <button type="button" className="btn-secondary" onClick={() => onGoToStep(0)}>
            Edit source & scope
          </button>
        )}
      </div>
      <div className="space-y-3">
        <h4 className="font-semibold">Strategy</h4>
        <p>
          Schedule:{' '}
          {data.newSchedule
            ? `new ${data.newSchedule.name}`
            : data.scheduleId
              ? `${schedule?.name || `unverified #${data.scheduleId}`} (${schedule?.enabled ? 'enabled' : 'disabled or unknown'})`
              : data.scheduleMode === 'legacy'
                ? `preserve legacy ${data.scheduleCron}`
                : 'manual only; both schedule fields cleared'}
          .
        </p>
        {expression && <CronPreview expression={expression} />}
        <p>
          Retention:{' '}
          {data.newRetentionPolicy
            ? `new ${data.newRetentionPolicy.name}: last ${data.newRetentionPolicy.keepLast}, daily ${data.newRetentionPolicy.keepDaily}, weekly ${data.newRetentionPolicy.keepWeekly}, monthly ${data.newRetentionPolicy.keepMonthly}, yearly ${data.newRetentionPolicy.keepYearly}, age ${data.newRetentionPolicy.maxAgeDays} days`
            : `${data.retentionPolicyId ? 'assigned' : 'standard fallback'} ${retention?.name || (data.retentionPolicyId ? `unverified #${data.retentionPolicyId}` : 'policy named default when available')}`}
          . This is not retention-off or a deletion preview.
        </p>
        {retention && !data.newRetentionPolicy && (
          <p>
            Last {retention.keep_last}; daily {retention.keep_daily}; weekly {retention.keep_weekly}; monthly{' '}
            {retention.keep_monthly}; yearly {retention.keep_yearly}; max age {retention.max_age_days} days. Keep Last
            is protected independently of age.
          </p>
        )}
        <p>
          Local archive first. Remote destinations:{' '}
          {data.remoteStorageIds
            .map((id) => storages.find((s) => s.id === id)?.name || `unverified #${id}`)
            .join(', ') || 'none (local only)'}
          .
        </p>
        <p>
          {data.remoteStorageIds.length && data.deleteLocalAfterSync
            ? 'Remove local copy only after every required normal sync, including key material, succeeds.'
            : 'Keep local copy.'}{' '}
          Removing destinations does not delete existing copies. Provider reachability is not proven by saving.
        </p>
        {onGoToStep && (
          <button type="button" className="btn-secondary" onClick={() => onGoToStep(1)}>
            Edit strategy
          </button>
        )}
      </div>
      <div className="space-y-3">
        <h4 className="font-semibold">Execution</h4>
        <p>
          {data.targetType === 'stack'
            ? 'Stack members follow the existing engine stop/start topology, even with the source-stop flag disabled.'
            : data.targetType === 'container' && data.stopContainer
              ? 'Stop the source container while archiving.'
              : 'No automatic source-container stop. Volume/path does not stop every consumer.'}{' '}
          Explicit dependencies are additional stops regardless of the source-stop flag; only originally running
          containers are restarted. Engine discovery determines execution order; list order is not a guaranteed custom
          sequence.
        </p>
        <p>
          Dependencies: {data.dependencies.join(', ') || 'none'}. Source-stop flag: {data.stopContainer ? 'on' : 'off'}.
        </p>
        <p>
          Format: {data.compression === 'gzip' ? 'gzip (.tar.gz)' : 'tar (.tar)'}. Plan{' '}
          {data.enabled ? 'enabled (configured, not proven healthy)' : 'disabled (scheduled and manual runs blocked)'}.
        </p>
        <p>
          Private pre-hook: {hooks?.pre ? 'configured' : 'empty or not loaded'}; post-hook:{' '}
          {hooks?.post ? 'configured' : 'empty or not loaded'}. Commands are not persisted in drafts. Pre runs after
          stops; post after archive/encryption and before restart, on the backend with executable allowlist and timeout,
          not a shell pipeline.
        </p>
      </div>
    </section>
  )
}
