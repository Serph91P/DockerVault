import { useQuery } from '@tanstack/react-query'
import { Archive, Container, HardDrive, Clock, AlertCircle, CheckCircle } from 'lucide-react'
import { dockerApi, backupsApi, targetsApi, schedulesApi } from '../api'
import { formatDistanceToNow } from 'date-fns'

function StatCard({
  title,
  value,
  icon: Icon,
  color,
}: {
  title: string
  value: string | number
  icon: React.ElementType
  color: string
}) {
  return (
    <div className="bg-dark-800 rounded-xl p-6 border border-dark-700">
      <div className="flex items-center justify-between">
        <div>
          <p className="text-sm text-dark-400">{title}</p>
          <p className="text-2xl font-bold text-dark-100 mt-1">{value}</p>
        </div>
        <div className={`w-12 h-12 rounded-lg flex items-center justify-center ${color}`}>
          <Icon className="w-6 h-6 text-white" />
        </div>
      </div>
    </div>
  )
}

export default function Dashboard() {
  const { data: containers } = useQuery({
    queryKey: ['containers'],
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
  })

  const { data: volumes } = useQuery({
    queryKey: ['volumes'],
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
  })

  const { data: targets } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  const { data: backups } = useQuery({
    queryKey: ['backups'],
    queryFn: () => backupsApi.list({ limit: 10 }).then((r) => r.data),
  })

  const { data: schedules } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })

  const runningContainers = containers?.filter((c) => c.status === 'running').length ?? 0
  const activeTargets = targets?.filter((t) => t.enabled).length ?? 0
  const completedBackups = backups?.filter((b) => b.status === 'completed').length ?? 0
  const failedBackups = backups?.filter((b) => b.status === 'failed').length ?? 0

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Dashboard</h1>
        <p className="text-dark-400 mt-1">Overview of your Docker backups</p>
      </div>

      {/* Stats Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard
          title="Container"
          value={`${runningContainers}/${containers?.length ?? 0}`}
          icon={Container}
          color="bg-blue-500"
        />
        <StatCard
          title="Volumes"
          value={volumes?.length ?? 0}
          icon={HardDrive}
          color="bg-purple-500"
        />
        <StatCard
          title="Backup Targets"
          value={activeTargets}
          icon={Archive}
          color="bg-green-500"
        />
        <StatCard
          title="Scheduled Backups"
          value={schedules?.filter((s) => s.enabled).length ?? 0}
          icon={Clock}
          color="bg-orange-500"
        />
      </div>

      {/* Recent Backups & Next Scheduled */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Recent Backups */}
        <div className="bg-dark-800 rounded-xl border border-dark-700">
          <div className="px-6 py-4 border-b border-dark-700">
            <h2 className="text-lg font-semibold text-dark-100">Recent Backups</h2>
          </div>
          <div className="divide-y divide-dark-700">
            {backups?.slice(0, 5).map((backup) => (
              <div key={backup.id} className="px-6 py-4 flex items-center justify-between">
                <div className="flex items-center gap-3">
                  {backup.status === 'completed' ? (
                    <CheckCircle className="w-5 h-5 text-green-500" />
                  ) : backup.status === 'failed' ? (
                    <AlertCircle className="w-5 h-5 text-red-500" />
                  ) : (
                    <Clock className="w-5 h-5 text-yellow-500 animate-pulse" />
                  )}
                  <div>
                    <p className="text-sm font-medium text-dark-100">
                      {backup.target_name || `Target #${backup.target_id}`}
                    </p>
                    <p className="text-xs text-dark-400">
                      {formatDistanceToNow(new Date(backup.created_at), {
                        addSuffix: true,
                      })}
                    </p>
                  </div>
                </div>
                <div className="text-right">
                  <p className="text-sm text-dark-300">{backup.file_size_human || '-'}</p>
                  <p className="text-xs text-dark-400">
                    {backup.duration_seconds ? `${backup.duration_seconds}s` : '-'}
                  </p>
                </div>
              </div>
            ))}
            {(!backups || backups.length === 0) && (
              <div className="px-6 py-8 text-center text-dark-400">
                No backups yet
              </div>
            )}
          </div>
        </div>

        {/* Next Scheduled Backups */}
        <div className="bg-dark-800 rounded-xl border border-dark-700">
          <div className="px-6 py-4 border-b border-dark-700">
            <h2 className="text-lg font-semibold text-dark-100">Upcoming Scheduled Backups</h2>
          </div>
          <div className="divide-y divide-dark-700">
            {schedules
              ?.filter((s) => s.enabled && s.next_run)
              .sort((a, b) => 
                new Date(a.next_run!).getTime() - new Date(b.next_run!).getTime()
              )
              .slice(0, 5)
              .map((schedule) => (
                <div key={schedule.id} className="px-6 py-4 flex items-center justify-between">
                  <div>
                    <p className="text-sm font-medium text-dark-100">{schedule.name}</p>
                    <p className="text-xs text-dark-400 font-mono">{schedule.cron_expression}</p>
                    {schedule.target_count > 0 && (
                      <p className="text-xs text-dark-500">{schedule.target_count} target(s)</p>
                    )}
                  </div>
                  <div className="text-right">
                    <p className="text-sm text-dark-300">
                      {schedule.next_run &&
                        formatDistanceToNow(new Date(schedule.next_run), {
                          addSuffix: true,
                        })}
                    </p>
                  </div>
                </div>
              ))}
            {(!schedules || schedules.filter((s) => s.enabled).length === 0) && (
              <div className="px-6 py-8 text-center text-dark-400">
                No scheduled backups
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Status Summary */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
        <h2 className="text-lg font-semibold text-dark-100 mb-4">Status Summary</h2>
        <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
          <div className="text-center">
            <p className="text-3xl font-bold text-green-500">{completedBackups}</p>
            <p className="text-sm text-dark-400">Successful</p>
          </div>
          <div className="text-center">
            <p className="text-3xl font-bold text-red-500">{failedBackups}</p>
            <p className="text-sm text-dark-400">Failed</p>
          </div>
          <div className="text-center">
            <p className="text-3xl font-bold text-blue-500">{runningContainers}</p>
            <p className="text-sm text-dark-400">Active Containers</p>
          </div>
          <div className="text-center">
            <p className="text-3xl font-bold text-purple-500">{volumes?.length ?? 0}</p>
            <p className="text-sm text-dark-400">Volumes</p>
          </div>
        </div>
      </div>
    </div>
  )
}
