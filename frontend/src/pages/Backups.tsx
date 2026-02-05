import { useState, useMemo, useCallback } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Archive,
  CheckCircle,
  XCircle,
  Clock,
  Loader2,
  Trash2,
  RotateCcw,
  Container as ContainerIcon,
  HardDrive,
  Layers,
  Search,
  SortAsc,
  SortDesc,
  Plus,
  ChevronDown,
  ChevronUp,
  Play,
} from 'lucide-react'
import {
  backupsApi,
  dockerApi,
  targetsApi,
  Backup,
  Container,
  Volume,
  Stack,
  BackupTarget,
} from '../api'
import { format, formatDistanceToNow } from 'date-fns'
import toast from 'react-hot-toast'
import { useWebSocketStore } from '../store/websocket'
import { clsx } from 'clsx'

type TabType = 'containers' | 'volumes' | 'stacks'
type SortField = 'name' | 'status' | 'lastBackup'
type SortOrder = 'asc' | 'desc'

// Backup Row Component (for history view)
function BackupRow({ backup }: { backup: Backup }) {
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
      toast.success('Backup restored')
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
    <div className="flex items-center justify-between py-2 px-3 bg-dark-750 rounded-lg">
      <div className="flex items-center gap-3">
        {getStatusIcon()}
        <div>
          <p className="text-sm text-dark-200">
            {format(new Date(backup.created_at), 'yyyy-MM-dd HH:mm')}
          </p>
          <p className="text-xs text-dark-400">
            {backup.file_size_human || '-'} • {backup.duration_seconds ? `${backup.duration_seconds}s` : '-'}
          </p>
        </div>
      </div>
      {progress && backup.status === 'running' && (
        <div className="flex-1 mx-4">
          <div className="w-full h-1 bg-dark-700 rounded-full overflow-hidden">
            <div
              className="h-full bg-primary-500 rounded-full transition-all"
              style={{ width: `${Math.max(0, progress.progress)}%` }}
            />
          </div>
        </div>
      )}
      <div className="flex items-center gap-1">
        {backup.status === 'completed' && (
          <button
            onClick={() => restoreMutation.mutate()}
            disabled={restoreMutation.isPending}
            className="p-1.5 text-dark-400 hover:text-primary-400 hover:bg-dark-700 rounded transition-colors"
            title="Restore"
          >
            <RotateCcw className="w-3.5 h-3.5" />
          </button>
        )}
        <button
          onClick={() => deleteMutation.mutate()}
          disabled={deleteMutation.isPending}
          className="p-1.5 text-dark-400 hover:text-red-400 hover:bg-dark-700 rounded transition-colors"
          title="Delete"
        >
          <Trash2 className="w-3.5 h-3.5" />
        </button>
      </div>
    </div>
  )
}

