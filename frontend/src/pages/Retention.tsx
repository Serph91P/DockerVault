import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Trash2, Plus, Edit, Save, X } from 'lucide-react'
import { retentionApi, RetentionPolicy } from '../api'
import toast from 'react-hot-toast'
import { useState } from 'react'

function PolicyCard({ policy }: { policy: RetentionPolicy }) {
  const queryClient = useQueryClient()
  const [isEditing, setIsEditing] = useState(false)
  const [formData, setFormData] = useState({
    keep_daily: policy.keep_daily,
    keep_weekly: policy.keep_weekly,
    keep_monthly: policy.keep_monthly,
    keep_yearly: policy.keep_yearly,
    max_age_days: policy.max_age_days,
  })

  const updateMutation = useMutation({
    mutationFn: () => retentionApi.updatePolicy(policy.id, formData),
    onSuccess: () => {
      toast.success('Policy updated')
      queryClient.invalidateQueries({ queryKey: ['retention-policies'] })
      setIsEditing(false)
    },
    onError: () => toast.error('Failed to update policy'),
  })

  const deleteMutation = useMutation({
    mutationFn: () => retentionApi.deletePolicy(policy.id),
    onSuccess: () => {
      toast.success('Policy deleted')
      queryClient.invalidateQueries({ queryKey: ['retention-policies'] })
    },
    onError: (err: Error) => toast.error(err.message || 'Failed to delete policy'),
  })

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div>
          <h3 className="text-lg font-semibold text-dark-100">{policy.name}</h3>
          <p className="text-sm text-dark-400">
            Max. {policy.max_age_days} days retention
          </p>
        </div>
        <div className="flex gap-2">
          {isEditing ? (
            <>
              <button
                onClick={() => updateMutation.mutate()}
                className="p-2 text-green-400 hover:bg-dark-700 rounded-lg"
              >
                <Save className="w-4 h-4" />
              </button>
              <button
                onClick={() => setIsEditing(false)}
                className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
              >
                <X className="w-4 h-4" />
              </button>
            </>
          ) : (
            <>
              <button
                onClick={() => setIsEditing(true)}
                className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
              >
                <Edit className="w-4 h-4" />
              </button>
              {policy.name !== 'default' && (
                <button
                  onClick={() => deleteMutation.mutate()}
                  className="p-2 text-red-400 hover:bg-dark-700 rounded-lg"
                >
                  <Trash2 className="w-4 h-4" />
                </button>
              )}
            </>
          )}
        </div>
      </div>

      {/* Retention Settings */}
      <div className="grid grid-cols-2 gap-4">
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Daily</label>
          {isEditing ? (
            <input
              type="number"
              value={formData.keep_daily}
              onChange={(e) =>
                setFormData({ ...formData, keep_daily: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          ) : (
            <p className="text-lg font-semibold text-dark-100">{policy.keep_daily}</p>
          )}
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Weekly</label>
          {isEditing ? (
            <input
              type="number"
              value={formData.keep_weekly}
              onChange={(e) =>
                setFormData({ ...formData, keep_weekly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          ) : (
            <p className="text-lg font-semibold text-dark-100">{policy.keep_weekly}</p>
          )}
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Monthly</label>
          {isEditing ? (
            <input
              type="number"
              value={formData.keep_monthly}
              onChange={(e) =>
                setFormData({ ...formData, keep_monthly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          ) : (
            <p className="text-lg font-semibold text-dark-100">{policy.keep_monthly}</p>
          )}
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Yearly</label>
          {isEditing ? (
            <input
              type="number"
              value={formData.keep_yearly}
              onChange={(e) =>
                setFormData({ ...formData, keep_yearly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          ) : (
            <p className="text-lg font-semibold text-dark-100">{policy.keep_yearly}</p>
          )}
        </div>
      </div>

      {isEditing && (
        <div className="mt-4">
          <label className="block text-xs text-dark-400 mb-1">Max. Alter (Tage)</label>
          <input
            type="number"
            value={formData.max_age_days}
            onChange={(e) =>
              setFormData({ ...formData, max_age_days: parseInt(e.target.value) })
            }
            className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
          />
        </div>
      )}
    </div>
  )
}

function CreatePolicyForm({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient()
  const [formData, setFormData] = useState({
    name: '',
    keep_daily: 7,
    keep_weekly: 4,
    keep_monthly: 6,
    keep_yearly: 2,
    max_age_days: 365,
  })

  const createMutation = useMutation({
    mutationFn: () => retentionApi.createPolicy(formData),
    onSuccess: () => {
      toast.success('Policy created')
      queryClient.invalidateQueries({ queryKey: ['retention-policies'] })
      onClose()
    },
    onError: () => toast.error('Failed to create policy'),
  })

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <h3 className="text-lg font-semibold text-dark-100 mb-4">New Retention Policy</h3>

      <div className="space-y-4">
        <div>
          <label className="block text-xs text-dark-400 mb-1">Name</label>
          <input
            type="text"
            value={formData.name}
            onChange={(e) => setFormData({ ...formData, name: e.target.value })}
            className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            placeholder="e.g. aggressive, conservative"
          />
        </div>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-xs text-dark-400 mb-1">Daily</label>
            <input
              type="number"
              value={formData.keep_daily}
              onChange={(e) =>
                setFormData({ ...formData, keep_daily: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>
          <div>
            <label className="block text-xs text-dark-400 mb-1">Weekly</label>
            <input
              type="number"
              value={formData.keep_weekly}
              onChange={(e) =>
                setFormData({ ...formData, keep_weekly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>
          <div>
            <label className="block text-xs text-dark-400 mb-1">Monthly</label>
            <input
              type="number"
              value={formData.keep_monthly}
              onChange={(e) =>
                setFormData({ ...formData, keep_monthly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>
          <div>
            <label className="block text-xs text-dark-400 mb-1">Yearly</label>
            <input
              type="number"
              value={formData.keep_yearly}
              onChange={(e) =>
                setFormData({ ...formData, keep_yearly: parseInt(e.target.value) })
              }
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>
        </div>

        <div>
          <label className="block text-xs text-dark-400 mb-1">Max Age (Days)</label>
          <input
            type="number"
            value={formData.max_age_days}
            onChange={(e) =>
              setFormData({ ...formData, max_age_days: parseInt(e.target.value) })
            }
            className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
          />
        </div>

        <div className="flex gap-2">
          <button
            onClick={() => createMutation.mutate()}
            disabled={!formData.name || createMutation.isPending}
            className="flex-1 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 disabled:opacity-50"
          >
            Create
          </button>
          <button
            onClick={onClose}
            className="px-4 py-2 bg-dark-600 text-dark-300 rounded-lg hover:bg-dark-500"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  )
}

export default function Retention() {
  const [showCreate, setShowCreate] = useState(false)

  const { data: policies, isLoading } = useQuery({
    queryKey: ['retention-policies'],
    queryFn: () => retentionApi.listPolicies().then((r) => r.data),
  })

  const cleanupMutation = useMutation({
    mutationFn: () => retentionApi.cleanupOrphaned(),
    onSuccess: (data) => {
      toast.success(`${data.data.deleted} orphaned files deleted`)
    },
    onError: () => toast.error('Failed to cleanup'),
  })

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Retention Policies</h1>
          <p className="text-dark-400 mt-1">Configure how long backups are retained</p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => cleanupMutation.mutate()}
            disabled={cleanupMutation.isPending}
            className="flex items-center gap-2 px-4 py-2 bg-dark-700 text-dark-200 rounded-lg hover:bg-dark-600 transition-colors"
          >
            <Trash2 className="w-4 h-4" />
            Cleanup
          </button>
          <button
            onClick={() => setShowCreate(true)}
            className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
          >
            <Plus className="w-4 h-4" />
            New Policy
          </button>
        </div>
      </div>

      {/* Info Box */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
        <h2 className="text-lg font-semibold text-dark-100 mb-2">
          Grandfather-Father-Son (GFS) Strategy
        </h2>
        <p className="text-sm text-dark-400">
          The retention policy uses the GFS strategy for intelligent backup retention:
        </p>
        <ul className="mt-2 space-y-1 text-sm text-dark-400">
          <li>
            • <span className="text-dark-200">Daily:</span> Keeps the last N daily backups
          </li>
          <li>
            • <span className="text-dark-200">Weekly:</span> Keeps one backup per week for the
            last N weeks
          </li>
          <li>
            • <span className="text-dark-200">Monthly:</span> Keeps one backup per month for the
            last N months
          </li>
          <li>
            • <span className="text-dark-200">Yearly:</span> Keeps one backup per year for the
            last N years
          </li>
        </ul>
      </div>

      {/* Create Form */}
      {showCreate && <CreatePolicyForm onClose={() => setShowCreate(false)} />}

      {/* Policies Grid */}
      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {[...Array(2)].map((_, i) => (
            <div
              key={i}
              className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse"
            >
              <div className="h-6 bg-dark-700 rounded w-1/2 mb-2" />
              <div className="h-4 bg-dark-700 rounded w-3/4" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {policies?.map((policy) => (
            <PolicyCard key={policy.id} policy={policy} />
          ))}
        </div>
      )}
    </div>
  )
}
