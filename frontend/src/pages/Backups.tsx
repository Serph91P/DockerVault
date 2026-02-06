import { useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Archive,
  CheckCircle,
  XCircle,
  Clock,
  Loader2,
  Trash2,
  RotateCcw,
  Play,
  Plus,
  ChevronDown,
  ChevronUp,
  Calendar,
  FolderOpen,
} from 'lucide-react'
import {
  backupsApi,
  targetsApi,
  schedulesApi,
  Backup,
  BackupTarget,
  ScheduleEntity,
} from '../api'
import { format, formatDistanceToNow } from 'date-fns'
import toast from 'react-hot-toast'
import { useWebSocketStore } from '../store/websocket'
import { clsx } from 'clsx'
import BackupWizard from '../components/BackupWizard'
import BackupBrowser from '../components/BackupBrowser'

// Backup Row Component with browser option
function BackupRow({
  backup,
  onBrowse,
}: {
  backup: Backup
  onBrowse: (backup: Backup) => void
}) {
  const queryClient = useQueryClient()
  const backupProgress = useWebSocketStore((state) => state.backupProgress)
  const progress = backupProgress.get(backup.id)

  const deleteMutation = useMutation({
    mutationFn: () => backupsApi.delete(backup.id),
    onSuccess: () => {
      toast.success('Backup deleted')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: () => toast.error('Failed to delete backup'),
  })

  const restoreMutation = useMutation({
    mutationFn: () => backupsApi.restore(backup.id),
    onSuccess: () => {
      toast.success('Restore started')
    },
    onError: () => toast.error('Failed to restore backup'),
  })

  const getStatusIcon = () => {
    switch (backup.status) {
      case 'completed':
        return <CheckCircle className="w-4 h-4 text-green-500" />
      case 'failed':
        return <XCircle className="w-4 h-4 text-red-500" />
      case 'running':
        return <Loader2 className="w-4 h-4 text-primary-500 animate-spin" />
      case 'pending':
        return <Clock className="w-4 h-4 text-yellow-500" />
      default:
        return <Archive className="w-4 h-4 text-dark-400" />
    }
  }

  return (
    <div className="flex items-center justify-between py-3 px-4 bg-dark-750 rounded-lg hover:bg-dark-700 transition-colors">
      <div className="flex items-center gap-3">
        {getStatusIcon()}
        <div>
          <p className="text-sm text-dark-200">
            {format(new Date(backup.created_at), 'yyyy-MM-dd HH:mm')}
          </p>
          <p className="text-xs text-dark-400">
            {backup.file_size_human || '-'} •{' '}
            {backup.duration_seconds ? `${backup.duration_seconds}s` : '-'}
          </p>
        </div>
      </div>

      {progress && backup.status === 'running' && (
        <div className="flex-1 mx-4">
          <div className="w-full h-1.5 bg-dark-600 rounded-full overflow-hidden">
            <div
              className="h-full bg-primary-500 rounded-full transition-all"
              style={{ width: `${Math.max(0, progress.progress)}%` }}
            />
          </div>
          <p className="text-xs text-dark-400 mt-1">{progress.message}</p>
        </div>
      )}

      <div className="flex items-center gap-1">
        {backup.status === 'completed' && (
          <>
            <button
              onClick={() => onBrowse(backup)}
              className="p-1.5 text-dark-400 hover:text-primary-400 hover:bg-dark-600 rounded transition-colors"
              title="Browse files"
            >
              <FolderOpen className="w-4 h-4" />
            </button>
            <button
              onClick={() => restoreMutation.mutate()}
              disabled={restoreMutation.isPending}
              className="p-1.5 text-dark-400 hover:text-green-400 hover:bg-dark-600 rounded transition-colors"
              title="Restore backup"
            >
              <RotateCcw className="w-4 h-4" />
            </button>
          </>
        )}
        <button
          onClick={() => {
            if (confirm('Delete this backup?')) {
              deleteMutation.mutate()
            }
          }}
          disabled={deleteMutation.isPending}
          className="p-1.5 text-dark-400 hover:text-red-400 hover:bg-dark-600 rounded transition-colors"
          title="Delete backup"
        >
          <Trash2 className="w-4 h-4" />
        </button>
      </div>
    </div>
  )
}

// Target Card with backup list
function TargetBackupCard({
  target,
  backups,
  schedules,
  onBrowseBackup,
}: {
  target: BackupTarget
  backups: Backup[]
  schedules: ScheduleEntity[]
  onBrowseBackup: (backup: Backup) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const queryClient = useQueryClient()

  const triggerMutation = useMutation({
    mutationFn: () => backupsApi.create(target.id, 'full'),
    onSuccess: () => {
      toast.success('Backup started')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: () => toast.error('Failed to start backup'),
  })

  const toggleMutation = useMutation({
    mutationFn: () => targetsApi.update(target.id, { enabled: !target.enabled }),
    onSuccess: () => {
      toast.success(target.enabled ? 'Backup disabled' : 'Backup enabled')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to update backup'),
  })

  const deleteMutation = useMutation({
    mutationFn: () => targetsApi.delete(target.id),
    onSuccess: () => {
      toast.success('Backup configuration removed')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to remove backup'),
  })

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

  const lastBackup = backups[0]
  const schedule = target.schedule || schedules.find((s) => s.id === target.schedule_id)

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 overflow-hidden">
      {/* Header */}
      <div
        className="p-4 cursor-pointer hover:bg-dark-750 transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 bg-dark-700 rounded-lg flex items-center justify-center text-xl">
              {getTargetTypeIcon()}
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h3 className="text-base font-medium text-dark-100">{target.name}</h3>
                {!target.enabled && (
                  <span className="px-1.5 py-0.5 rounded text-xs bg-yellow-500/10 text-yellow-400">
                    Disabled
                  </span>
                )}
              </div>
              <p className="text-sm text-dark-400 capitalize">
                {target.target_type}
                {target.container_name && `: ${target.container_name}`}
                {target.volume_name && `: ${target.volume_name}`}
                {target.stack_name && `: ${target.stack_name}`}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-3">
            {/* Last Backup Status */}
            {lastBackup ? (
              <div className="flex items-center gap-2 text-sm">
                <span
                  className={clsx(
                    'px-2 py-1 rounded-full text-xs font-medium',
                    lastBackup.status === 'completed'
                      ? 'bg-green-500/10 text-green-400'
                      : lastBackup.status === 'failed'
                        ? 'bg-red-500/10 text-red-400'
                        : lastBackup.status === 'running'
                          ? 'bg-primary-500/10 text-primary-400'
                          : 'bg-dark-600 text-dark-400'
                  )}
                >
                  {formatDistanceToNow(new Date(lastBackup.created_at), { addSuffix: true })}
                </span>
              </div>
            ) : (
              <span className="px-2 py-1 rounded-full text-xs font-medium bg-dark-600 text-dark-400">
                No backups
              </span>
            )}

            {/* Backup Count */}
            <span className="px-2 py-1 rounded text-xs bg-dark-700 text-dark-300">
              {backups.length} backup{backups.length !== 1 ? 's' : ''}
            </span>

            {expanded ? (
              <ChevronUp className="w-5 h-5 text-dark-400" />
            ) : (
              <ChevronDown className="w-5 h-5 text-dark-400" />
            )}
          </div>
        </div>
      </div>

      {/* Expanded Content */}
      {expanded && (
        <div className="px-4 pb-4 border-t border-dark-700 pt-4 space-y-4">
          {/* Info Row */}
          <div className="flex flex-wrap items-center gap-2 text-sm">
            {schedule && (
              <span className="flex items-center gap-1 px-2 py-1 bg-dark-700 rounded text-dark-300">
                <Calendar className="w-3 h-3" />
                {schedule.name}
              </span>
            )}
            {target.selected_volumes && target.selected_volumes.length > 0 && (
              <span className="px-2 py-1 bg-purple-500/10 text-purple-400 rounded text-xs">
                {target.selected_volumes.length} volume(s)
              </span>
            )}
            {target.exclude_paths && target.exclude_paths.length > 0 && (
              <span className="px-2 py-1 bg-orange-500/10 text-orange-400 rounded text-xs">
                {target.exclude_paths.length} exclude(s)
              </span>
            )}
          </div>

          {/* Actions */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  toggleMutation.mutate()
                }}
                disabled={toggleMutation.isPending}
                className={clsx(
                  'px-3 py-1.5 rounded-lg text-sm transition-colors disabled:opacity-50',
                  target.enabled
                    ? 'bg-yellow-500/10 hover:bg-yellow-500/20 text-yellow-400'
                    : 'bg-green-500/10 hover:bg-green-500/20 text-green-400'
                )}
              >
                {target.enabled ? 'Disable' : 'Enable'}
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  if (confirm('Remove backup configuration? Existing backups will be kept.')) {
                    deleteMutation.mutate()
                  }
                }}
                disabled={deleteMutation.isPending}
                className="px-3 py-1.5 bg-red-500/10 hover:bg-red-500/20 text-red-400 rounded-lg text-sm transition-colors disabled:opacity-50"
              >
                <Trash2 className="w-4 h-4" />
              </button>
            </div>
            <button
              onClick={(e) => {
                e.stopPropagation()
                triggerMutation.mutate()
              }}
              disabled={triggerMutation.isPending || !target.enabled}
              className="flex items-center gap-1.5 px-4 py-1.5 bg-primary-500 hover:bg-primary-600 text-white rounded-lg text-sm transition-colors disabled:opacity-50"
            >
              <Play className="w-4 h-4" />
              Run Now
            </button>
          </div>

          {/* Backup History */}
          <div className="space-y-2">
            <h4 className="text-sm font-medium text-dark-200">Backup History</h4>
            {backups.length > 0 ? (
              <div className="space-y-2 max-h-64 overflow-y-auto">
                {backups.slice(0, 10).map((backup) => (
                  <BackupRow key={backup.id} backup={backup} onBrowse={onBrowseBackup} />
                ))}
                {backups.length > 10 && (
                  <p className="text-xs text-dark-500 text-center py-2">
                    Showing 10 of {backups.length} backups
                  </p>
                )}
              </div>
            ) : (
              <p className="text-sm text-dark-400 py-4 text-center">No backups yet</p>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

export default function Backups() {
  const [wizardOpen, setWizardOpen] = useState(false)
  const [browsingBackup, setBrowsingBackup] = useState<Backup | null>(null)

  const { data: targets, isLoading: targetsLoading } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  const { data: allBackups } = useQuery({
    queryKey: ['backups'],
    queryFn: () => backupsApi.list({ limit: 500 }).then((r) => r.data),
    refetchInterval: 5000,
  })

  const { data: schedules = [] } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })

  // Group backups by target
  const getBackupsForTarget = (targetId: number): Backup[] => {
    return (allBackups || [])
      .filter((b) => b.target_id === targetId)
      .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
  }

  // Stats
  const stats = {
    activeTargets: targets?.filter((t) => t.enabled).length || 0,
    totalBackups: allBackups?.length || 0,
    completedBackups: allBackups?.filter((b) => b.status === 'completed').length || 0,
    failedBackups: allBackups?.filter((b) => b.status === 'failed').length || 0,
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Backups</h1>
          <p className="text-dark-400 mt-1">Manage your backup configurations and history</p>
        </div>
        <button
          onClick={() => setWizardOpen(true)}
          className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
        >
          <Plus className="w-4 h-4" />
          New Backup
        </button>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-4">
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <p className="text-2xl font-bold text-primary-500">{stats.activeTargets}</p>
          <p className="text-sm text-dark-400">Active Backups</p>
        </div>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <p className="text-2xl font-bold text-dark-200">{stats.totalBackups}</p>
          <p className="text-sm text-dark-400">Total Backups</p>
        </div>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <p className="text-2xl font-bold text-green-500">{stats.completedBackups}</p>
          <p className="text-sm text-dark-400">Completed</p>
        </div>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <p className="text-2xl font-bold text-red-500">{stats.failedBackups}</p>
          <p className="text-sm text-dark-400">Failed</p>
        </div>
      </div>

      {/* Backup Targets List */}
      {targetsLoading ? (
        <div className="space-y-4">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse">
              <div className="h-6 bg-dark-700 rounded w-1/3 mb-2" />
              <div className="h-4 bg-dark-700 rounded w-1/2" />
            </div>
          ))}
        </div>
      ) : targets && targets.length > 0 ? (
        <div className="space-y-4">
          {targets.map((target) => (
            <TargetBackupCard
              key={target.id}
              target={target}
              backups={getBackupsForTarget(target.id)}
              schedules={schedules}
              onBrowseBackup={setBrowsingBackup}
            />
          ))}
        </div>
      ) : (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
          <Archive className="w-12 h-12 text-dark-500 mx-auto mb-4" />
          <p className="text-dark-400">No backups configured</p>
          <p className="text-sm text-dark-500 mt-2">
            Click "New Backup" to set up your first backup
          </p>
          <button
            onClick={() => setWizardOpen(true)}
            className="mt-4 flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors mx-auto"
          >
            <Plus className="w-4 h-4" />
            Create Backup
          </button>
        </div>
      )}

      {/* Backup Wizard Modal */}
      <BackupWizard isOpen={wizardOpen} onClose={() => setWizardOpen(false)} />

      {/* Backup Browser Modal */}
      {browsingBackup && (
        <BackupBrowser backup={browsingBackup} onClose={() => setBrowsingBackup(null)} />
      )}
    </div>
  )
}
