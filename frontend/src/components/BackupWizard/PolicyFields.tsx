import { useQuery } from '@tanstack/react-query'
import { schedulesApi } from '../../api'
export interface ScheduleDraft {
  name: string
  cronExpression: string
  description: string
  enabled?: boolean
}
export interface RetentionDraft {
  name: string
  keepLast: number | ''
  keepDaily: number | ''
  keepWeekly: number | ''
  keepMonthly: number | ''
  keepYearly: number | ''
  maxAgeDays: number | ''
}
import type { useValidationFeedback } from '../ValidationFeedback'
type Feedback = ReturnType<typeof useValidationFeedback>['field']

export function CronPreview({ expression }: { expression: string }) {
  const preview = useQuery({
    queryKey: ['cron-preview', expression],
    queryFn: () => schedulesApi.preview(expression).then((r) => r.data),
    enabled: !!expression,
    retry: false,
  })
  if (preview.isError)
    return (
      <p role="alert">
        Schedule preview unavailable. Check the server before saving.
      </p>
    )
  if (!preview.data) return <p role="status">Loading server preview...</p>
  return (
    <div className="text-sm space-y-2">
      <p>{preview.data.contract}</p>
      <p>
        Next runs (server timezone: {preview.data.timezone}; dates displayed in
        your local timezone):
      </p>
      {preview.data.next_runs.map((text) => (
        <p key={text}>{new Date(text).toLocaleString()}</p>
      ))}
      {preview.data.warnings.map((warning) => (
        <p role="alert" key={warning}>
          {warning}
        </p>
      ))}
    </div>
  )
}
export function ScheduleFields({
  value,
  onChange,
  feedback,
  legend = 'New shared schedule',
}: {
  value: ScheduleDraft
  onChange: (value: ScheduleDraft) => void
  feedback?: Feedback
  legend?: string
}) {
  return (
    <fieldset className="space-y-3">
      <legend>{legend}</legend>
      <label className="block">
        Schedule name
        <input
          className="input w-full"
          {...feedback?.('name')}
          value={value.name}
          onChange={(e) => onChange({ ...value, name: e.target.value })}
        />
      </label>
      <label className="block">
        Preset
        <select
          className="input w-full"
          value=""
          onChange={(e) =>
            onChange({ ...value, cronExpression: e.target.value })
          }
        >
          <option value="">Choose a preset</option>
          {[
            ['Every hour', '0 * * * *'],
            ['Every 6 hours', '0 */6 * * *'],
            ['Daily at midnight', '0 0 * * *'],
            ['Daily at 3 AM', '0 3 * * *'],
            ['Weekly on Sunday', '0 0 * * sun'],
            ['Monthly', '0 0 1 * *'],
          ].map(([name, cron]) => (
            <option value={cron} key={cron}>
              {name}
            </option>
          ))}
        </select>
      </label>
      <label className="block">
        Cron expression
        <input
          className="input w-full font-mono"
          {...feedback?.('cron_expression')}
          value={value.cronExpression}
          onChange={(e) =>
            onChange({ ...value, cronExpression: e.target.value })
          }
        />
      </label>
      <label className="block">
        Description
        <input
          className="input w-full"
          {...feedback?.('description')}
          value={value.description ?? ''}
          onChange={(e) => onChange({ ...value, description: e.target.value })}
        />
      </label>
      <label className="flex gap-2">
        <input
          type="checkbox"
          checked={value.enabled !== false}
          onChange={(e) => onChange({ ...value, enabled: e.target.checked })}
        />
        Enable schedule
      </label>
      <details>
        <summary>Cron help</summary>
        <p>
          Five fields: minute, hour, day of month, month, day of week. Server
          scheduling uses Monday=0, Sunday=6; day-of-month AND day-of-week. Use
          named weekdays to avoid ambiguity. Preview is authoritative.
        </p>
      </details>
      <CronPreview expression={value.cronExpression} />
    </fieldset>
  )
}

export function RetentionFields({
  value,
  onChange,
  feedback,
  legend = 'New shared retention policy',
}: {
  value: RetentionDraft
  onChange: (value: RetentionDraft) => void
  feedback?: Feedback
  legend?: string
}) {
  const fields = [
    ['keepLast', 'Keep Last', 1, 1000],
    ['keepDaily', 'Daily', 0, 1000],
    ['keepWeekly', 'Weekly', 0, 1000],
    ['keepMonthly', 'Monthly', 0, 1000],
    ['keepYearly', 'Yearly', 0, 1000],
    ['maxAgeDays', 'Max age (days)', 1, 36500],
  ] as const
  return (
    <fieldset className="space-y-3">
      <legend>{legend}</legend>
      <label className="block">
        Policy name
        <input
          className="input w-full"
          {...feedback?.('name')}
          value={value.name}
          onChange={(e) => onChange({ ...value, name: e.target.value })}
        />
      </label>
      <div className="grid gap-3 sm:grid-cols-2">
        {fields.map(([key, label, min, max]) => (
          <label className="block" key={key}>
            {label}
            <input
              className="input w-full"
              {...feedback?.(
                key === 'maxAgeDays'
                  ? 'max_age_days'
                  : key.replace(/([A-Z])/g, '_$1').toLowerCase(),
              )}
              type="number"
              min={min}
              max={max}
              value={value[key]}
              onChange={(e) =>
                onChange({
                  ...value,
                  [key]: e.target.value === '' ? '' : Number(e.target.value),
                })
              }
            />
          </label>
        ))}
      </div>
      <p className="text-sm text-dark-300">
        Keep Last protects the newest backups independently of age. GFS buckets
        are combined, not added into a guaranteed total. Max age limits bucket
        candidates, not Keep Last; at least the newest backup remains protected.
        Zero disables a GFS bucket only.
      </p>
    </fieldset>
  )
}
