import { useState } from 'react'
import { Archive, Plus, Clock, Trash2 } from 'lucide-react'
import { WizardData } from './index'
import { RetentionPolicy } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  policies: RetentionPolicy[]
  isLoadingPolicies: boolean
}

// Common retention presets
const RETENTION_PRESETS = [
  { name: 'Keep Last 7', keepLast: 7, keepDaily: 0, keepWeekly: 0, keepMonthly: 0 },
  { name: 'Daily for a week', keepLast: 0, keepDaily: 7, keepWeekly: 0, keepMonthly: 0 },
  { name: '7 daily + 4 weekly', keepLast: 0, keepDaily: 7, keepWeekly: 4, keepMonthly: 0 },
  { name: 'Full rotation', keepLast: 7, keepDaily: 7, keepWeekly: 4, keepMonthly: 6 },
]

export default function StepRetention({ data, updateData, policies, isLoadingPolicies }: Props) {
  const [createNew, setCreateNew] = useState(!data.retentionPolicyId)

  const handlePolicySelect = (policyId: string) => {
    if (policyId === 'none') {
      updateData({
        retentionPolicyId: null,
        newRetentionPolicy: null,
      })
      setCreateNew(false)
    } else if (policyId === 'new') {
      updateData({
        retentionPolicyId: null,
        newRetentionPolicy: {
          name: '',
          keepLast: 7,
          keepDaily: 0,
          keepWeekly: 0,
          keepMonthly: 0,
        },
      })
      setCreateNew(true)
    } else {
      updateData({ retentionPolicyId: parseInt(policyId), newRetentionPolicy: null })
      setCreateNew(false)
    }
  }

  const updateNewPolicy = (updates: Partial<NonNullable<WizardData['newRetentionPolicy']>>) => {
    if (data.newRetentionPolicy) {
      updateData({ newRetentionPolicy: { ...data.newRetentionPolicy, ...updates } })
    }
  }

  const applyPreset = (preset: (typeof RETENTION_PRESETS)[number]) => {
    updateNewPolicy({
      keepLast: preset.keepLast,
      keepDaily: preset.keepDaily,
      keepWeekly: preset.keepWeekly,
      keepMonthly: preset.keepMonthly,
    })
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Retention Policy</h3>
        <p className="text-sm text-dark-400">
          Define how long to keep backups and how many to retain
        </p>
      </div>

      {/* Policy Selection */}
      <div>
        <label className="block text-sm font-medium text-dark-300 mb-2">Select Policy</label>
        <div className="relative">
          <Archive className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-400" />
          <select
            value={createNew ? 'new' : data.retentionPolicyId?.toString() || 'none'}
            onChange={(e) => handlePolicySelect(e.target.value)}
            disabled={isLoadingPolicies}
            className="w-full pl-10 pr-4 py-3 bg-dark-800 border border-dark-700 rounded-xl text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500 appearance-none cursor-pointer"
          >
            <option value="none">No retention policy (keep all)</option>
            <option value="new">➕ Create new policy...</option>
            {policies.map((policy) => (
              <option key={policy.id} value={policy.id}>
                {policy.name}
              </option>
            ))}
          </select>
        </div>
      </div>

      {/* Existing Policy Info */}
      {data.retentionPolicyId && !createNew && (
        <div className="bg-primary-500/10 border border-primary-500/30 rounded-xl p-4">
          {(() => {
            const selected = policies.find((p) => p.id === data.retentionPolicyId)
            return selected ? (
              <div className="flex items-start gap-3">
                <Archive className="w-5 h-5 text-primary-400 mt-0.5" />
                <div>
                  <h4 className="text-primary-400 font-medium">{selected.name}</h4>
                  <div className="text-sm text-dark-300 mt-2 grid grid-cols-2 gap-2">
                    {selected.keep_last > 0 && (
                      <div>Keep Last: <span className="text-dark-100">{selected.keep_last}</span></div>
                    )}
                    {selected.keep_daily > 0 && (
                      <div>Daily: <span className="text-dark-100">{selected.keep_daily}</span></div>
                    )}
                    {selected.keep_weekly > 0 && (
                      <div>Weekly: <span className="text-dark-100">{selected.keep_weekly}</span></div>
                    )}
                    {selected.keep_monthly > 0 && (
                      <div>Monthly: <span className="text-dark-100">{selected.keep_monthly}</span></div>
                    )}
                    {selected.keep_yearly > 0 && (
                      <div>Yearly: <span className="text-dark-100">{selected.keep_yearly}</span></div>
                    )}
                  </div>
                </div>
              </div>
            ) : null
          })()}
        </div>
      )}

      {/* Create New Policy Form */}
      {createNew && data.newRetentionPolicy && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Plus className="w-4 h-4 text-primary-400" />
            <h4 className="text-dark-100 font-medium">New Retention Policy</h4>
          </div>

          {/* Policy Name */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">Policy Name *</label>
            <input
              type="text"
              value={data.newRetentionPolicy.name}
              onChange={(e) => updateNewPolicy({ name: e.target.value })}
              placeholder="e.g., Standard Rotation, Long-term Storage"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
          </div>

          {/* Presets */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">Quick Presets</label>
            <div className="grid grid-cols-2 gap-2">
              {RETENTION_PRESETS.map((preset) => (
                <button
                  key={preset.name}
                  type="button"
                  onClick={() => applyPreset(preset)}
                  className="p-2 text-left bg-dark-900 border border-dark-600 rounded-lg hover:border-primary-500 transition-colors"
                >
                  <p className="text-sm text-dark-100">{preset.name}</p>
                </button>
              ))}
            </div>
          </div>

          {/* Retention Values */}
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">
                <Clock className="w-3 h-3 inline mr-1" />
                Keep Last
              </label>
              <input
                type="number"
                min="0"
                value={data.newRetentionPolicy.keepLast}
                onChange={(e) => updateNewPolicy({ keepLast: parseInt(e.target.value) || 0 })}
                className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
              />
              <p className="text-xs text-dark-500 mt-1">Newest backups to keep</p>
            </div>

            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">
                <Trash2 className="w-3 h-3 inline mr-1" />
                Daily
              </label>
              <input
                type="number"
                min="0"
                value={data.newRetentionPolicy.keepDaily}
                onChange={(e) => updateNewPolicy({ keepDaily: parseInt(e.target.value) || 0 })}
                className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
              />
              <p className="text-xs text-dark-500 mt-1">One per day to keep</p>
            </div>

            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Weekly</label>
              <input
                type="number"
                min="0"
                value={data.newRetentionPolicy.keepWeekly}
                onChange={(e) => updateNewPolicy({ keepWeekly: parseInt(e.target.value) || 0 })}
                className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
              />
              <p className="text-xs text-dark-500 mt-1">One per week to keep</p>
            </div>

            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Monthly</label>
              <input
                type="number"
                min="0"
                value={data.newRetentionPolicy.keepMonthly}
                onChange={(e) => updateNewPolicy({ keepMonthly: parseInt(e.target.value) || 0 })}
                className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
              />
              <p className="text-xs text-dark-500 mt-1">One per month to keep</p>
            </div>
          </div>
        </div>
      )}

      {/* No Policy Info */}
      {!data.retentionPolicyId && !createNew && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-xl p-4">
          <div className="flex items-start gap-3">
            <Archive className="w-5 h-5 text-yellow-400 mt-0.5" />
            <div>
              <h4 className="text-yellow-400 font-medium">No Retention Policy</h4>
              <p className="text-sm text-dark-300 mt-1">
                Without a retention policy, all backups will be kept indefinitely.
                This may consume significant disk space over time.
              </p>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
