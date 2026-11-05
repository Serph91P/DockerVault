import type { ScheduleEntity } from '../../api'
import type { WizardData } from './planEditorModel'
import { CronPreview, ScheduleFields } from './PolicyFields'
import type { useValidationFeedback } from '../ValidationFeedback'

export default function StepSchedule({
  data,
  updateData,
  schedules,
  isLoadingSchedules,
  feedback,
}: {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  schedules: ScheduleEntity[]
  isLoadingSchedules: boolean
  feedback?: ReturnType<typeof useValidationFeedback>['field']
}) {
  const selected = schedules.find((s) => s.id === data.scheduleId)
  return (
    <section className="space-y-3" aria-label="Backup schedule">
      <h3 className="text-lg font-semibold">Backup Schedule</h3>
      {isLoadingSchedules && <p role="status">Loading schedules...</p>}
      <label className="block">
        Select Schedule
        <select
          className="input w-full"
          value={data.newSchedule ? 'new' : data.scheduleMode === 'legacy' ? 'legacy' : data.scheduleId || 'none'}
          onChange={(e) => {
            const value = e.target.value
            updateData({
              scheduleMode: value === 'none' ? 'manual' : value === 'legacy' ? 'legacy' : 'shared',
              ...(value === 'none' ? { scheduleCron: null } : {}),
              scheduleId: value !== 'none' && value !== 'new' && value !== 'legacy' ? Number(value) : null,
              newSchedule:
                value === 'new' ? { name: '', cronExpression: '0 0 * * *', description: '', enabled: true } : null,
            })
          }}
        >
          <option value="none">No schedule (manual only)</option>
          {data.scheduleCron && <option value="legacy">Preserve legacy cron: {data.scheduleCron}</option>}
          {data.scheduleId && !selected && (
            <option value={data.scheduleId}>Missing schedule #{data.scheduleId} — recheck</option>
          )}
          {schedules.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name} — {s.cron_expression}
              {!s.enabled && ' (disabled)'}
            </option>
          ))}
          <option value="new">Create new schedule</option>
        </select>
      </label>
      {data.newSchedule && (
        <ScheduleFields
          feedback={feedback}
          value={data.newSchedule}
          onChange={(newSchedule) => updateData({ newSchedule })}
        />
      )}
      {!data.newSchedule && (data.scheduleMode === 'legacy' ? data.scheduleCron : selected?.cron_expression) && (
        <CronPreview expression={(data.scheduleMode === 'legacy' ? data.scheduleCron : selected?.cron_expression)!} />
      )}
      {selected && (
        <p className="text-sm">
          Shared schedule {selected.enabled ? 'enabled' : 'disabled — no fallback to legacy cron'}. Editing a plan only
          assigns this policy; it does not change it for other plans.
        </p>
      )}
      {data.scheduleMode === 'manual' && !data.newSchedule && (
        <p>Manual only: both schedule assignment and legacy cron will be cleared.</p>
      )}
    </section>
  )
}
