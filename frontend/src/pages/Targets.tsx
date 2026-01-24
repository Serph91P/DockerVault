import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Target, Trash2, Play, Clock } from 'lucide-react'
import { targetsApi, backupsApi, BackupTarget } from '../api'
import toast from 'react-hot-toast'

function TargetCard({ target }: { target: BackupTarget }) {
  const queryClient = useQueryClient()

  const triggerBackupMutation = useMutation({
    mutationFn: () => backupsApi.create(target.id),
    onSuccess: () => {
      toast.success('Backup gestartet')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: () => toast.error('Fehler beim Starten'),
  })

  const toggleMutation = useMutation({
    mutationFn: () => targetsApi.update(target.id, { enabled: !target.enabled }),
    onSuccess: () => {
      toast.success(target.enabled ? 'Target deaktiviert' : 'Target aktiviert')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Fehler beim Aktualisieren'),
  })

  const deleteMutation = useMutation({
    mutationFn: () => targetsApi.delete(target.id),
    onSuccess: () => {
      toast.success('Target gelöscht')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
    },
    onError: () => toast.error('Fehler beim Löschen'),
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
          {target.enabled ? 'Aktiv' : 'Inaktiv'}
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
            <span className="text-dark-400">Pfad: </span>
            <span className="text-dark-200 font-mono">{target.host_path}</span>
          </div>
        )}
        {target.stack_name && (
          <div>
            <span className="text-dark-400">Stack: </span>
            <span className="text-dark-200">{target.stack_name}</span>
          </div>
        )}
        {target.schedule_cron && (
          <div className="flex items-center gap-1">
            <Clock className="w-3 h-3 text-dark-400" />
            <span className="text-dark-400">Schedule: </span>
            <span className="text-dark-200 font-mono">{target.schedule_cron}</span>
          </div>
        )}
      </div>

      {/* Settings */}
      <div className="flex flex-wrap gap-2 mb-4">
        {target.stop_container && (
          <span className="text-xs bg-orange-500/10 text-orange-400 rounded px-2 py-1">
            Stoppt Container
          </span>
        )}
        {target.compression_enabled && (
          <span className="text-xs bg-blue-500/10 text-blue-400 rounded px-2 py-1">
            Komprimiert
          </span>
        )}
        {target.dependencies.length > 0 && (
          <span className="text-xs bg-purple-500/10 text-purple-400 rounded px-2 py-1">
            {target.dependencies.length} Abhängigkeiten
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
          Backup starten
        </button>
        <button
          onClick={() => deleteMutation.mutate()}
          disabled={deleteMutation.isPending}
          className="px-3 py-2 bg-red-500/10 text-red-400 rounded-lg hover:bg-red-500/20 transition-colors"
        >
          <Trash2 className="w-4 h-4" />
        </button>
      </div>
    </div>
  )
}

export default function Targets() {
  const { data: targets, isLoading } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Backup Targets</h1>
          <p className="text-dark-400 mt-1">Konfigurierte Backup-Ziele</p>
        </div>
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
            <TargetCard key={target.id} target={target} />
          ))}
        </div>
      )}

      {!isLoading && targets?.length === 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
          <Target className="w-12 h-12 text-dark-500 mx-auto mb-4" />
          <p className="text-dark-400">Keine Backup Targets konfiguriert</p>
          <p className="text-sm text-dark-500 mt-2">
            Gehe zu Container, Volumes oder Stacks um Backup Targets hinzuzufügen
          </p>
        </div>
      )}
    </div>
  )
}
