import { useState, useMemo, useCallback, useRef, useEffect } from 'react'
import { createPortal } from 'react-dom'
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
  Pencil,
  Search,
  ArrowUpDown,
  Cloud,
  CloudOff,
  CloudCog,
  HardDrive,
  RefreshCw,
  MoreVertical,
} from 'lucide-react'
import {
  backupsApi,
  targetsApi,
  schedulesApi,
  storageApi,
  Backup,
  BackupTarget,
  ScheduleEntity,
  RemoteStorage,
} from '../api'
import { format, formatDistanceToNowStrict } from 'date-fns'
import toast from 'react-hot-toast'
import { useWebSocketStore } from '../store/websocket'
import { clsx } from 'clsx'
import BackupWizard from '../components/BackupWizard'
import BackupBrowser from '../components/BackupBrowser'
import BackupEditDialog from '../components/BackupEditDialog'
import ConfirmDialog from '../components/ConfirmDialog'
import EmptyState from '../components/EmptyState'
import LoadingSkeleton from '../components/LoadingSkeleton'
import StorageBrowser from '../components/StorageBrowser'
import RestoreWizard from '../components/RestoreWizard'

// Backup Row Component with dropdown actions menu
function BackupRow({
  backup,
  target,
  onBrowse,
  onDelete,
}: {
  backup: Backup
  target: BackupTarget
  onBrowse: (backup: Backup) => void
  onDelete: (backup: Backup) => void
}) {
  const backupProgress = useWebSocketStore((state) => state.backupProgress)
  const progress = backupProgress.get(backup.id)
  const queryClient = useQueryClient()
  const [menuOpen, setMenuOpen] = useState(false)
  const [showRestore, setShowRestore] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const [menuPos, setMenuPos] = useState({ top: 0, left: 0 })

  // Position dropdown via portal when opened
  useEffect(() => {
    if (!menuOpen || !btnRef.current) return
    const rect = btnRef.current.getBoundingClientRect()
    setMenuPos({ top: rect.bottom + 4, left: rect.right - 176 })
  }, [menuOpen])

  // Close menu on outside click or scroll
  useEffect(() => {
    if (!menuOpen) return
    const handleClick = (e: MouseEvent) => {
      if (
        menuRef.current && !menuRef.current.contains(e.target as Node) &&
        btnRef.current && !btnRef.current.contains(e.target as Node)
      ) {
        setMenuOpen(false)
      }
    }
    const handleScroll = () => setMenuOpen(false)
    document.addEventListener('mousedown', handleClick)
    window.addEventListener('scroll', handleScroll, true)
    return () => {
      document.removeEventListener('mousedown', handleClick)
      window.removeEventListener('scroll', handleScroll, true)
    }
  }, [menuOpen])

  const retrySyncMutation = useMutation({
    mutationFn: () => backupsApi.retrySync(backup.id),
    onSuccess: (res) => {
      const data = res.data as { succeeded: number; failed: number }
      if (data.failed === 0) {
        toast.success(`Remote sync succeeded (${data.succeeded} storage(s))`)
      } else {
        toast.error(`Retry: ${data.succeeded} ok, ${data.failed} failed`)
      }
      queryClient.invalidateQueries({ queryKey: ['backups'] })
      setMenuOpen(false)
    },
    onError: () => toast.error('Failed to retry remote sync'),
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

  // Determine remote sync status for display
  const remoteConfigured = target.remote_storage_ids && target.remote_storage_ids.length > 0
  const syncDetails = backup.remote_sync_details || []
  const syncCompleted = syncDetails.filter((s) => s.status === 'completed')
  const syncFailed = syncDetails.filter((s) => s.status === 'failed')
  const syncPending = syncDetails.filter((s) => s.status === 'pending')
  const allSynced = remoteConfigured && syncCompleted.length > 0 && syncFailed.length === 0 && syncPending.length === 0
  const hasSyncFailures = syncFailed.length > 0
  const hasSyncPending = syncPending.length > 0
  // Remote configured on target but no sync records yet for this backup
  const syncNotStarted = remoteConfigured && syncDetails.length === 0 && backup.status === 'completed'

  const getSyncStatusIndicator = () => {
    if (!remoteConfigured) return null

    if (allSynced) {
      return (
        <span
          title={syncCompleted.map((s) => `✓ ${s.storage_name}`).join('\n')}
          className="flex items-center gap-0.5 text-green-500"
        >
          <Cloud className="w-3.5 h-3.5" />
        </span>
      )
    }

    if (hasSyncFailures) {
      const failedNames = syncFailed.map((s) => s.storage_name).join(', ')
      const completedCount = syncCompleted.length
      const totalCount = syncDetails.length
      return (
        <span
          title={`Sync failed: ${failedNames}\n${completedCount}/${totalCount} succeeded`}
          className="flex items-center gap-0.5 text-red-400"
        >
          <CloudOff className="w-3.5 h-3.5" />
          <span className="text-[10px] font-medium">{syncFailed.length}</span>
        </span>
      )
    }

    if (hasSyncPending || syncNotStarted) {
      return (
        <span
          title="Remote sync pending"
          className="flex items-center gap-0.5 text-yellow-500"
        >
          <CloudCog className="w-3.5 h-3.5 animate-pulse" />
        </span>
      )
    }

    return null
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
        {/* Storage location indicators */}
        <div className="flex items-center gap-1.5 ml-2">
          {backup.local_available ? (
            <span title="Available locally" className="text-green-500">
              <HardDrive className="w-3.5 h-3.5" />
            </span>
          ) : backup.status === 'completed' ? (
            <span title="Not available locally (remote only)" className="text-dark-500">
              <HardDrive className="w-3.5 h-3.5" />
            </span>
          ) : null}
          {getSyncStatusIndicator()}
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

      {/* Action menu */}
      <div className="relative">
        <button
          ref={btnRef}
          onClick={(e) => {
            e.stopPropagation()
            setMenuOpen(!menuOpen)
          }}
          className="p-1.5 text-dark-400 hover:text-dark-200 hover:bg-dark-600 rounded transition-colors"
          title="Actions"
        >
          <MoreVertical className="w-4 h-4" />
        </button>
        {menuOpen && createPortal(
          <div
            ref={menuRef}
            className="fixed w-44 bg-dark-700 border border-dark-600 rounded-lg shadow-xl z-50 py-1"
            style={{ top: menuPos.top, left: menuPos.left }}
          >
            {backup.status === 'completed' && (
              <>
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    onBrowse(backup)
                    setMenuOpen(false)
                  }}
                  className="w-full flex items-center gap-2 px-3 py-2 text-sm text-dark-200 hover:bg-dark-600 transition-colors"
                >
                  <FolderOpen className="w-4 h-4" />
                  Browse files
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    setShowRestore(true)
                    setMenuOpen(false)
                  }}
                  className="w-full flex items-center gap-2 px-3 py-2 text-sm text-dark-200 hover:bg-dark-600 transition-colors"
                >
                  <RotateCcw className="w-4 h-4" />
                  Restore
                </button>
              </>
            )}
            {hasSyncFailures && (
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  retrySyncMutation.mutate()
                }}
                disabled={retrySyncMutation.isPending}
                className="w-full flex items-center gap-2 px-3 py-2 text-sm text-orange-400 hover:bg-dark-600 transition-colors disabled:opacity-50"
              >
                <RefreshCw className={clsx('w-4 h-4', retrySyncMutation.isPending && 'animate-spin')} />
                {retrySyncMutation.isPending ? 'Retrying...' : 'Retry sync'}
              </button>
            )}
            {syncNotStarted && backup.status === 'completed' && (
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  retrySyncMutation.mutate()
                }}
                disabled={retrySyncMutation.isPending}
                className="w-full flex items-center gap-2 px-3 py-2 text-sm text-yellow-400 hover:bg-dark-600 transition-colors disabled:opacity-50"
              >
                <Cloud className={clsx('w-4 h-4', retrySyncMutation.isPending && 'animate-spin')} />
                {retrySyncMutation.isPending ? 'Syncing...' : 'Sync to remote'}
              </button>
            )}
            <div className="border-t border-dark-600 my-1" />
            <button
              onClick={(e) => {
                e.stopPropagation()
                onDelete(backup)
                setMenuOpen(false)
              }}
              className="w-full flex items-center gap-2 px-3 py-2 text-sm text-red-400 hover:bg-dark-600 transition-colors"
            >
              <Trash2 className="w-4 h-4" />
              Delete
            </button>
          </div>,
          document.body
        )}
      </div>

      {showRestore && (
        <RestoreWizard backupId={backup.id} onClose={() => setShowRestore(false)} />
      )}
    </div>
  )
}

