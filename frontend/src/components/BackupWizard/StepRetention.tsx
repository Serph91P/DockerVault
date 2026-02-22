import { useState, useEffect } from 'react'
import { Archive, Plus, Clock, Trash2, Ban, BarChart3 } from 'lucide-react'
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

// R1: Visual preview of retention policy effect
function RetentionPreview({ keepLast, keepDaily, keepWeekly, keepMonthly }: {
  keepLast: number; keepDaily: number; keepWeekly: number; keepMonthly: number
}) {
  const total = keepLast + keepDaily + keepWeekly + keepMonthly
  if (total === 0) return null

  const segments = [
    { label: 'Last', value: keepLast, color: 'bg-blue-500' },
    { label: 'Daily', value: keepDaily, color: 'bg-green-500' },
    { label: 'Weekly', value: keepWeekly, color: 'bg-yellow-500' },
    { label: 'Monthly', value: keepMonthly, color: 'bg-purple-500' },
  ].filter(s => s.value > 0)

  return (
    <div className="bg-dark-900 rounded-lg p-3 space-y-2">
      <div className="flex items-center gap-2 text-xs text-dark-400">
        <BarChart3 className="w-3.5 h-3.5" />
        <span>Approx. <strong className="text-dark-200">{total}</strong> backups kept max</span>
      </div>
      <div className="flex h-2 rounded-full overflow-hidden bg-dark-700">
        {segments.map((seg) => (
          <div
            key={seg.label}
            className={`${seg.color} transition-all`}
            style={{ width: `${(seg.value / total) * 100}%` }}
          />
        ))}
      </div>
      <div className="flex flex-wrap gap-3 text-xs">
        {segments.map((seg) => (
          <div key={seg.label} className="flex items-center gap-1.5">
            <span className={`w-2 h-2 rounded-full ${seg.color}`} />
            <span className="text-dark-400">{seg.label}: {seg.value}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

export default function StepRetention({ data, updateData, policies, isLoadingPolicies }: Props) {
  const [createNew, setCreateNew] = useState(false)
  const [hasAutoSelected, setHasAutoSelected] = useState(false)

  // Smart defaults: auto-select policy based on available options
  useEffect(() => {
    if (hasAutoSelected || data.retentionPolicyId || data.newRetentionPolicy) return
    if (isLoadingPolicies) return

    if (policies.length === 0) {
      // No policies exist: default to "create new"
      setCreateNew(true)
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
    } else if (policies.length === 1) {
      // Exactly one policy: auto-select it
      updateData({ retentionPolicyId: policies[0].id, newRetentionPolicy: null })
      setCreateNew(false)
    } else {
      // Multiple policies: neutral state, user must choose
      setCreateNew(false)
    }
    setHasAutoSelected(true)
  }, [policies, isLoadingPolicies, hasAutoSelected])

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
      <div className="space-y-3">
        <label className="block text-sm font-medium text-dark-300">Select Policy</label>

        {/* Existing Policies */}
        {policies.length > 0 && (
          <div className="rounded-xl border border-dark-700 divide-y divide-dark-700 overflow-hidden">
            {policies.map((policy) => {
              const isSelected = !createNew && data.retentionPolicyId === policy.id
              const details = [
                policy.keep_last > 0 && `Last ${policy.keep_last}`,
                policy.keep_daily > 0 && `${policy.keep_daily} daily`,
                policy.keep_weekly > 0 && `${policy.keep_weekly} weekly`,
                policy.keep_monthly > 0 && `${policy.keep_monthly} monthly`,
                policy.keep_yearly > 0 && `${policy.keep_yearly} yearly`,
              ].filter(Boolean).join(' · ')
              return (
                <button
                  key={policy.id}
                  onClick={() => handlePolicySelect(policy.id.toString())}
                  className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                    isSelected
                      ? 'bg-primary-500/15'
                      : 'bg-dark-800 hover:bg-dark-750'
                  }`}
                >
                  <Archive className={`w-4 h-4 flex-shrink-0 ${isSelected ? 'text-primary-400' : 'text-dark-500'}`} />
                  <div className="min-w-0 flex-1">
                    <span className={`block text-sm font-medium truncate ${isSelected ? 'text-primary-300' : 'text-dark-200'}`}>
                      {policy.name}
                    </span>
                    {details && (
                      <span className="block text-xs text-dark-500 truncate">{details}</span>
                    )}
                  </div>
                  {isSelected && (
                    <span className="flex-shrink-0 w-2 h-2 rounded-full bg-primary-400" />
                  )}
                </button>
              )
            })}
          </div>
        )}

        {isLoadingPolicies && (
          <p className="text-sm text-dark-500 py-2">Loading policies...</p>
        )}

        {/* Action Buttons */}
        <div className="flex gap-3">
          <button
            type="button"
            onClick={() => handlePolicySelect('new')}
            className={`flex-1 flex items-center justify-center gap-2 px-4 py-3 rounded-xl border-2 border-dashed transition-all ${
              createNew
                ? 'border-primary-500 bg-primary-500/10 text-primary-300'
                : 'border-dark-600 text-dark-400 hover:border-dark-500 hover:text-dark-300'
            }`}
          >
            <Plus className="w-4 h-4" />
            <span className="text-sm font-medium">Create new policy</span>
          </button>

          <button
            type="button"
            onClick={() => handlePolicySelect('none')}
            className={`flex items-center justify-center gap-2 px-4 py-3 rounded-xl border-2 transition-all ${
              !createNew && !data.retentionPolicyId
                ? 'border-yellow-500/50 bg-yellow-500/10 text-yellow-400'
                : 'border-dark-700 text-dark-500 hover:border-dark-600 hover:text-dark-400'
            }`}
          >
            <Ban className="w-4 h-4" />
            <span className="text-sm font-medium">None</span>
          </button>
        </div>

        {/* R1: Preview for selected existing policy */}
        {!createNew && data.retentionPolicyId && (() => {
          const selected = policies.find(p => p.id === data.retentionPolicyId)
          if (!selected) return null
          return (
            <RetentionPreview
              keepLast={selected.keep_last}
              keepDaily={selected.keep_daily}
              keepWeekly={selected.keep_weekly}
              keepMonthly={selected.keep_monthly}
            />
          )
        })()}
      </div>

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

          {/* R1: Retention Visual Preview */}
          <RetentionPreview
            keepLast={data.newRetentionPolicy.keepLast}
            keepDaily={data.newRetentionPolicy.keepDaily}
            keepWeekly={data.newRetentionPolicy.keepWeekly}
            keepMonthly={data.newRetentionPolicy.keepMonthly}
          />
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
