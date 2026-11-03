import { useState, useEffect, useRef } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Calendar, Clock, HelpCircle, Plus, Ban } from 'lucide-react'
import { WizardData } from './index'
import { ScheduleEntity, schedulesApi } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  schedules: ScheduleEntity[]
  isLoadingSchedules: boolean
}

// Common cron presets
const CRON_PRESETS = [
  { label: 'Every hour', cron: '0 * * * *', description: 'At minute 0 of every hour' },
  { label: 'Every 6 hours', cron: '0 */6 * * *', description: 'At minute 0 every 6 hours' },
  { label: 'Daily at midnight', cron: '0 0 * * *', description: 'Every day at 00:00' },
  { label: 'Daily at 3 AM', cron: '0 3 * * *', description: 'Every day at 03:00' },
  { label: 'Weekly on Sunday', cron: '0 0 * * sun', description: 'Every Sunday at 00:00' },
  { label: 'Monthly', cron: '0 0 1 * *', description: 'First day of month at 00:00' },
]

function CronPreview({ expression }: { expression: string }) {
  const preview = useQuery({
    queryKey: ['cron-preview', expression],
    queryFn: () => schedulesApi.preview(expression).then((r) => r.data),
    enabled: !!expression,
    retry: false,
  })
  if (preview.isError) {
    const error = preview.error as { response?: { data?: { detail?: string } } }
    return <p role="alert" className="text-sm text-red-400">{error.response?.data?.detail || 'Schedule preview unavailable. Check the server before saving.'}</p>
  }
  if (!preview.data) return <p className="text-xs text-dark-400">Loading server preview...</p>
  return <div className="mt-2 space-y-2">
    <p className="text-xs text-dark-400">{preview.data.contract}</p>
    <p className="text-xs text-dark-400">Next runs (server timezone: {preview.data.timezone}; dates displayed in your local timezone):</p>
    {preview.data.next_runs.map((text) => <p key={text} className="text-sm text-primary-400">{new Date(text).toLocaleString()}</p>)}
    {preview.data.warnings.map((warning) => <p key={warning} role="alert" className="text-sm text-yellow-400">{warning}</p>)}
  </div>
}

