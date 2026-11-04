import { useState, useMemo, useCallback } from 'react'
import { Menu, MenuTrigger, MenuContent, MenuItem } from '../components/ui/menu'
import { ChoiceSelect } from '../components/ui/select'
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
  FileText,
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
import RemovalDialog, { RemovalIntent } from '../components/RemovalDialog'
import EmptyState from '../components/EmptyState'
import LoadingSkeleton from '../components/LoadingSkeleton'
import StorageBrowser from '../components/StorageBrowser'
import RestoreWizard from '../components/RestoreWizard'
import BackupLogViewer from '../components/BackupLogViewer'

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
  const [showLogs, setShowLogs] = useState(false)

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

      <Menu open={menuOpen} onOpenChange={setMenuOpen}>
        <MenuTrigger aria-label="Archive actions" className="p-3 text-dark-300 hover:bg-dark-600 rounded"><MoreVertical className="size-4" /></MenuTrigger>
        <MenuContent>
          {backup.status === 'completed' && <>
            <MenuItem onClick={() => onBrowse(backup)}><FolderOpen className="size-4" />Browse files</MenuItem>
            <MenuItem onClick={() => setShowRestore(true)}><RotateCcw className="size-4" />Restore</MenuItem>
          </>}
          {(hasSyncFailures || syncNotStarted) && <MenuItem disabled={retrySyncMutation.isPending} onClick={() => retrySyncMutation.mutate()}><RefreshCw className="size-4" />{hasSyncFailures ? 'Retry sync' : 'Sync to remote'}</MenuItem>}
          <MenuItem onClick={() => setShowLogs(true)}><FileText className="size-4" />View Logs</MenuItem>
          <MenuItem className="text-red-300" onClick={() => onDelete(backup)}><Trash2 className="size-4" />Delete</MenuItem>
        </MenuContent>
      </Menu>

      {showRestore && (
        <RestoreWizard backupId={backup.id} onClose={() => setShowRestore(false)} />
      )}

      {showLogs && (
        <BackupLogViewer backupId={backup.id} onClose={() => setShowLogs(false)} />
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
        role="button"
        tabIndex={0}
        aria-expanded={expanded}
        onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setExpanded(!expanded) } }}
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex min-w-0 flex-1 items-center gap-3">
            <div className="w-10 h-10 bg-dark-700 rounded-lg flex items-center justify-center text-xl">
              {getTargetTypeIcon()}
            </div>
            <div className="min-w-0 break-words">
              <div className="flex items-center gap-2">
                <h2 className="text-base font-medium text-dark-100">{target.name}</h2>
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

          <div className="flex flex-wrap items-center gap-3">
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
          {target.removed_at && <p>Retained history — protection plan removed. Copies remain available for browse and restore.</p>}
          {!target.removed_at && (
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
                aria-label={`Remove protection plan ${target.name}`}
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
          )}

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
                  Review archive copy deletion
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

export default function Backups({ workspace }: { workspace?: 'protection' | 'history' } = {}) {
  const queryClient = useQueryClient()
  const [wizardOpen, setWizardOpen] = useState(false)
  const [browsingBackup, setBrowsingBackup] = useState<Backup | null>(null)
  const [editingTarget, setEditingTarget] = useState<BackupTarget | null>(null)
  const [searchQuery, setSearchQuery] = useState('')
  const [typeFilter, setTypeFilter] = useState<string | null>(null)
  const [sortBy, setSortBy] = useState<'name' | 'type' | 'last-backup' | 'created'>('name')
  const [statsFilter, setStatsFilter] = useState<'active' | 'completed' | 'failed' | null>(null)

  // Confirm dialog state
  const [removal, setRemoval] = useState<RemovalIntent | null>(null)
  const [browsingStorage, setBrowsingStorage] = useState<RemoteStorage | null>(null)

  const { data: targets, isLoading: targetsLoading } = useQuery({
    queryKey: ['targets', 'with-history'],
    queryFn: () => targetsApi.list(true).then((r) => r.data),
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
    setRemoval({ kind: 'copies', backupId: backup.id, name: backup.target_name || `Archive #${backup.id}` })
  }

  const handleDeleteTarget = (target: BackupTarget) => {
    setRemoval({ kind: 'plan', targetId: target.id, name: target.name })
  }

  const handleDeleteAllBackups = (target: BackupTarget) => {
    setRemoval({ kind: 'copies', targetId: target.id, name: target.name })
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">{workspace === 'protection' ? 'Protection' : workspace === 'history' ? 'History & Restore' : 'Backups'}</h1>
          <p className="text-dark-400 mt-1">Manage your backup configurations and history</p>
          {workspace && <p className="mt-2 text-sm text-dark-300">{workspace === 'history' ? 'Expand a plan to browse existing archives, view logs, download or restore. This shared view remains available while the dedicated history workspace is developed.' : 'Existing plans, creation and editing remain available here. Schedules and retention are shared protection policies.'}</p>}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {storages.length > 0 && (
            <Menu>
              <MenuTrigger className="flex min-h-11 items-center gap-2 rounded-lg border border-dark-600 bg-dark-800 px-3 py-2 text-dark-200"><Cloud className="size-4" />Remote Storage<ChevronDown className="size-4" /></MenuTrigger>
              <MenuContent>{storages.map((storage) => <MenuItem key={storage.id} onClick={() => setBrowsingStorage(storage)}><Cloud className="size-4" />{storage.name}<span className="text-dark-300">{storage.storage_type}</span></MenuItem>)}</MenuContent>
            </Menu>
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
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
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
        <div className="flex flex-wrap items-center gap-3">
          <div className="relative min-w-40 flex-1">
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
            <ChoiceSelect label="Sort plans" value={sortBy} onValueChange={setSortBy} options={[{ value: 'name', label: 'Name' }, { value: 'type', label: 'Type' }, { value: 'last-backup', label: 'Last Backup' }, { value: 'created', label: 'Newest First' }]} />
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

      {removal && <RemovalDialog intent={removal} onClose={() => setRemoval(null)} onChanged={() => {
        queryClient.invalidateQueries({ queryKey: ['backups'] })
        queryClient.invalidateQueries({ queryKey: ['targets'] })
        queryClient.invalidateQueries({ queryKey: ['schedules'] })
      }} />}

      {/* Remote Storage Browser */}
      {browsingStorage && (
        <StorageBrowser storage={browsingStorage} onClose={() => setBrowsingStorage(null)} />
      )}
    </div>
  )
}