// Item Card with backup info
function ItemCard({
  name,
  subtitle,
  icon: Icon,
  iconColor,
  target,
  backups,
  onSetupBackup,
  details,
}: {
  name: string
  subtitle?: string
  icon: typeof ContainerIcon
  iconColor: string
  target?: BackupTarget
  backups: Backup[]
  onSetupBackup: () => void
  details?: React.ReactNode
}) {
  const [expanded, setExpanded] = useState(false)
  const queryClient = useQueryClient()

  const triggerMutation = useMutation({
    mutationFn: () => backupsApi.create(target!.id, 'full'),
    onSuccess: () => {
      toast.success('Backup started')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: () => toast.error('Failed to start backup'),
  })

  const hasBackup = !!target
  const lastBackup = backups[0]
  const lastBackupStatus = lastBackup?.status

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 overflow-hidden">
      <div
        className="p-4 cursor-pointer hover:bg-dark-750 transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className={`w-10 h-10 ${iconColor} rounded-lg flex items-center justify-center`}>
              <Icon className="w-5 h-5 text-white" />
            </div>
            <div>
              <h3 className="text-base font-medium text-dark-100">{name}</h3>
              {subtitle && <p className="text-sm text-dark-400">{subtitle}</p>}
            </div>
          </div>
          <div className="flex items-center gap-3">
            {/* Backup Status Badge */}
            {hasBackup ? (
              <div className="flex items-center gap-2">
                <span
                  className={clsx(
                    'px-2 py-1 rounded-full text-xs font-medium',
                    lastBackupStatus === 'completed'
                      ? 'bg-green-500/10 text-green-400'
                      : lastBackupStatus === 'failed'
                      ? 'bg-red-500/10 text-red-400'
                      : lastBackupStatus === 'running'
                      ? 'bg-primary-500/10 text-primary-400'
                      : 'bg-dark-600 text-dark-400'
                  )}
                >
                  {lastBackup
                    ? formatDistanceToNow(new Date(lastBackup.created_at), { addSuffix: true })
                    : 'No backups yet'}
                </span>
                <CheckCircle className="w-5 h-5 text-green-500" />
              </div>
            ) : (
              <span className="px-2 py-1 rounded-full text-xs font-medium bg-dark-600 text-dark-400">
                No backup
              </span>
            )}
            {expanded ? (
              <ChevronUp className="w-5 h-5 text-dark-400" />
            ) : (
              <ChevronDown className="w-5 h-5 text-dark-400" />
            )}
          </div>
        </div>
      </div>

      {/* Expanded Details */}
      {expanded && (
        <div className="px-4 pb-4 border-t border-dark-700 pt-4">
          {details && <div className="mb-4">{details}</div>}

          {hasBackup ? (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <h4 className="text-sm font-medium text-dark-200">Backup History</h4>
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    triggerMutation.mutate()
                  }}
                  disabled={triggerMutation.isPending}
                  className="flex items-center gap-1.5 px-3 py-1.5 bg-primary-500 hover:bg-primary-600 text-white rounded-lg text-sm transition-colors disabled:opacity-50"
                >
                  <Play className="w-3.5 h-3.5" />
                  Run Now
                </button>
              </div>
              {backups.length > 0 ? (
                <div className="space-y-2 max-h-48 overflow-y-auto">
                  {backups.slice(0, 5).map((backup) => (
                    <BackupRow key={backup.id} backup={backup} />
                  ))}
                </div>
              ) : (
                <p className="text-sm text-dark-400 py-2">No backups yet</p>
              )}
            </div>
          ) : (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onSetupBackup()
              }}
              className="w-full flex items-center justify-center gap-2 px-4 py-3 bg-primary-500/10 hover:bg-primary-500/20 text-primary-400 rounded-lg transition-colors"
            >
              <Plus className="w-5 h-5" />
              Setup Backup
            </button>
          )}
        </div>
      )}
    </div>
  )
}