export default function StepSchedule({ data, updateData, schedules, isLoadingSchedules }: Props) {
  const [showCronHelp, setShowCronHelp] = useState(false)
  const createNew = !!data.newSchedule
  const hasAutoSelected = useRef(false)

  // Smart defaults: auto-select schedule based on available options
  useEffect(() => {
    if (hasAutoSelected.current || data.scheduleId || data.newSchedule) return
    if (isLoadingSchedules) return

    if (schedules.length === 0) {
      updateData({
        scheduleId: null,
        newSchedule: { name: '', cronExpression: '0 0 * * *', description: '' },
      })
    } else if (schedules.length === 1) {
      updateData({ scheduleId: schedules[0].id, newSchedule: null })
    }
    hasAutoSelected.current = true
  }, [schedules, isLoadingSchedules, data.scheduleId, data.newSchedule, updateData])

  const handleScheduleSelect = (scheduleId: string) => {
    hasAutoSelected.current = true
    if (scheduleId === 'none') {
      updateData({ scheduleId: null, newSchedule: null })
    } else if (scheduleId === 'new') {
      updateData({
        scheduleId: null,
        newSchedule: { name: '', cronExpression: '0 0 * * *', description: '' },
      })
    } else {
      updateData({ scheduleId: parseInt(scheduleId), newSchedule: null })
    }
  }

  const updateNewSchedule = (updates: Partial<NonNullable<WizardData['newSchedule']>>) => {
    if (data.newSchedule) {
      updateData({ newSchedule: { ...data.newSchedule, ...updates } })
    }
  }

  const applyPreset = (cron: string) => {
    updateNewSchedule({ cronExpression: cron })
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Backup Schedule</h3>
        <p className="text-sm text-dark-400">
          Choose an existing schedule or create a new one for automated backups
        </p>
      </div>

      {/* Schedule Selection */}
      <div className="space-y-3">
        <label className="block text-sm font-medium text-dark-300">Select Schedule</label>

        {/* Existing Schedules - S1: sorted by usage (most targets first) */}
        {schedules.length > 0 && (
          <div className="rounded-xl border border-dark-700 divide-y divide-dark-700 overflow-hidden">
            {[...schedules].sort((a, b) => b.target_count - a.target_count).map((schedule) => {
              const isSelected = !createNew && data.scheduleId === schedule.id
              return (
                <button
                  key={schedule.id}
                  onClick={() => handleScheduleSelect(schedule.id.toString())}
                  className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                    isSelected
                      ? 'bg-primary-500/15'
                      : 'bg-dark-800 hover:bg-dark-750'
                  }`}
                >
                  <Calendar className={`w-4 h-4 flex-shrink-0 ${isSelected ? 'text-primary-400' : 'text-dark-500'}`} />
                  <div className="min-w-0 flex-1">
                    <span className={`block text-sm font-medium truncate ${isSelected ? 'text-primary-300' : 'text-dark-200'}`}>
                      {schedule.name}
                    </span>
                    <span className="block text-xs text-dark-500 font-mono truncate">
                      {schedule.cron_expression}
                      {schedule.description && ` — ${schedule.description}`}
                    </span>
                  </div>
                  {schedule.target_count > 0 && (
                    <span className="flex-shrink-0 px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-400">
                      {schedule.target_count} target{schedule.target_count !== 1 ? 's' : ''}
                    </span>
                  )}
                  {isSelected && (
                    <span className="flex-shrink-0 w-2 h-2 rounded-full bg-primary-400" />
                  )}
                </button>
              )
            })}
          </div>
        )}

        {isLoadingSchedules && (
          <p className="text-sm text-dark-500 py-2">Loading schedules...</p>
        )}

        {/* Action Buttons */}
        <div className="flex gap-3">
          <button
            type="button"
            onClick={() => handleScheduleSelect('new')}
            className={`flex-1 flex items-center justify-center gap-2 px-4 py-3 rounded-xl border-2 border-dashed transition-all ${
              createNew
                ? 'border-primary-500 bg-primary-500/10 text-primary-300'
                : 'border-dark-600 text-dark-400 hover:border-dark-500 hover:text-dark-300'
            }`}
          >
            <Plus className="w-4 h-4" />
            <span className="text-sm font-medium">Create new schedule</span>
          </button>
        </div>

        {/* S3: None option - less prominent, at the bottom */}
        <button
          type="button"
          onClick={() => handleScheduleSelect('none')}
          className={`w-full flex items-center justify-center gap-2 px-3 py-2 rounded-lg text-xs transition-all ${
            !createNew && !data.scheduleId
              ? 'bg-yellow-500/10 text-yellow-400 border border-yellow-500/30'
              : 'text-dark-500 hover:text-dark-400 hover:bg-dark-800'
          }`}
          title="Only for manual backups — no automatic scheduling"
        >
          <Ban className="w-3 h-3" />
          <span>No schedule (manual only)</span>
        </button>
      </div>

      {/* Create New Schedule Form */}
      {createNew && data.newSchedule && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Plus className="w-4 h-4 text-primary-400" />
            <h4 className="text-dark-100 font-medium">New Schedule</h4>
          </div>

          {/* Schedule Name */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">Schedule Name *</label>
            <input
              type="text"
              value={data.newSchedule.name}
              onChange={(e) => updateNewSchedule({ name: e.target.value })}
              placeholder="e.g., Daily Backup, Weekly Full"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
          </div>

          {/* Cron Expression */}
          <div>
            <div className="flex items-center justify-between mb-2">
              <label className="text-sm font-medium text-dark-300">Cron Expression *</label>
              <button
                type="button"
                onClick={() => setShowCronHelp(!showCronHelp)}
                className="text-xs text-primary-400 hover:text-primary-300 flex items-center gap-1"
              >
                <HelpCircle className="w-3 h-3" />
                {showCronHelp ? 'Hide help' : 'Show help'}
              </button>
            </div>

            <div className="flex gap-2">
              <input
                type="text"
                value={data.newSchedule.cronExpression}
                onChange={(e) => updateNewSchedule({ cronExpression: e.target.value })}
                placeholder="0 0 * * *"
                className="flex-1 px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 font-mono focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
              />
            </div>

            {/* Human-readable cron preview */}
            {data.newSchedule.cronExpression && (
              <CronPreview expression={data.newSchedule.cronExpression} />
            )}

            {/* Cron Presets — always visible */}
            <div className="mt-3 grid grid-cols-3 gap-2">
              {CRON_PRESETS.map((preset) => {
                const isActive = data.newSchedule?.cronExpression === preset.cron
                return (
                  <button
                    key={preset.cron}
                    type="button"
                    onClick={() => applyPreset(preset.cron)}
                    className={`p-2 text-left rounded-lg border transition-colors ${
                      isActive
                        ? 'border-primary-500 bg-primary-500/10'
                        : 'border-dark-600 bg-dark-900 hover:border-primary-500/50'
                    }`}
                  >
                    <p className={`text-sm ${isActive ? 'text-primary-300' : 'text-dark-100'}`}>{preset.label}</p>
                    <p className="text-xs text-dark-500 font-mono">{preset.cron}</p>
                  </button>
                )
              })}
            </div>

            {/* Cron Help */}
            {showCronHelp && (
              <div className="mt-3 bg-dark-900 border border-dark-600 rounded-lg p-3 text-sm">
                <p className="text-dark-300 mb-2">Cron format: <code>minute hour day month weekday</code></p>
                <div className="grid grid-cols-5 gap-2 text-xs text-dark-400">
                  <div className="text-center">
                    <div className="font-medium text-dark-300">Minute</div>
                    <div>0-59</div>
                  </div>
                  <div className="text-center">
                    <div className="font-medium text-dark-300">Hour</div>
                    <div>0-23</div>
                  </div>
                  <div className="text-center">
                    <div className="font-medium text-dark-300">Day</div>
                    <div>1-31</div>
                  </div>
                  <div className="text-center">
                    <div className="font-medium text-dark-300">Month</div>
                    <div>1-12</div>
                  </div>
                  <div className="text-center">
                    <div className="font-medium text-dark-300">Weekday</div>
                    <div>mon-sun (0=Mon, 6=Sun)</div>
                  </div>
                </div>
                <p className="mt-2 text-dark-400">
                  Use <code>*</code> for any, <code>*/n</code> for every n, <code>n,m</code> for specific values. Day-of-month AND weekday must both match; macros are unsupported.
                </p>
              </div>
            )}
          </div>

          {/* Description */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">Description (optional)</label>
            <input
              type="text"
              value={data.newSchedule.description}
              onChange={(e) => updateNewSchedule({ description: e.target.value })}
              placeholder="e.g., Runs every night at midnight"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
          </div>
        </div>
      )}

      {/* No Schedule Info */}
      {!data.scheduleId && !createNew && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-xl p-4">
          <div className="flex items-start gap-3">
            <Clock className="w-5 h-5 text-yellow-400 mt-0.5" />
            <div>
              <h4 className="text-yellow-400 font-medium">Manual Backups Only</h4>
              <p className="text-sm text-dark-300 mt-1">
                Without a schedule, backups will only run when manually triggered.
                You can always add a schedule later.
              </p>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
