import { useState } from 'react'
import { Calendar, Clock, HelpCircle, Plus, ChevronDown, ChevronUp } from 'lucide-react'
import { WizardData } from './index'
import { ScheduleEntity } from '../../api'

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
  { label: 'Weekly on Sunday', cron: '0 0 * * 0', description: 'Every Sunday at 00:00' },
  { label: 'Monthly', cron: '0 0 1 * *', description: 'First day of month at 00:00' },
]

export default function StepSchedule({ data, updateData, schedules, isLoadingSchedules }: Props) {
  const [showCronHelp, setShowCronHelp] = useState(false)
  const [showPresets, setShowPresets] = useState(false)
  const [createNew, setCreateNew] = useState(!data.scheduleId)

  const handleScheduleSelect = (scheduleId: string) => {
    if (scheduleId === 'none') {
      updateData({ scheduleId: null, newSchedule: null })
      setCreateNew(false)
    } else if (scheduleId === 'new') {
      updateData({
        scheduleId: null,
        newSchedule: { name: '', cronExpression: '0 0 * * *', description: '' },
      })
      setCreateNew(true)
    } else {
      updateData({ scheduleId: parseInt(scheduleId), newSchedule: null })
      setCreateNew(false)
    }
  }

  const updateNewSchedule = (updates: Partial<NonNullable<WizardData['newSchedule']>>) => {
    if (data.newSchedule) {
      updateData({ newSchedule: { ...data.newSchedule, ...updates } })
    }
  }

  const applyPreset = (cron: string) => {
    updateNewSchedule({ cronExpression: cron })
    setShowPresets(false)
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
      <div>
        <label className="block text-sm font-medium text-dark-300 mb-2">Select Schedule</label>
        <div className="relative">
          <Calendar className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-400" />
          <select
            value={createNew ? 'new' : data.scheduleId?.toString() || 'none'}
            onChange={(e) => handleScheduleSelect(e.target.value)}
            disabled={isLoadingSchedules}
            className="w-full pl-10 pr-4 py-3 bg-dark-800 border border-dark-700 rounded-xl text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500 appearance-none cursor-pointer"
          >
            <option value="none">No automatic schedule</option>
            <option value="new">➕ Create new schedule...</option>
            {schedules.map((schedule) => (
              <option key={schedule.id} value={schedule.id}>
                {schedule.name} ({schedule.cron_expression})
              </option>
            ))}
          </select>
        </div>
      </div>

      {/* Existing Schedule Info */}
      {data.scheduleId && !createNew && (
        <div className="bg-primary-500/10 border border-primary-500/30 rounded-xl p-4">
          {(() => {
            const selected = schedules.find((s) => s.id === data.scheduleId)
            return selected ? (
              <div className="flex items-start gap-3">
                <Clock className="w-5 h-5 text-primary-400 mt-0.5" />
                <div>
                  <h4 className="text-primary-400 font-medium">{selected.name}</h4>
                  <p className="text-sm text-dark-300 mt-1">
                    Cron: <code className="bg-dark-700 px-2 py-0.5 rounded">{selected.cron_expression}</code>
                  </p>
                  {selected.description && (
                    <p className="text-sm text-dark-400 mt-1">{selected.description}</p>
                  )}
                </div>
              </div>
            ) : null
          })()}
        </div>
      )}

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
              <button
                type="button"
                onClick={() => setShowPresets(!showPresets)}
                className="px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-300 hover:text-dark-100 transition-colors"
              >
                {showPresets ? <ChevronUp className="w-4 h-4" /> : <ChevronDown className="w-4 h-4" />}
              </button>
            </div>

            {/* Cron Presets */}
            {showPresets && (
              <div className="mt-2 grid grid-cols-2 gap-2">
                {CRON_PRESETS.map((preset) => (
                  <button
                    key={preset.cron}
                    type="button"
                    onClick={() => applyPreset(preset.cron)}
                    className="p-2 text-left bg-dark-900 border border-dark-600 rounded-lg hover:border-primary-500 transition-colors"
                  >
                    <p className="text-sm text-dark-100">{preset.label}</p>
                    <p className="text-xs text-dark-500 font-mono">{preset.cron}</p>
                  </button>
                ))}
              </div>
            )}

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
                    <div>0-6</div>
                  </div>
                </div>
                <p className="mt-2 text-dark-400">
                  Use <code>*</code> for any, <code>*/n</code> for every n, <code>n,m</code> for specific values
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