export default function Backups() {
  const [activeTab, setActiveTab] = useState<TabType>('containers')
  const [searchQuery, setSearchQuery] = useState('')
  const [sortField, setSortField] = useState<SortField>('name')
  const [sortOrder, setSortOrder] = useState<SortOrder>('asc')
  const [showOnlyWithBackup, setShowOnlyWithBackup] = useState(false)

  const queryClient = useQueryClient()

  // Fetch all data
  const { data: containers, isLoading: containersLoading } = useQuery({
    queryKey: ['containers'],
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
  })

  const { data: volumes, isLoading: volumesLoading } = useQuery({
    queryKey: ['volumes'],
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
  })

  const { data: stacks, isLoading: stacksLoading } = useQuery({
    queryKey: ['stacks'],
    queryFn: () => dockerApi.listStacks().then((r) => r.data),
  })

  const { data: targets } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  const { data: backups } = useQuery({
    queryKey: ['backups'],
    queryFn: () => backupsApi.list({ limit: 100 }).then((r) => r.data),
    refetchInterval: 5000,
  })

  // Helper to find target for an item
  const findTarget = useCallback((type: string, name: string): BackupTarget | undefined => {
    return targets?.find((t) => {
      if (type === 'container') return t.target_type === 'container' && t.container_name === name
      if (type === 'volume') return t.target_type === 'volume' && t.volume_name === name
      if (type === 'stack') return t.target_type === 'stack' && t.stack_name === name
      return false
    })
  }, [targets])

  // Helper to get backups for a target
  const getBackupsForTarget = useCallback((targetId: number): Backup[] => {
    return (backups || [])
      .filter((b) => b.target_id === targetId)
      .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
  }, [backups])

  // Setup backup mutation
  const createTargetMutation = useMutation({
    mutationFn: (data: { type: string; name: string; dependencies?: string[] }) => {
      const baseData = {
        name: `${data.name}-backup`,
        enabled: true,
        stop_container: true,
        compression_enabled: true,
        dependencies: data.dependencies || [],
      }

      if (data.type === 'container') {
        return targetsApi.create({ ...baseData, target_type: 'container', container_name: data.name })
      } else if (data.type === 'volume') {
        return targetsApi.create({ ...baseData, target_type: 'volume', volume_name: data.name })
      } else {
        return targetsApi.create({ ...baseData, target_type: 'stack', stack_name: data.name })
      }
    },
    onSuccess: () => {
      toast.success('Backup target created')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to create backup target'),
  })

  // Filter and sort logic
  const filteredItems = useMemo(() => {
    let items: Array<{
      type: 'container' | 'volume' | 'stack'
      name: string
      subtitle?: string
      data: Container | Volume | Stack
      target?: BackupTarget
      backups: Backup[]
    }> = []

    if (activeTab === 'containers' && containers) {
      items = containers.map((c) => {
        const target = findTarget('container', c.name)
        return {
          type: 'container' as const,
          name: c.name,
          subtitle: c.image,
          data: c,
          target,
          backups: target ? getBackupsForTarget(target.id) : [],
        }
      })
    } else if (activeTab === 'volumes' && volumes) {
      items = volumes.map((v) => {
        const target = findTarget('volume', v.name)
        return {
          type: 'volume' as const,
          name: v.name,
          subtitle: v.driver,
          data: v,
          target,
          backups: target ? getBackupsForTarget(target.id) : [],
        }
      })
    } else if (activeTab === 'stacks' && stacks) {
      items = stacks.map((s) => {
        const target = findTarget('stack', s.name)
        return {
          type: 'stack' as const,
          name: s.name,
          subtitle: `${s.containers.length} containers`,
          data: s,
          target,
          backups: target ? getBackupsForTarget(target.id) : [],
        }
      })
    }

    // Filter by search
    if (searchQuery) {
      const query = searchQuery.toLowerCase()
      items = items.filter((item) => item.name.toLowerCase().includes(query))
    }

    // Filter by backup status
    if (showOnlyWithBackup) {
      items = items.filter((item) => !!item.target)
    }

    // Sort
    items.sort((a, b) => {
      let comparison = 0
      if (sortField === 'name') {
        comparison = a.name.localeCompare(b.name)
      } else if (sortField === 'status') {
        comparison = (a.target ? 1 : 0) - (b.target ? 1 : 0)
      } else if (sortField === 'lastBackup') {
        const aTime = a.backups[0]?.created_at ? new Date(a.backups[0].created_at).getTime() : 0
        const bTime = b.backups[0]?.created_at ? new Date(b.backups[0].created_at).getTime() : 0
        comparison = bTime - aTime
      }
      return sortOrder === 'asc' ? comparison : -comparison
    })

    return items
  }, [activeTab, containers, volumes, stacks, searchQuery, showOnlyWithBackup, sortField, sortOrder, findTarget, getBackupsForTarget])

  const isLoading =
    (activeTab === 'containers' && containersLoading) ||
    (activeTab === 'volumes' && volumesLoading) ||
    (activeTab === 'stacks' && stacksLoading)

  const getIcon = (type: 'container' | 'volume' | 'stack') => {
    switch (type) {
      case 'container':
        return { icon: ContainerIcon, color: 'bg-blue-500' }
      case 'volume':
        return { icon: HardDrive, color: 'bg-purple-500' }
      case 'stack':
        return { icon: Layers, color: 'bg-orange-500' }
    }
  }

  const renderDetails = (item: (typeof filteredItems)[0]) => {
    if (item.type === 'container') {
      const container = item.data as Container
      return (
        <div className="grid grid-cols-2 gap-2 text-sm">
          <div>
            <span className="text-dark-400">Status:</span>{' '}
            <span className={container.status === 'running' ? 'text-green-400' : 'text-dark-300'}>
              {container.status}
            </span>
          </div>
          {container.compose_project && (
            <div>
              <span className="text-dark-400">Stack:</span>{' '}
              <span className="text-primary-400">{container.compose_project}</span>
            </div>
          )}
          {container.mounts.filter((m) => m.type === 'volume').length > 0 && (
            <div className="col-span-2">
              <span className="text-dark-400">Volumes:</span>{' '}
              <span className="text-dark-300">
                {container.mounts
                  .filter((m) => m.type === 'volume')
                  .map((m) => m.name)
                  .join(', ')}
              </span>
            </div>
          )}
        </div>
      )
    } else if (item.type === 'volume') {
      const volume = item.data as Volume
      return (
        <div className="text-sm">
          {volume.used_by.length > 0 && (
            <div>
              <span className="text-dark-400">Used by:</span>{' '}
              <span className="text-dark-300">{volume.used_by.join(', ')}</span>
            </div>
          )}
        </div>
      )
    } else if (item.type === 'stack') {
      const stack = item.data as Stack
      return (
        <div className="text-sm space-y-2">
          <div>
            <span className="text-dark-400">Containers:</span>
            <div className="flex flex-wrap gap-1 mt-1">
              {stack.containers.map((c) => (
                <span
                  key={c.id}
                  className={clsx(
                    'px-2 py-0.5 rounded text-xs',
                    c.status === 'running' ? 'bg-green-500/10 text-green-400' : 'bg-dark-700 text-dark-400'
                  )}
                >
                  {c.compose_service || c.name}
                </span>
              ))}
            </div>
          </div>
          {stack.volumes.length > 0 && (
            <div>
              <span className="text-dark-400">Volumes:</span>{' '}
              <span className="text-dark-300">{stack.volumes.join(', ')}</span>
            </div>
          )}
        </div>
      )
    }
    return null
  }

  const tabs: { key: TabType; label: string; icon: typeof ContainerIcon; count: number }[] = [
    { key: 'containers', label: 'Containers', icon: ContainerIcon, count: containers?.length || 0 },
    { key: 'volumes', label: 'Volumes', icon: HardDrive, count: volumes?.length || 0 },
    { key: 'stacks', label: 'Stacks', icon: Layers, count: stacks?.length || 0 },
  ]

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Backups</h1>
        <p className="text-dark-400 mt-1">Manage backups for your Docker resources</p>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 bg-dark-800 p-1 rounded-xl border border-dark-700">
        {tabs.map((tab) => (
          <button
            key={tab.key}
            onClick={() => setActiveTab(tab.key)}
            className={clsx(
              'flex-1 flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg text-sm font-medium transition-colors',
              activeTab === tab.key
                ? 'bg-primary-500 text-white'
                : 'text-dark-400 hover:text-dark-200 hover:bg-dark-700'
            )}
          >
            <tab.icon className="w-4 h-4" />
            {tab.label}
            <span
              className={clsx(
                'px-1.5 py-0.5 rounded-full text-xs',
                activeTab === tab.key ? 'bg-white/20' : 'bg-dark-700'
              )}
            >
              {tab.count}
            </span>
          </button>
        ))}
      </div>

      {/* Filters */}
      <div className="flex flex-wrap items-center gap-4">
        {/* Search */}
        <div className="relative flex-1 min-w-[200px]">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-400" />
          <input
            type="text"
            placeholder="Search..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 bg-dark-800 border border-dark-700 rounded-lg text-dark-100 placeholder-dark-500 focus:outline-none focus:ring-2 focus:ring-primary-500/50"
          />
        </div>

        {/* Sort */}
        <div className="flex items-center gap-2">
          <select
            value={sortField}
            onChange={(e) => setSortField(e.target.value as SortField)}
            className="px-3 py-2 bg-dark-800 border border-dark-700 rounded-lg text-dark-200 text-sm focus:outline-none focus:ring-2 focus:ring-primary-500/50"
          >
            <option value="name">Name</option>
            <option value="status">Backup Status</option>
            <option value="lastBackup">Last Backup</option>
          </select>
          <button
            onClick={() => setSortOrder(sortOrder === 'asc' ? 'desc' : 'asc')}
            className="p-2 bg-dark-800 border border-dark-700 rounded-lg text-dark-400 hover:text-dark-200 transition-colors"
          >
            {sortOrder === 'asc' ? <SortAsc className="w-4 h-4" /> : <SortDesc className="w-4 h-4" />}
          </button>
        </div>

        {/* Filter toggle */}
        <label className="flex items-center gap-2 text-sm text-dark-300 cursor-pointer">
          <input
            type="checkbox"
            checked={showOnlyWithBackup}
            onChange={(e) => setShowOnlyWithBackup(e.target.checked)}
            className="w-4 h-4 rounded border-dark-600 bg-dark-700 text-primary-500 focus:ring-primary-500/50"
          />
          Only with backup
        </label>
      </div>

      {/* Items List */}
      <div className="space-y-3">
        {isLoading ? (
          [...Array(3)].map((_, i) => (
            <div key={i} className="h-20 bg-dark-800 rounded-xl animate-pulse" />
          ))
        ) : filteredItems.length === 0 ? (
          <div className="text-center py-12 bg-dark-800 rounded-xl border border-dark-700">
            <Archive className="w-12 h-12 text-dark-500 mx-auto mb-4" />
            <p className="text-dark-400">
              {searchQuery ? 'No items match your search' : `No ${activeTab} found`}
            </p>
          </div>
        ) : (
          filteredItems.map((item) => {
            const { icon, color } = getIcon(item.type)
            return (
              <ItemCard
                key={`${item.type}-${item.name}`}
                name={item.name}
                subtitle={item.subtitle}
                icon={icon}
                iconColor={color}
                target={item.target}
                backups={item.backups}
                onSetupBackup={() => {
                  const stack = item.type === 'stack' ? (item.data as Stack) : null
                  createTargetMutation.mutate({
                    type: item.type,
                    name: item.name,
                    dependencies: stack?.containers.map((c) => c.name),
                  })
                }}
                details={renderDetails(item)}
              />
            )
          })
        )}
      </div>

      {/* Stats */}
      <div className="grid grid-cols-3 gap-4">
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4 text-center">
          <p className="text-2xl font-bold text-green-500">
            {targets?.filter((t) => t.enabled).length || 0}
          </p>
          <p className="text-sm text-dark-400">Active Backups</p>
        </div>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4 text-center">
          <p className="text-2xl font-bold text-primary-500">
            {backups?.filter((b) => b.status === 'completed').length || 0}
          </p>
          <p className="text-sm text-dark-400">Completed</p>
        </div>
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4 text-center">
          <p className="text-2xl font-bold text-red-500">
            {backups?.filter((b) => b.status === 'failed').length || 0}
          </p>
          <p className="text-sm text-dark-400">Failed</p>
        </div>
      </div>
    </div>
  )
}
