import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Target, Trash2, Play, Clock, Calendar, X, Check, Plus, Archive, Pencil } from 'lucide-react'
import { targetsApi, backupsApi, schedulesApi, BackupTarget, ScheduleEntity } from '../api'
import toast from 'react-hot-toast'
import { useState } from 'react'
import BackupWizard from '../components/BackupWizard'
import ConfirmDialog from '../components/ConfirmDialog'

function TargetCard({ target, schedules, onEdit }: { target: BackupTarget; schedules: ScheduleEntity[]; onEdit: (target: BackupTarget) => void }) {
  const queryClient = useQueryClient()
  const [editingSchedule, setEditingSchedule] = useState(false)
  const [selectedScheduleId, setSelectedScheduleId] = useState<number | undefined>(target.schedule_id)
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false)

  const triggerBackupMutation = useMutation({
    mutationFn: () => backupsApi.create(target.id),
    onSuccess: () => {
      toast.success('Backup started')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: (error: Error & { response?: { data?: { detail?: string } } }) => {
      const message = error.response?.data?.detail || 'Failed to start backup'
      toast.error(message)
    },
  })

  const toggleMutation = useMutation({
    mutationFn: () => targetsApi.update(target.id, { enabled: !target.enabled }),
    onSuccess: () => {
      toast.success(target.enabled ? 'Target disabled' : 'Target enabled')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
    },
    onError: () => toast.error('Failed to update target'),
  })

  const updateScheduleMutation = useMutation({
    mutationFn: (schedule_id: number | undefined) => 
      targetsApi.update(target.id, { schedule_id: schedule_id ?? null } as Partial<BackupTarget>),
    onSuccess: () => {
      toast.success('Schedule updated')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      setEditingSchedule(false)
    },
    onError: () => toast.error('Failed to update schedule'),
  })

  const deleteMutation = useMutation({
    mutationFn: () => targetsApi.delete(target.id),
    onSuccess: () => {
      toast.success('Target deleted')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
    },
    onError: () => toast.error('Failed to delete target'),
  })

  const handleSaveSchedule = () => {
    updateScheduleMutation.mutate(selectedScheduleId)
  }

  const handleCancelSchedule = () => {
    setSelectedScheduleId(target.schedule_id)
    setEditingSchedule(false)
  }

  const currentSchedule = target.schedule || schedules.find(s => s.id === target.schedule_id)

  const getTargetTypeIcon = () => {
    switch (target.target_type) {
      case 'container':
        return '🐳'
      case 'volume':
        return '💾'
      case 'path':
        return '📁'
      case 'stack':
        return '📚'
      default:
        return '📦'
    }
  }

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-dark-700 rounded-lg flex items-center justify-center text-xl">
            {getTargetTypeIcon()}
          </div>
          <div>
            <h3 className="text-lg font-semibold text-dark-100">{target.name}</h3>
            <p className="text-sm text-dark-400 capitalize">{target.target_type}</p>
          </div>
        </div>
        <button
          onClick={() => toggleMutation.mutate()}
          className={`px-2 py-1 rounded-full text-xs font-medium ${
            target.enabled
              ? 'bg-green-500/10 text-green-400'
              : 'bg-dark-600 text-dark-400'
          }`}
        >
          {target.enabled ? 'Active' : 'Inactive'}
        </button>
      </div>

      {/* Target Details */}
      <div className="space-y-2 mb-4 text-sm">
        {target.container_name && (
          <div>
            <span className="text-dark-400">Container: </span>
            <span className="text-dark-200">{target.container_name}</span>
          </div>
        )}
        {target.volume_name && (
          <div>
            <span className="text-dark-400">Volume: </span>
            <span className="text-dark-200">{target.volume_name}</span>
          </div>
        )}
        {target.host_path && (
          <div>
            <span className="text-dark-400">Path: </span>
            <span className="text-dark-200 font-mono">{target.host_path}</span>
          </div>
        )}
        {target.stack_name && (
          <div>
            <span className="text-dark-400">Stack: </span>
            <span className="text-dark-200">{target.stack_name}</span>
          </div>
        )}
        
        {/* Schedule Editor */}
        <div className="pt-2 border-t border-dark-700">
          <div className="flex items-center gap-1 mb-2">
            <Calendar className="w-3 h-3 text-dark-400" />
            <span className="text-dark-400">Schedule: </span>
          </div>
          {editingSchedule ? (
            <div className="flex gap-2">
              <select
                value={selectedScheduleId || ''}
                onChange={(e) => setSelectedScheduleId(e.target.value ? Number(e.target.value) : undefined)}
                className="flex-1 px-2 py-1 bg-dark-700 border border-dark-600 rounded text-sm text-dark-100"
              >
                <option value="">No schedule</option>
                {schedules.map((schedule) => (
                  <option key={schedule.id} value={schedule.id}>
                    {schedule.name} ({schedule.cron_expression})
                  </option>
                ))}
              </select>
              <button
                onClick={handleSaveSchedule}
                disabled={updateScheduleMutation.isPending}
                className="p-1 bg-green-500/20 text-green-400 rounded hover:bg-green-500/30"
              >
                <Check className="w-4 h-4" />
              </button>
              <button
                onClick={handleCancelSchedule}
                className="p-1 bg-dark-600 text-dark-300 rounded hover:bg-dark-500"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
          ) : (
            <button
              onClick={() => setEditingSchedule(true)}
              className="flex items-center gap-2 px-2 py-1 bg-dark-700 rounded text-sm hover:bg-dark-600 transition-colors w-full text-left"
            >
              {currentSchedule ? (
                <div className="flex-1">
                  <span className="text-primary-400">{currentSchedule.name}</span>
                  <span className="text-dark-500 ml-2 font-mono text-xs">
                    {currentSchedule.cron_expression}
                  </span>
                </div>
              ) : target.schedule_cron ? (
                <span className="text-orange-400 font-mono text-xs flex-1">
                  Legacy: {target.schedule_cron}
                </span>
              ) : (
                <span className="text-dark-500 italic flex-1">No schedule</span>
              )}
              <Clock className="w-3 h-3 text-dark-400" />
            </button>
          )}
        </div>
      </div>

      {/* Settings */}
      <div className="flex flex-wrap gap-2 mb-4">
        {target.retention_policy && (
          <span className="text-xs bg-green-500/10 text-green-400 rounded px-2 py-1 flex items-center gap-1">
            <Archive className="w-3 h-3" />
            {target.retention_policy.name}
          </span>
        )}
        {target.stop_container && (
          <span className="text-xs bg-orange-500/10 text-orange-400 rounded px-2 py-1">
            Stops Container
          </span>
        )}
        {target.compression_enabled && (
          <span className="text-xs bg-blue-500/10 text-blue-400 rounded px-2 py-1">
            Compressed
          </span>
        )}
        {target.dependencies.length > 0 && (
          <span className="text-xs bg-purple-500/10 text-purple-400 rounded px-2 py-1">
            {target.dependencies.length} Dependencies
          </span>
        )}
      </div>

      {/* Actions */}
      <div className="flex gap-2">
        <button
          onClick={() => triggerBackupMutation.mutate()}
          disabled={triggerBackupMutation.isPending || !target.enabled}
          className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-primary-500/10 text-primary-400 rounded-lg hover:bg-primary-500/20 transition-colors text-sm disabled:opacity-50"
        >
          <Play className="w-4 h-4" />
          Start Backup
        </button>
        <button
          onClick={() => onEdit(target)}
          className="px-3 py-2 bg-dark-700 text-dark-300 rounded-lg hover:bg-dark-600 hover:text-dark-100 transition-colors"
          title="Edit target"
        >
          <Pencil className="w-4 h-4" />
        </button>
        <button
          onClick={() => setShowDeleteConfirm(true)}
          disabled={deleteMutation.isPending}
          className="px-3 py-2 bg-red-500/10 text-red-400 rounded-lg hover:bg-red-500/20 transition-colors"
        >
          <Trash2 className="w-4 h-4" />
        </button>
      </div>

      <ConfirmDialog
        isOpen={showDeleteConfirm}
        onClose={() => setShowDeleteConfirm(false)}
        onConfirm={() => {
          deleteMutation.mutate()
          setShowDeleteConfirm(false)
        }}
        title="Delete Target"
        message={`Delete target "${target.name}"? This cannot be undone. Existing backups will be kept.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deleteMutation.isPending}
      />
    </div>
  )
}

export default function Targets() {
  const [wizardOpen, setWizardOpen] = useState(false)
  const [editingTarget, setEditingTarget] = useState<BackupTarget | null>(null)
  
  const { data: targets, isLoading } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  const { data: schedules = [] } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })

  const handleEdit = (target: BackupTarget) => {
    setEditingTarget(target)
    setWizardOpen(true)
  }

  const handleWizardClose = () => {
    setWizardOpen(false)
    setEditingTarget(null)
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Backup Targets</h1>
          <p className="text-dark-400 mt-1">Configured backup targets</p>
        </div>
        <button
          onClick={() => { setEditingTarget(null); setWizardOpen(true) }}
          className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
        >
          <Plus className="w-4 h-4" />
          New Target
        </button>
      </div>

      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse">
              <div className="h-6 bg-dark-700 rounded w-1/2 mb-2" />
              <div className="h-4 bg-dark-700 rounded w-3/4" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {targets?.map((target) => (
            <TargetCard key={target.id} target={target} schedules={schedules} onEdit={handleEdit} />
          ))}
        </div>
      )}

      {!isLoading && targets?.length === 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
          <Target className="w-12 h-12 text-dark-500 mx-auto mb-4" />
          <p className="text-dark-400">No backup targets configured</p>
          <p className="text-sm text-dark-500 mt-2">
            Click "New Target" to create your first backup target
          </p>
          <button
            onClick={() => { setEditingTarget(null); setWizardOpen(true) }}
            className="mt-4 flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors mx-auto"
          >
            <Plus className="w-4 h-4" />
            Create Backup Target
          </button>
        </div>
      )}

      {/* Backup Wizard Modal */}
      <BackupWizard isOpen={wizardOpen} onClose={handleWizardClose} editTarget={editingTarget} />
    </div>
  )
}
