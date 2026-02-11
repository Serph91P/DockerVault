import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  X,
  Save,
  Loader2,
  Calendar,
  Archive,
  Cloud,
  Zap,
  Terminal,
  ChevronDown,
  ChevronUp,
  FolderMinus,
} from 'lucide-react'
import {
  targetsApi,
  schedulesApi,
  retentionApi,
  storageApi,
  BackupTarget,
  ScheduleEntity,
  RetentionPolicy,
  RemoteStorage,
} from '../api'
import toast from 'react-hot-toast'
import { clsx } from 'clsx'

interface BackupEditDialogProps {
  target: BackupTarget
  isOpen: boolean
  onClose: () => void
}

// Compression options
const COMPRESSION_OPTIONS = [
  { value: 'none', label: 'None', description: 'No compression' },
  { value: 'gzip', label: 'Gzip', description: 'Good balance' },
  { value: 'zstd', label: 'Zstandard', description: 'Best ratio' },
]

export default function BackupEditDialog({ target, isOpen, onClose }: BackupEditDialogProps) {
  const queryClient = useQueryClient()
  const [showAdvanced, setShowAdvanced] = useState(false)
  
  // Helper to create initial form data from target
  const createInitialFormData = () => ({
    name: target.name,
    enabled: target.enabled,
    schedule_id: target.schedule_id || null,
    retention_policy_id: target.retention_policy_id || null,
    remote_storage_ids: [] as number[],
    compression: target.compression_enabled ? 'gzip' : 'none',
    exclude_paths: target.exclude_paths || [],
    pre_backup_command: target.pre_backup_command || '',
    post_backup_command: target.post_backup_command || '',
    stop_container: target.stop_container,
  })
  
  // Form state
  const [formData, setFormData] = useState(createInitialFormData)
  
  // Track last target id to reset form when target changes  
  const [lastTargetId, setLastTargetId] = useState(target.id)
  if (target.id !== lastTargetId) {
    setFormData(createInitialFormData())
    setLastTargetId(target.id)
  }

  // New exclude path input
  const [newExcludePath, setNewExcludePath] = useState('')

  // Fetch schedules
  const { data: schedules = [] } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
    enabled: isOpen,
  })

  // Fetch retention policies
  const { data: retentionPolicies = [] } = useQuery({
    queryKey: ['retention-policies'],
    queryFn: () => retentionApi.listPolicies().then((r) => r.data),
    enabled: isOpen,
  })

  // Fetch remote storages
  const { data: remoteStorages = [] } = useQuery({
    queryKey: ['remote-storages'],
    queryFn: () => storageApi.list().then((r) => r.data),
    enabled: isOpen,
  })

  // Update mutation
  const updateMutation = useMutation({
    mutationFn: (data: Partial<BackupTarget>) => targetsApi.update(target.id, data),
    onSuccess: () => {
      toast.success('Backup configuration updated')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      onClose()
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to update backup')
    },
  })

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    
    updateMutation.mutate({
      name: formData.name,
      enabled: formData.enabled,
      schedule_id: formData.schedule_id || undefined,
      retention_policy_id: formData.retention_policy_id || undefined,
      compression_enabled: formData.compression !== 'none',
      exclude_paths: formData.exclude_paths,
      pre_backup_command: formData.pre_backup_command || undefined,
      post_backup_command: formData.post_backup_command || undefined,
      stop_container: formData.stop_container,
    })
  }

  const addExcludePath = () => {
    if (newExcludePath && !formData.exclude_paths.includes(newExcludePath)) {
      setFormData((prev) => ({
        ...prev,
        exclude_paths: [...prev.exclude_paths, newExcludePath],
      }))
      setNewExcludePath('')
    }
  }

  const removeExcludePath = (path: string) => {
    setFormData((prev) => ({
      ...prev,
      exclude_paths: prev.exclude_paths.filter((p) => p !== path),
    }))
  }

  const toggleStorage = (storageId: number) => {
    setFormData((prev) => ({
      ...prev,
      remote_storage_ids: prev.remote_storage_ids.includes(storageId)
        ? prev.remote_storage_ids.filter((id) => id !== storageId)
        : [...prev.remote_storage_ids, storageId],
    }))
  }

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />

      {/* Dialog */}
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 w-full max-w-2xl max-h-[90vh] overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-dark-700">
          <div>
            <h2 className="text-xl font-bold text-dark-100">Edit Backup</h2>
            <p className="text-sm text-dark-400 capitalize">
              {target.target_type}: {target.container_name || target.volume_name || target.stack_name || target.host_path}
            </p>
          </div>
          <button
            onClick={onClose}
            className="p-2 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto p-6 space-y-6">
          {/* Name & Status */}
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Name</label>
              <input
                type="text"
                value={formData.name}
                onChange={(e) => setFormData((prev) => ({ ...prev, name: e.target.value }))}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500"
                required
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Status</label>
              <button
                type="button"
                onClick={() => setFormData((prev) => ({ ...prev, enabled: !prev.enabled }))}
                className={clsx(
                  'w-full px-3 py-2 rounded-lg text-left transition-colors border',
                  formData.enabled
                    ? 'bg-green-500/10 border-green-500/30 text-green-400'
                    : 'bg-yellow-500/10 border-yellow-500/30 text-yellow-400'
                )}
              >
                {formData.enabled ? '✓ Enabled' : '⏸ Disabled'}
              </button>
            </div>
          </div>

          {/* Schedule */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <Calendar className="w-4 h-4" />
              Schedule
            </label>
            <select
              value={formData.schedule_id || ''}
              onChange={(e) =>
                setFormData((prev) => ({ ...prev, schedule_id: e.target.value ? parseInt(e.target.value) : null }))
              }
              className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500"
            >
              <option value="">No schedule (manual only)</option>
              {schedules.map((schedule: ScheduleEntity) => (
                <option key={schedule.id} value={schedule.id}>
                  {schedule.name} ({schedule.cron_expression})
                </option>
              ))}
            </select>
          </div>

          {/* Retention Policy */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <Archive className="w-4 h-4" />
              Retention Policy
            </label>
            <select
              value={formData.retention_policy_id || ''}
              onChange={(e) =>
                setFormData((prev) => ({
                  ...prev,
                  retention_policy_id: e.target.value ? parseInt(e.target.value) : null,
                }))
              }
              className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500"
            >
              <option value="">Keep all backups</option>
              {retentionPolicies.map((policy: RetentionPolicy) => (
                <option key={policy.id} value={policy.id}>
                  {policy.name}
                </option>
              ))}
            </select>
          </div>

          {/* Remote Storage */}
          {remoteStorages.filter((s: RemoteStorage) => s.enabled).length > 0 && (
            <div>
              <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
                <Cloud className="w-4 h-4" />
                Remote Storage (optional)
              </label>
              <div className="flex flex-wrap gap-2">
                {remoteStorages
                  .filter((s: RemoteStorage) => s.enabled)
                  .map((storage: RemoteStorage) => (
                    <button
                      key={storage.id}
                      type="button"
                      onClick={() => toggleStorage(storage.id)}
                      className={clsx(
                        'px-3 py-1.5 rounded-lg text-sm transition-colors border',
                        formData.remote_storage_ids.includes(storage.id)
                          ? 'bg-primary-500/20 border-primary-500/50 text-primary-300'
                          : 'bg-dark-700 border-dark-600 text-dark-300 hover:border-dark-500'
                      )}
                    >
                      {storage.name}
                    </button>
                  ))}
              </div>
            </div>
          )}

          {/* Compression */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <Zap className="w-4 h-4" />
              Compression
            </label>
            <div className="flex gap-2">
              {COMPRESSION_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  onClick={() => setFormData((prev) => ({ ...prev, compression: option.value }))}
                  className={clsx(
                    'flex-1 px-3 py-2 rounded-lg text-sm transition-colors border',
                    formData.compression === option.value
                      ? 'bg-primary-500/20 border-primary-500/50 text-primary-300'
                      : 'bg-dark-700 border-dark-600 text-dark-300 hover:border-dark-500'
                  )}
                >
                  <span className="font-medium">{option.label}</span>
                  <span className="block text-xs text-dark-400">{option.description}</span>
                </button>
              ))}
            </div>
          </div>

          {/* Stop Container Option */}
          {(target.target_type === 'container' || target.target_type === 'stack') && (
            <div className="flex items-center gap-3 p-3 bg-dark-750 rounded-lg">
              <input
                type="checkbox"
                id="stop-container"
                checked={formData.stop_container}
                onChange={(e) => setFormData((prev) => ({ ...prev, stop_container: e.target.checked }))}
                className="w-4 h-4 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
              />
              <label htmlFor="stop-container" className="text-sm text-dark-200 cursor-pointer">
                <span className="font-medium">Stop container during backup</span>
                <span className="block text-xs text-dark-400">Ensures data consistency but causes downtime</span>
              </label>
            </div>
          )}

          {/* Exclude Paths */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <FolderMinus className="w-4 h-4" />
              Exclude Paths
            </label>
            <div className="flex gap-2 mb-2">
              <input
                type="text"
                value={newExcludePath}
                onChange={(e) => setNewExcludePath(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && (e.preventDefault(), addExcludePath())}
                placeholder="e.g., *.log, cache/, node_modules"
                className="flex-1 px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500 text-sm"
              />
              <button
                type="button"
                onClick={addExcludePath}
                className="px-3 py-2 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded-lg transition-colors text-sm"
              >
                Add
              </button>
            </div>
            {formData.exclude_paths.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {formData.exclude_paths.map((path) => (
                  <span
                    key={path}
                    className="inline-flex items-center gap-1 px-2 py-1 bg-orange-500/10 text-orange-400 rounded text-sm"
                  >
                    {path}
                    <button
                      type="button"
                      onClick={() => removeExcludePath(path)}
                      className="hover:text-orange-300"
                    >
                      <X className="w-3 h-3" />
                    </button>
                  </span>
                ))}
              </div>
            )}
          </div>

          {/* Advanced Options Toggle */}
          <button
            type="button"
            onClick={() => setShowAdvanced(!showAdvanced)}
            className="w-full flex items-center justify-between p-3 bg-dark-750 rounded-lg hover:bg-dark-700 transition-colors"
          >
            <div className="flex items-center gap-2">
              <Terminal className="w-4 h-4 text-dark-400" />
              <span className="text-sm text-dark-300">Advanced: Pre/Post Commands</span>
            </div>
            {showAdvanced ? (
              <ChevronUp className="w-4 h-4 text-dark-400" />
            ) : (
              <ChevronDown className="w-4 h-4 text-dark-400" />
            )}
          </button>

          {showAdvanced && (
            <div className="space-y-4 pl-4 border-l-2 border-dark-700">
              <div>
                <label className="block text-sm font-medium text-dark-300 mb-2">
                  Pre-backup command
                </label>
                <input
                  type="text"
                  value={formData.pre_backup_command}
                  onChange={(e) =>
                    setFormData((prev) => ({ ...prev, pre_backup_command: e.target.value }))
                  }
                  placeholder="e.g., docker exec db mysqldump -u root > /backup/dump.sql"
                  className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500 text-sm font-mono"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-300 mb-2">
                  Post-backup command
                </label>
                <input
                  type="text"
                  value={formData.post_backup_command}
                  onChange={(e) =>
                    setFormData((prev) => ({ ...prev, post_backup_command: e.target.value }))
                  }
                  placeholder="e.g., rm /backup/dump.sql"
                  className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:border-primary-500 text-sm font-mono"
                />
              </div>
            </div>
          )}
        </form>

        {/* Footer */}
        <div className="flex items-center justify-end gap-3 px-6 py-4 border-t border-dark-700 bg-dark-850">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-2 text-dark-300 hover:text-dark-100 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleSubmit}
            disabled={updateMutation.isPending || !formData.name}
            className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors disabled:opacity-50"
          >
            {updateMutation.isPending ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Save className="w-4 h-4" />
            )}
            Save Changes
          </button>
        </div>
      </div>
    </div>
  )
}