// Target Card with backup list
function TargetBackupCard({
  target,
  backups,
  schedules,
  onBrowseBackup,
  onEdit,
  onDeleteBackup,
  onDeleteTarget,
  onDeleteAllBackups,
}: {
  target: BackupTarget
  backups: Backup[]
  schedules: ScheduleEntity[]
  onBrowseBackup: (backup: Backup) => void
  onEdit: (target: BackupTarget) => void
  onDeleteBackup: (backup: Backup) => void
  onDeleteTarget: (target: BackupTarget) => void
  onDeleteAllBackups: (target: BackupTarget) => void
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

  const getTargetTypeIcon = () => {
    switch (target.target_type) {
      case 'container':
        return '\uD83D\uDC33'
      case 'volume':
        return '\uD83D\uDCBE'
      case 'path':
        return '\uD83D\uDCC1'
      case 'stack':
        return '\uD83D\uDCDA'
      default:
        return '\uD83D\uDCE6'
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
            {/* B1: Last Backup Status - more info on collapsed card */}
            {lastBackup ? (
              <div className="flex items-center gap-2 text-sm">
                {lastBackup.status === 'completed' ? (
                  <CheckCircle className="w-3.5 h-3.5 text-green-500 flex-shrink-0" />
                ) : lastBackup.status === 'failed' ? (
                  <XCircle className="w-3.5 h-3.5 text-red-500 flex-shrink-0" />
                ) : lastBackup.status === 'running' ? (
                  <Loader2 className="w-3.5 h-3.5 text-primary-500 animate-spin flex-shrink-0" />
                ) : null}
                <span className="text-xs text-dark-300">
                  {formatDistanceToNowStrict(new Date(lastBackup.created_at), { addSuffix: true })}
                </span>
                {lastBackup.file_size_human && (
                  <span className="text-xs text-dark-500">{lastBackup.file_size_human}</span>
                )}
              </div>
            ) : (
              <span className="px-2 py-1 rounded-full text-xs font-medium bg-dark-600 text-dark-400">
                No backups
              </span>
            )}

            {/* Schedule name on collapsed card */}
            {schedule && (
              <span className="hidden sm:flex items-center gap-1 text-xs text-dark-500">
                <Calendar className="w-3 h-3" />
                {schedule.name}
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
                  onEdit(target)
                }}
                className="flex items-center gap-1.5 px-3 py-1.5 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg text-sm transition-colors"
                title="Edit backup settings"
              >
                <Pencil className="w-4 h-4" />
                Edit
              </button>
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
                  onDeleteTarget(target)
                }}
                className="px-3 py-1.5 bg-red-500/10 hover:bg-red-500/20 text-red-400 rounded-lg text-sm transition-colors"
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
              {triggerMutation.isPending ? (
                <Loader2 className="w-4 h-4 animate-spin" />
              ) : (
                <Play className="w-4 h-4" />
              )}
              {triggerMutation.isPending ? 'Starting...' : 'Run Now'}
            </button>
          </div>

          {/* Backup History */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <h4 className="text-sm font-medium text-dark-200">Backup History</h4>
              {backups.length > 0 && (
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    onDeleteAllBackups(target)
                  }}
                  className="flex items-center gap-1 px-2 py-1 text-xs bg-red-500/10 hover:bg-red-500/20 text-red-400 rounded transition-colors"
                >
                  <Trash2 className="w-3 h-3" />
                  Delete All ({backups.length})
                </button>
              )}
            </div>
            {backups.length > 0 ? (
              <div className="space-y-2 max-h-64 overflow-y-auto">
                {backups.slice(0, 10).map((backup) => (
                  <BackupRow key={backup.id} backup={backup} target={target} onBrowse={onBrowseBackup} onDelete={onDeleteBackup} />
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
  const queryClient = useQueryClient()
  const [wizardOpen, setWizardOpen] = useState(false)
  const [browsingBackup, setBrowsingBackup] = useState<Backup | null>(null)
  const [editingTarget, setEditingTarget] = useState<BackupTarget | null>(null)
  const [searchQuery, setSearchQuery] = useState('')
  const [typeFilter, setTypeFilter] = useState<string | null>(null)
  const [sortBy, setSortBy] = useState<'name' | 'type' | 'last-backup' | 'created'>('name')
  const [statsFilter, setStatsFilter] = useState<'active' | 'completed' | 'failed' | null>(null)

  // Confirm dialog state
  const [confirmDialog, setConfirmDialog] = useState<{
    type: 'backup' | 'target' | 'all-backups'
    id: number
    title: string
    message: string
    hasRemote?: boolean
    hasLocal?: boolean
  } | null>(null)
  const [deleteLocal, setDeleteLocal] = useState(true)
  const [deleteRemote, setDeleteRemote] = useState(true)
  const [browsingStorage, setBrowsingStorage] = useState<RemoteStorage | null>(null)

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

  const { data: storages = [] } = useQuery({
    queryKey: ['storages'],
    queryFn: () => storageApi.list().then((r) => r.data),
  })

  const deleteBackupMutation = useMutation({
    mutationFn: ({ backupId, local, remote }: { backupId: number; local: boolean; remote: boolean }) =>
      backupsApi.delete(backupId, { deleteLocal: local, deleteRemote: remote }),
    onSuccess: () => {
      toast.success('Backup deleted')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
      setConfirmDialog(null)
    },
    onError: () => toast.error('Failed to delete backup'),
  })

  const deleteTargetMutation = useMutation({
    mutationFn: (targetId: number) => targetsApi.delete(targetId),
    onSuccess: () => {
      toast.success('Backup configuration removed')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      setConfirmDialog(null)
    },
    onError: () => toast.error('Failed to remove backup'),
  })

  const deleteAllBackupsMutation = useMutation({
    mutationFn: ({ targetId, local, remote }: { targetId: number; local: boolean; remote: boolean }) =>
      backupsApi.deleteAll(targetId, { deleteLocal: local, deleteRemote: remote }),
    onSuccess: (res) => {
      const count = res.data?.deleted_count ?? 0
      toast.success(`${count} backup(s) deleted`)
      queryClient.invalidateQueries({ queryKey: ['backups'] })
      setConfirmDialog(null)
    },
    onError: () => toast.error('Failed to delete backups'),
  })

  // Group backups by target
  const getBackupsForTarget = useCallback((targetId: number): Backup[] => {
    return (allBackups || [])
      .filter((b) => b.target_id === targetId)
      .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
  }, [allBackups])

  // Filter and sort targets
  const filteredTargets = useMemo(() => {
    if (!targets) return []
    const filtered = targets.filter((t) => {
      if (searchQuery) {
        const q = searchQuery.toLowerCase()
        const nameMatch = t.name.toLowerCase().includes(q)
        const targetMatch = (t.container_name || t.volume_name || t.stack_name || t.host_path || '').toLowerCase().includes(q)
        if (!nameMatch && !targetMatch) return false
      }
      if (typeFilter && t.target_type !== typeFilter) return false
      if (statsFilter === 'active' && !t.enabled) return false
      if (statsFilter === 'completed') {
        const backups = getBackupsForTarget(t.id)
        if (!backups.some((b) => b.status === 'completed')) return false
      }
      if (statsFilter === 'failed') {
        const backups = getBackupsForTarget(t.id)
        if (!backups.some((b) => b.status === 'failed')) return false
      }
      return true
    })

    return filtered.sort((a, b) => {
      switch (sortBy) {
        case 'name':
          return a.name.localeCompare(b.name)
        case 'type':
          return a.target_type.localeCompare(b.target_type) || a.name.localeCompare(b.name)
        case 'last-backup': {
          const aBackups = getBackupsForTarget(a.id)
          const bBackups = getBackupsForTarget(b.id)
          const aLast = aBackups[0]?.created_at || ''
          const bLast = bBackups[0]?.created_at || ''
          return bLast.localeCompare(aLast) || a.name.localeCompare(b.name)
        }
        case 'created':
          return new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
        default:
          return 0
      }
    })
  }, [targets, searchQuery, typeFilter, statsFilter, sortBy, getBackupsForTarget])

  // Stats
  const stats = {
    activeTargets: targets?.filter((t) => t.enabled).length || 0,
    totalBackups: allBackups?.length || 0,
    completedBackups: allBackups?.filter((b) => b.status === 'completed').length || 0,
    failedBackups: allBackups?.filter((b) => b.status === 'failed').length || 0,
  }

  const handleDeleteBackup = (backup: Backup) => {
    const hasLocal = backup.local_available !== false
    const hasRemote = backup.remote_synced === true
    setDeleteLocal(true)
    setDeleteRemote(hasRemote)
    setConfirmDialog({
      type: 'backup',
      id: backup.id,
      title: 'Delete Backup',
      message: 'Are you sure you want to delete this backup? This action cannot be undone.',
      hasLocal,
      hasRemote,
    })
  }

  const handleDeleteTarget = (target: BackupTarget) => {
    setConfirmDialog({
      type: 'target',
      id: target.id,
      title: 'Remove Backup Configuration',
      message: 'Remove this backup configuration? Existing backups will be kept.',
    })
  }

  const handleDeleteAllBackups = (target: BackupTarget) => {
    const backups = getBackupsForTarget(target.id)
    const count = backups.length
    const anyRemote = backups.some((b) => b.remote_synced)
    const anyLocal = backups.some((b) => b.local_available !== false)
    setDeleteLocal(true)
    setDeleteRemote(anyRemote)
    setConfirmDialog({
      type: 'all-backups',
      id: target.id,
      title: 'Delete All Backups',
      message: `Are you sure you want to delete all ${count} backup(s) for "${target.name}"? This action cannot be undone.`,
      hasLocal: anyLocal,
      hasRemote: anyRemote,
    })
  }

  const handleConfirmDelete = () => {
    if (!confirmDialog) return
    if (confirmDialog.type === 'backup') {
      deleteBackupMutation.mutate({
        backupId: confirmDialog.id,
        local: deleteLocal,
        remote: deleteRemote,
      })
    } else if (confirmDialog.type === 'all-backups') {
      deleteAllBackupsMutation.mutate({
        targetId: confirmDialog.id,
        local: deleteLocal,
        remote: deleteRemote,
      })
    } else {
      deleteTargetMutation.mutate(confirmDialog.id)
    }
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Backups</h1>
          <p className="text-dark-400 mt-1">Manage your backup configurations and history</p>
        </div>
        <div className="flex items-center gap-2">
          {storages.length > 0 && (
            <div className="relative group">
              <button
                className="flex items-center gap-2 px-3 py-2 bg-dark-800 border border-dark-700 text-dark-200 rounded-lg hover:bg-dark-700 transition-colors"
                onClick={() => {
                  if (storages.length === 1) {
                    setBrowsingStorage(storages[0])
                  }
                }}
              >
                <Cloud className="w-4 h-4" />
                Remote Storage
                {storages.length > 1 && <ChevronDown className="w-3 h-3" />}
              </button>
              {storages.length > 1 && (
                <div className="absolute right-0 mt-1 w-56 bg-dark-800 border border-dark-700 rounded-lg shadow-xl opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-all z-20">
                  {storages.map((s) => (
                    <button
                      key={s.id}
                      onClick={() => setBrowsingStorage(s)}
                      className="w-full text-left px-4 py-2.5 text-sm text-dark-200 hover:bg-dark-700 first:rounded-t-lg last:rounded-b-lg transition-colors flex items-center gap-2"
                    >
                      <Cloud className="w-3.5 h-3.5 text-dark-400" />
                      {s.name}
                      <span className="text-xs text-dark-500 ml-auto capitalize">{s.storage_type}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          <button
            onClick={() => setWizardOpen(true)}
            className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
          >
            <Plus className="w-4 h-4" />
            New Backup
          </button>
        </div>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-4">
        <button
          onClick={() => setStatsFilter(statsFilter === 'active' ? null : 'active')}
          className={`bg-dark-800 rounded-xl border p-4 text-left transition-colors ${
            statsFilter === 'active' ? 'border-primary-500 bg-primary-500/5' : 'border-dark-700 hover:border-dark-600'
          }`}
        >
          <p className="text-2xl font-bold text-primary-500">{stats.activeTargets}</p>
          <p className="text-sm text-dark-400">Active Backups</p>
        </button>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <p className="text-2xl font-bold text-dark-200">{stats.totalBackups}</p>
          <p className="text-sm text-dark-400">Total Backups</p>
        </div>
        <button
          onClick={() => setStatsFilter(statsFilter === 'completed' ? null : 'completed')}
          className={`bg-dark-800 rounded-xl border p-4 text-left transition-colors ${
            statsFilter === 'completed' ? 'border-green-500 bg-green-500/5' : 'border-dark-700 hover:border-dark-600'
          }`}
        >
          <p className="text-2xl font-bold text-green-500">{stats.completedBackups}</p>
          <p className="text-sm text-dark-400">Completed</p>
        </button>
        <button
          onClick={() => setStatsFilter(statsFilter === 'failed' ? null : 'failed')}
          className={`bg-dark-800 rounded-xl border p-4 text-left transition-colors ${
            statsFilter === 'failed' ? 'border-red-500 bg-red-500/5' : 'border-dark-700 hover:border-dark-600'
          }`}
        >
          <p className="text-2xl font-bold text-red-500">{stats.failedBackups}</p>
          <p className="text-sm text-dark-400">Failed</p>
        </button>
      </div>

      {/* Search & Filter Bar */}
      {targets && targets.length > 0 && (
        <div className="flex items-center gap-3">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-500" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search backup targets..."
              className="w-full pl-9 pr-4 py-2 bg-dark-800 border border-dark-700 rounded-lg text-dark-100 text-sm placeholder:text-dark-500 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
          </div>
          <div className="flex gap-1.5">
            {(['container', 'volume', 'stack', 'path'] as const).map((type) => {
              const count = targets?.filter((t) => t.target_type === type).length || 0
              if (count === 0) return null
              return (
                <button
                  key={type}
                  onClick={() => setTypeFilter(typeFilter === type ? null : type)}
                  className={clsx(
                    'px-3 py-1.5 rounded-lg text-xs capitalize transition-colors border',
                    typeFilter === type
                      ? 'bg-primary-500/20 border-primary-500/50 text-primary-300'
                      : 'bg-dark-800 border-dark-700 text-dark-400 hover:text-dark-300'
                  )}
                >
                  {type}
                  <span className={clsx(
                    'ml-1.5 px-1.5 py-0.5 rounded-full text-[10px] font-medium',
                    typeFilter === type
                      ? 'bg-primary-500/30 text-primary-200'
                      : 'bg-dark-700 text-dark-500'
                  )}>
                    {count}
                  </span>
                </button>
              )
            })}
          </div>
          <div className="flex items-center gap-1.5">
            <ArrowUpDown className="w-3.5 h-3.5 text-dark-500" />
            <select
              value={sortBy}
              onChange={(e) => setSortBy(e.target.value as typeof sortBy)}
              className="bg-dark-800 border border-dark-700 rounded-lg text-dark-300 text-xs py-1.5 px-2 focus:border-primary-500 focus:ring-1 focus:ring-primary-500 cursor-pointer"
            >
              <option value="name">Name</option>
              <option value="type">Type</option>
              <option value="last-backup">Last Backup</option>
              <option value="created">Newest First</option>
            </select>
          </div>
        </div>
      )}

      {/* Backup Targets List */}
      {targetsLoading ? (
        <LoadingSkeleton count={3} layout="list" />
      ) : filteredTargets.length > 0 ? (
        <div className="space-y-4">
          {filteredTargets.map((target) => (
            <TargetBackupCard
              key={target.id}
              target={target}
              backups={getBackupsForTarget(target.id)}
              schedules={schedules}
              onBrowseBackup={setBrowsingBackup}
              onEdit={setEditingTarget}
              onDeleteBackup={handleDeleteBackup}
              onDeleteTarget={handleDeleteTarget}
              onDeleteAllBackups={handleDeleteAllBackups}
            />
          ))}
        </div>
      ) : targets && targets.length > 0 ? (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-8 text-center">
          <Search className="w-8 h-8 text-dark-500 mx-auto mb-3" />
          <p className="text-dark-400">No targets matching your search</p>
          <button
            onClick={() => { setSearchQuery(''); setTypeFilter(null) }}
            className="mt-2 text-sm text-primary-400 hover:text-primary-300"
          >
            Clear filters
          </button>
        </div>
      ) : (
        <EmptyState
          icon={Archive}
          title="No backups configured"
          description='Click "New Backup" to set up your first backup'
          action={{ label: 'Create Backup', onClick: () => setWizardOpen(true), icon: Plus }}
        />
      )}

      {/* Backup Wizard Modal */}
      <BackupWizard isOpen={wizardOpen} onClose={() => setWizardOpen(false)} />

      {/* Backup Browser Modal */}
      {browsingBackup && (
        <BackupBrowser backup={browsingBackup} onClose={() => setBrowsingBackup(null)} />
      )}

      {/* Backup Edit Dialog */}
      {editingTarget && (
        <BackupEditDialog
          target={editingTarget}
          isOpen={true}
          onClose={() => setEditingTarget(null)}
        />
      )}

      {/* Confirm Dialog */}
      <ConfirmDialog
        isOpen={!!confirmDialog}
        onClose={() => setConfirmDialog(null)}
        onConfirm={handleConfirmDelete}
        title={confirmDialog?.title || ''}
        message={confirmDialog?.message || ''}
        confirmLabel="Delete"
        confirmVariant="danger"
        confirmDisabled={confirmDialog?.type !== 'target' && !deleteLocal && !deleteRemote}
        isLoading={deleteBackupMutation.isPending || deleteTargetMutation.isPending || deleteAllBackupsMutation.isPending}
      >
        {confirmDialog && confirmDialog.type !== 'target' && (confirmDialog.hasLocal || confirmDialog.hasRemote) && (
          <div className="space-y-2 mt-2">
            <p className="text-xs font-medium text-dark-300 uppercase tracking-wide">Delete from:</p>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={deleteLocal}
                onChange={(e) => setDeleteLocal(e.target.checked)}
                className="rounded border-dark-600 bg-dark-900 text-primary-500 focus:ring-primary-500"
              />
              <HardDrive className="w-3.5 h-3.5 text-dark-400" />
              <span className="text-sm text-dark-200">Local storage</span>
            </label>
            {confirmDialog.hasRemote && (
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={deleteRemote}
                  onChange={(e) => setDeleteRemote(e.target.checked)}
                  className="rounded border-dark-600 bg-dark-900 text-primary-500 focus:ring-primary-500"
                />
                <Cloud className="w-3.5 h-3.5 text-green-500" />
                <span className="text-sm text-dark-200">Remote storage</span>
              </label>
            )}
            {!deleteLocal && !deleteRemote && (
              <p className="text-xs text-red-400">Select at least one option.</p>
            )}
          </div>
        )}
      </ConfirmDialog>

      {/* Remote Storage Browser */}
      {browsingStorage && (
        <StorageBrowser storage={browsingStorage} onClose={() => setBrowsingStorage(null)} />
      )}
    </div>
  )
}
