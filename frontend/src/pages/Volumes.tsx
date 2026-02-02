import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { HardDrive, Plus, Container } from 'lucide-react'
import { dockerApi, targetsApi, Volume } from '../api'
import toast from 'react-hot-toast'

function VolumeCard({ volume }: { volume: Volume }) {
  const queryClient = useQueryClient()

  const addTargetMutation = useMutation({
    mutationFn: () =>
      targetsApi.create({
        name: `${volume.name}-backup`,
        target_type: 'volume',
        volume_name: volume.name,
        enabled: true,
        stop_container: true,
        compression_enabled: true,
        dependencies: volume.used_by,
      }),
    onSuccess: () => {
      toast.success('Backup target created')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to create target'),
  })

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-purple-500/10 rounded-lg flex items-center justify-center">
            <HardDrive className="w-5 h-5 text-purple-400" />
          </div>
          <div>
            <h3 className="text-lg font-semibold text-dark-100">{volume.name}</h3>
            <p className="text-sm text-dark-400">{volume.driver}</p>
          </div>
        </div>
      </div>

      {/* Mountpoint */}
      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-1">Mountpoint:</p>
        <p className="text-sm text-dark-300 font-mono bg-dark-700 rounded px-2 py-1 break-all">
          {volume.mountpoint}
        </p>
      </div>

      {/* Used By */}
      {volume.used_by.length > 0 && (
        <div className="mb-4">
          <p className="text-xs text-dark-400 mb-2">Used by:</p>
          <div className="flex flex-wrap gap-2">
            {volume.used_by.map((containerName) => (
              <span
                key={containerName}
                className="flex items-center gap-1 text-xs bg-dark-700 rounded px-2 py-1"
              >
                <Container className="w-3 h-3 text-blue-400" />
                {containerName}
              </span>
            ))}
          </div>
        </div>
      )}

      {/* Labels */}
      {Object.keys(volume.labels).length > 0 && (
        <div className="mb-4">
          <p className="text-xs text-dark-400 mb-2">Labels:</p>
          <div className="space-y-1">
            {Object.entries(volume.labels).slice(0, 3).map(([key, value]) => (
              <div key={key} className="text-xs">
                <span className="text-dark-400">{key}: </span>
                <span className="text-dark-300">{value}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Actions */}
      <button
        onClick={() => addTargetMutation.mutate()}
        disabled={addTargetMutation.isPending}
        className="flex items-center gap-2 px-3 py-2 bg-primary-500/10 text-primary-400 rounded-lg hover:bg-primary-500/20 transition-colors text-sm w-full justify-center"
      >
        <Plus className="w-4 h-4" />
        Add as Backup Target
      </button>
    </div>
  )
}

export default function Volumes() {
  const { data: volumes, isLoading } = useQuery({
    queryKey: ['volumes'],
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Volumes</h1>
        <p className="text-dark-400 mt-1">All Docker volumes on this host</p>
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
          {volumes?.map((volume) => (
            <VolumeCard key={volume.name} volume={volume} />
          ))}
        </div>
      )}

      {!isLoading && volumes?.length === 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
          <HardDrive className="w-12 h-12 text-dark-500 mx-auto mb-4" />
          <p className="text-dark-400">No volumes found</p>
        </div>
      )}
    </div>
  )
}
