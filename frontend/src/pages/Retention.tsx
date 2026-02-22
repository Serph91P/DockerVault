import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Trash2, Plus, Edit, Target, ChevronDown, ChevronRight } from 'lucide-react'
import { retentionApi, targetsApi, RetentionPolicy, BackupTarget } from '../api'
import toast from 'react-hot-toast'
import { useState } from 'react'
import ConfirmDialog from '../components/ConfirmDialog'

function PolicyCard({ policy, targets, onEdit }: { policy: RetentionPolicy; targets: BackupTarget[]; onEdit: (policy: RetentionPolicy) => void }) {
  const queryClient = useQueryClient()
  const [showConfirm, setShowConfirm] = useState(false)

  // RE2: Count which targets use this policy
  const usedByTargets = targets.filter((t) => t.retention_policy_id === policy.id)

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
          {/* RE2: Used by badge */}
          <div className="flex items-center gap-1 mt-1">
            <Target className="w-3 h-3 text-dark-500" />
            {usedByTargets.length > 0 ? (
              <span className="text-xs text-dark-400" title={usedByTargets.map(t => t.name).join(', ')}>
                Used by {usedByTargets.length} target{usedByTargets.length !== 1 ? 's' : ''}
              </span>
            ) : (
              <span className="text-xs text-dark-500">Not in use</span>
            )}
          </div>
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => onEdit(policy)}
            className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
          >
            <Edit className="w-4 h-4" />
          </button>
          {policy.name !== 'default' && (
            <button
              onClick={() => setShowConfirm(true)}
              className="p-2 text-red-400 hover:bg-dark-700 rounded-lg"
            >
              <Trash2 className="w-4 h-4" />
            </button>
          )}
        </div>
      </div>

      {/* Retention Settings (display only) */}
      <div className="grid grid-cols-2 gap-4">
        <div className="col-span-2">
          <label className="block text-xs text-dark-400 mb-1">Keep Last</label>
          <p className="text-lg font-semibold text-dark-100">{policy.keep_last || 0}</p>
          <p className="text-xs text-dark-500 mt-0.5">Always keep the last N backups regardless of age</p>
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Daily</label>
          <p className="text-lg font-semibold text-dark-100">{policy.keep_daily}</p>
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Weekly</label>
          <p className="text-lg font-semibold text-dark-100">{policy.keep_weekly}</p>
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Monthly</label>
          <p className="text-lg font-semibold text-dark-100">{policy.keep_monthly}</p>
        </div>
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Yearly</label>
          <p className="text-lg font-semibold text-dark-100">{policy.keep_yearly}</p>
        </div>
      </div>

      {/* RE1: ConfirmDialog for delete */}
      <ConfirmDialog
        isOpen={showConfirm}
        onClose={() => setShowConfirm(false)}
        onConfirm={() => {
          deleteMutation.mutate()
          setShowConfirm(false)
        }}
        title="Delete Retention Policy"
        message={`Delete policy "${policy.name}"?${usedByTargets.length > 0 ? ` It is currently used by ${usedByTargets.length} target(s): ${usedByTargets.map(t => t.name).join(', ')}.` : ''}`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deleteMutation.isPending}
      />
    </div>
  )
}

