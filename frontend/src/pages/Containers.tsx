import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Play, Square, RefreshCw, Plus } from 'lucide-react'
import { dockerApi, targetsApi, Container } from '../api'
import toast from 'react-hot-toast'
import { useState } from 'react'

function ContainerCard({ container }: { container: Container }) {
  const queryClient = useQueryClient()
  const [, setShowAddTarget] = useState(false)

  const stopMutation = useMutation({
    mutationFn: () => dockerApi.stopContainer(container.id),
    onSuccess: () => {
      toast.success(`Container ${container.name} stopped`)
      queryClient.invalidateQueries({ queryKey: ['containers'] })
    },
    onError: () => toast.error('Failed to stop container'),
  })

  const startMutation = useMutation({
    mutationFn: () => dockerApi.startContainer(container.id),
    onSuccess: () => {
      toast.success(`Container ${container.name} started`)
      queryClient.invalidateQueries({ queryKey: ['containers'] })
    },
    onError: () => toast.error('Failed to start container'),
  })

  const addTargetMutation = useMutation({
    mutationFn: () =>
      targetsApi.create({
        name: `${container.name}-backup`,
        target_type: 'container',
        container_name: container.name,
        enabled: true,
        stop_container: true,
        compression_enabled: true,
      }),
    onSuccess: () => {
      toast.success('Backup target created')
      setShowAddTarget(false)
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to create target'),
  })

  const isRunning = container.status === 'running'
  const volumeMounts = container.mounts.filter((m) => m.type === 'volume')

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div>
          <h3 className="text-lg font-semibold text-dark-100">{container.name}</h3>
          <p className="text-sm text-dark-400">{container.image}</p>
        </div>
        <span
          className={`px-2 py-1 rounded-full text-xs font-medium ${
            isRunning ? 'bg-green-500/10 text-green-400' : 'bg-dark-600 text-dark-400'
          }`}
        >
          {container.status}
        </span>
      </div>

      {/* Compose Info */}
      {container.compose_project && (
        <div className="mb-4 text-sm">
          <span className="text-dark-400">Stack: </span>
          <span className="text-primary-400">{container.compose_project}</span>
          {container.compose_service && (
            <>
              <span className="text-dark-400"> / </span>
              <span className="text-dark-300">{container.compose_service}</span>
            </>
          )}
        </div>
      )}

      {/* Volumes */}
      {volumeMounts.length > 0 && (
        <div className="mb-4">
          <p className="text-xs text-dark-400 mb-2">Volumes:</p>
          <div className="space-y-1">
            {volumeMounts.map((mount, i) => (
              <div key={i} className="text-xs bg-dark-700 rounded px-2 py-1">
                <span className="text-primary-400">{mount.name}</span>
                <span className="text-dark-400"> → </span>
                <span className="text-dark-300">{mount.destination}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Actions */}
      <div className="flex gap-2">
        {isRunning ? (
          <button
            onClick={() => stopMutation.mutate()}
            disabled={stopMutation.isPending}
            className="flex items-center gap-2 px-3 py-2 bg-red-500/10 text-red-400 rounded-lg hover:bg-red-500/20 transition-colors text-sm"
          >
            <Square className="w-4 h-4" />
            Stop
          </button>
        ) : (
          <button
            onClick={() => startMutation.mutate()}
            disabled={startMutation.isPending}
            className="flex items-center gap-2 px-3 py-2 bg-green-500/10 text-green-400 rounded-lg hover:bg-green-500/20 transition-colors text-sm"
          >
            <Play className="w-4 h-4" />
            Start
          </button>
        )}
        <button
          onClick={() => addTargetMutation.mutate()}
          disabled={addTargetMutation.isPending}
          className="flex items-center gap-2 px-3 py-2 bg-primary-500/10 text-primary-400 rounded-lg hover:bg-primary-500/20 transition-colors text-sm"
        >
          <Plus className="w-4 h-4" />
          Backup Target
        </button>
      </div>
    </div>
  )
}

export default function Containers() {
  const { data: containers, isLoading, refetch } = useQuery({
    queryKey: ['containers'],
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
  })

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Containers</h1>
          <p className="text-dark-400 mt-1">All Docker containers on this host</p>
        </div>
        <button
          onClick={() => refetch()}
          className="flex items-center gap-2 px-4 py-2 bg-dark-700 text-dark-200 rounded-lg hover:bg-dark-600 transition-colors"
        >
          <RefreshCw className="w-4 h-4" />
          Refresh
        </button>
      </div>

      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {[...Array(6)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse">
              <div className="h-6 bg-dark-700 rounded w-1/2 mb-2" />
              <div className="h-4 bg-dark-700 rounded w-3/4" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {containers?.map((container) => (
            <ContainerCard key={container.id} container={container} />
          ))}
        </div>
      )}
    </div>
  )
}
