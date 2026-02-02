import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Layers, Plus, Container, HardDrive, Network } from 'lucide-react'
import { dockerApi, targetsApi, Stack } from '../api'
import toast from 'react-hot-toast'

function StackCard({ stack }: { stack: Stack }) {
  const queryClient = useQueryClient()

  const addTargetMutation = useMutation({
    mutationFn: () =>
      targetsApi.create({
        name: `${stack.name}-stack-backup`,
        target_type: 'stack',
        stack_name: stack.name,
        enabled: true,
        stop_container: true,
        compression_enabled: true,
        dependencies: stack.containers.map((c) => c.name),
      }),
    onSuccess: () => {
      toast.success('Stack backup target created')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Failed to create target'),
  })

  const runningContainers = stack.containers.filter((c) => c.status === 'running').length

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-primary-500/10 rounded-lg flex items-center justify-center">
            <Layers className="w-5 h-5 text-primary-400" />
          </div>
          <div>
            <h3 className="text-lg font-semibold text-dark-100">{stack.name}</h3>
            <p className="text-sm text-dark-400">
              {runningContainers}/{stack.containers.length} containers active
            </p>
          </div>
        </div>
      </div>

      {/* Containers */}
      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-2 flex items-center gap-1">
          <Container className="w-3 h-3" />
          Container:
        </p>
        <div className="space-y-1">
          {stack.containers.map((container) => (
            <div
              key={container.id}
              className="flex items-center justify-between text-sm bg-dark-700 rounded px-2 py-1"
            >
              <span className="text-dark-300">{container.compose_service || container.name}</span>
              <span
                className={`text-xs ${
                  container.status === 'running' ? 'text-green-400' : 'text-dark-400'
                }`}
              >
                {container.status}
              </span>
            </div>
          ))}
        </div>
      </div>

      {/* Volumes */}
      {stack.volumes.length > 0 && (
        <div className="mb-4">
          <p className="text-xs text-dark-400 mb-2 flex items-center gap-1">
            <HardDrive className="w-3 h-3" />
            Volumes:
          </p>
          <div className="flex flex-wrap gap-1">
            {stack.volumes.map((volume) => (
              <span
                key={volume}
                className="text-xs bg-purple-500/10 text-purple-400 rounded px-2 py-1"
              >
                {volume}
              </span>
            ))}
          </div>
        </div>
      )}

      {/* Networks */}
      {stack.networks.length > 0 && (
        <div className="mb-4">
          <p className="text-xs text-dark-400 mb-2 flex items-center gap-1">
            <Network className="w-3 h-3" />
            Networks:
          </p>
          <div className="flex flex-wrap gap-1">
            {stack.networks.map((network) => (
              <span
                key={network}
                className="text-xs bg-dark-600 text-dark-300 rounded px-2 py-1"
              >
                {network}
              </span>
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
        Add Entire Stack as Backup Target
      </button>
    </div>
  )
}

export default function Stacks() {
  const { data: stacks, isLoading } = useQuery({
    queryKey: ['stacks'],
    queryFn: () => dockerApi.listStacks().then((r) => r.data),
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Stacks</h1>
        <p className="text-dark-400 mt-1">Docker Compose stacks on this host</p>
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
          {stacks?.map((stack) => (
            <StackCard key={stack.name} stack={stack} />
          ))}
        </div>
      )}

      {!isLoading && stacks?.length === 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
          <Layers className="w-12 h-12 text-dark-500 mx-auto mb-4" />
          <p className="text-dark-400">No Docker Compose stacks found</p>
          <p className="text-sm text-dark-500 mt-2">
            Stacks are detected by com.docker.compose.project labels
          </p>
        </div>
      )}
    </div>
  )
}
