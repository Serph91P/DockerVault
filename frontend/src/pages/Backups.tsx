import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Archive,
  CheckCircle,
  XCircle,
  Clock,
  Loader2,
  Trash2,
  RotateCcw,
} from 'lucide-react'
import { backupsApi, Backup } from '../api'
import { format } from 'date-fns'
import toast from 'react-hot-toast'
import { useWebSocketStore } from '../store/websocket'

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
        return <CheckCircle className="w-5 h-5 text-green-500" />
      case 'failed':
        return <XCircle className="w-5 h-5 text-red-500" />
      case 'running':
        return <Loader2 className="w-5 h-5 text-primary-500 animate-spin" />
      case 'pending':
        return <Clock className="w-5 h-5 text-yellow-500" />
      default:
        return <Archive className="w-5 h-5 text-dark-400" />
    }
  }

  const getStatusText = () => {
    switch (backup.status) {
      case 'completed':
        return 'Completed'
      case 'failed':
        return 'Failed'
      case 'running':
        return 'Running'
      case 'pending':
        return 'Pending'
      case 'cancelled':
        return 'Cancelled'
      default:
        return backup.status
    }
  }

  return (
    <tr className="border-b border-dark-700 hover:bg-dark-750">
      <td className="px-6 py-4">
        <div className="flex items-center gap-3">
          {getStatusIcon()}
          <div>
            <p className="text-sm font-medium text-dark-100">
              {backup.target_name || `Target #${backup.target_id}`}
            </p>
            <p className="text-xs text-dark-400">#{backup.id}</p>
          </div>
        </div>
      </td>
      <td className="px-6 py-4">
        <span
          className={`inline-flex px-2 py-1 rounded-full text-xs font-medium ${
            backup.status === 'completed'
              ? 'bg-green-500/10 text-green-400'
              : backup.status === 'failed'
              ? 'bg-red-500/10 text-red-400'
              : backup.status === 'running'
              ? 'bg-primary-500/10 text-primary-400'
              : 'bg-dark-600 text-dark-400'
          }`}
        >
          {getStatusText()}
        </span>
        {progress && backup.status === 'running' && (
          <div className="mt-2">
            <div className="w-32 h-1 bg-dark-700 rounded-full overflow-hidden">
              <div
                className="h-full progress-animated rounded-full transition-all"
                style={{ width: `${Math.max(0, progress.progress)}%` }}
              />
            </div>
            <p className="text-xs text-dark-400 mt-1">{progress.message}</p>
          </div>
        )}
      </td>
      <td className="px-6 py-4 text-sm text-dark-300">{backup.file_size_human || '-'}</td>
      <td className="px-6 py-4 text-sm text-dark-300">
        {backup.duration_seconds ? `${backup.duration_seconds}s` : '-'}
      </td>
      <td className="px-6 py-4 text-sm text-dark-400">
        {format(new Date(backup.created_at), 'yyyy-MM-dd HH:mm')}
      </td>
      <td className="px-6 py-4">
        <div className="flex items-center gap-2">
          {backup.status === 'completed' && (
            <button
              onClick={() => restoreMutation.mutate()}
              disabled={restoreMutation.isPending}
              className="p-2 text-dark-400 hover:text-primary-400 hover:bg-dark-700 rounded-lg transition-colors"
              title="Restore"
            >
              <RotateCcw className="w-4 h-4" />
            </button>
          )}
          <button
            onClick={() => deleteMutation.mutate()}
            disabled={deleteMutation.isPending}
            className="p-2 text-dark-400 hover:text-red-400 hover:bg-dark-700 rounded-lg transition-colors"
            title="Delete"
          >
            <Trash2 className="w-4 h-4" />
          </button>
        </div>
      </td>
    </tr>
  )
}

export default function Backups() {
  const { data: backups, isLoading } = useQuery({
    queryKey: ['backups'],
    queryFn: () => backupsApi.list({ limit: 50 }).then((r) => r.data),
    refetchInterval: 5000, // Refresh every 5 seconds
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Backups</h1>
        <p className="text-dark-400 mt-1">All created backups</p>
      </div>

      <div className="bg-dark-800 rounded-xl border border-dark-700 overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead className="bg-dark-750 text-left">
              <tr>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Target
                </th>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Status
                </th>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Size
                </th>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Duration
                </th>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Created
                </th>
                <th className="px-6 py-3 text-xs font-medium text-dark-400 uppercase tracking-wider">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-700">
              {isLoading
                ? [...Array(5)].map((_, i) => (
                    <tr key={i}>
                      <td className="px-6 py-4" colSpan={6}>
                        <div className="h-8 bg-dark-700 rounded animate-pulse" />
                      </td>
                    </tr>
                  ))
                : backups?.map((backup) => (
                    <BackupRow key={backup.id} backup={backup} />
                  ))}
            </tbody>
          </table>
        </div>

        {!isLoading && backups?.length === 0 && (
          <div className="p-12 text-center">
            <Archive className="w-12 h-12 text-dark-500 mx-auto mb-4" />
            <p className="text-dark-400">No backups yet</p>
          </div>
        )}
      </div>
    </div>
  )
}