function EditPolicyModal({ policy, onClose }: { policy: RetentionPolicy; onClose: () => void }) {
  const queryClient = useQueryClient()
  const [formData, setFormData] = useState({
    keep_last: policy.keep_last,
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
      onClose()
    },
    onError: () => toast.error('Failed to update policy'),
  })

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 shadow-2xl w-full max-w-lg p-6">
        <h2 className="text-lg font-semibold text-dark-100 mb-4">Edit Policy — {policy.name}</h2>

        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-1">Keep Last</label>
            <input
              type="number"
              min={0}
              value={formData.keep_last}
              onChange={(e) => setFormData({ ...formData, keep_last: parseInt(e.target.value) || 0 })}
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
            <p className="text-xs text-dark-500 mt-0.5">Always keep the last N backups regardless of age</p>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-1">Keep Daily</label>
              <input
                type="number"
                min={0}
                value={formData.keep_daily}
                onChange={(e) => setFormData({ ...formData, keep_daily: parseInt(e.target.value) || 0 })}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-1">Keep Weekly</label>
              <input
                type="number"
                min={0}
                value={formData.keep_weekly}
                onChange={(e) => setFormData({ ...formData, keep_weekly: parseInt(e.target.value) || 0 })}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-1">Keep Monthly</label>
              <input
                type="number"
                min={0}
                value={formData.keep_monthly}
                onChange={(e) => setFormData({ ...formData, keep_monthly: parseInt(e.target.value) || 0 })}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-1">Keep Yearly</label>
              <input
                type="number"
                min={0}
                value={formData.keep_yearly}
                onChange={(e) => setFormData({ ...formData, keep_yearly: parseInt(e.target.value) || 0 })}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
              />
            </div>
          </div>

          <div>
            <label className="block text-sm font-medium text-dark-300 mb-1">Max Age (Days)</label>
            <input
              type="number"
              min={0}
              value={formData.max_age_days}
              onChange={(e) => setFormData({ ...formData, max_age_days: parseInt(e.target.value) || 0 })}
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>
        </div>

        <div className="flex gap-2 mt-6">
          <button
            onClick={() => updateMutation.mutate()}
            disabled={updateMutation.isPending}
            className="flex-1 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 disabled:opacity-50 transition-colors"
          >
            {updateMutation.isPending ? 'Saving...' : 'Save'}
          </button>
          <button
            onClick={onClose}
            className="px-4 py-2 bg-dark-600 text-dark-300 rounded-lg hover:bg-dark-500 transition-colors"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  )
}

function CreatePolicyForm({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient()
  const [formData, setFormData] = useState({
    name: '',
    keep_last: 0,
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
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 shadow-2xl w-full max-w-lg p-6">
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

        {/* RE6: Keep Last field */}
        <div>
          <label className="block text-xs text-dark-400 mb-1">Keep Last</label>
          <input
            type="number"
            min={0}
            value={formData.keep_last}
            onChange={(e) => setFormData({ ...formData, keep_last: parseInt(e.target.value) || 0 })}
            className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
          />
          <p className="text-xs text-dark-500 mt-0.5">Always keep the last N backups regardless of age (0 = disabled)</p>
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
    </div>
  )
}

export default function Retention() {
  const [showCreate, setShowCreate] = useState(false)
  const [showCleanupConfirm, setShowCleanupConfirm] = useState(false)
  const [editingPolicy, setEditingPolicy] = useState<RetentionPolicy | null>(null)
  const [showGfsInfo, setShowGfsInfo] = useState(false)

  const { data: policies, isLoading } = useQuery({
    queryKey: ['retention-policies'],
    queryFn: () => retentionApi.listPolicies().then((r) => r.data),
  })

  // RE2: Fetch targets to show usage info
  const { data: targets = [] } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
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
            onClick={() => setShowCleanupConfirm(true)}
            disabled={cleanupMutation.isPending}
            title="Remove orphaned backup files that are no longer referenced by any backup record"
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

      {/* Info Box — collapsible */}
      <div className="bg-dark-800 rounded-xl border border-dark-700">
        <button
          onClick={() => setShowGfsInfo(!showGfsInfo)}
          className="w-full flex items-center justify-between p-4 text-left hover:bg-dark-750 rounded-xl transition-colors"
        >
          <h2 className="text-sm font-semibold text-dark-200">
            Grandfather-Father-Son (GFS) Strategy
          </h2>
          {showGfsInfo ? (
            <ChevronDown className="w-4 h-4 text-dark-400" />
          ) : (
            <ChevronRight className="w-4 h-4 text-dark-400" />
          )}
        </button>
        {showGfsInfo && (
          <div className="px-4 pb-4">
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
        )}
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
            <PolicyCard key={policy.id} policy={policy} targets={targets} onEdit={setEditingPolicy} />
          ))}
        </div>
      )}

      <ConfirmDialog
        isOpen={showCleanupConfirm}
        onClose={() => setShowCleanupConfirm(false)}
        onConfirm={() => {
          cleanupMutation.mutate()
          setShowCleanupConfirm(false)
        }}
        title="Cleanup Orphaned Files"
        message="Delete all backup files that are not linked to any backup record? This cannot be undone."
        confirmLabel="Cleanup"
        confirmVariant="danger"
        isLoading={cleanupMutation.isPending}
      />

      {editingPolicy && (
        <EditPolicyModal
          policy={editingPolicy}
          onClose={() => setEditingPolicy(null)}
        />
      )}
    </div>
  )
}
