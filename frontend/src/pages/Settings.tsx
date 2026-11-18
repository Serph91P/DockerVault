import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Check, X, RefreshCw, HardDrive, Shield, Clock, Database, Archive, Upload } from 'lucide-react'
import { dockerApi, settingsApi } from '../api'
import toast from 'react-hot-toast'
import EncryptionSetup from '../components/EncryptionSetup'
import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'

function StorageSettingsCard() {
  const queryClient = useQueryClient()
  const { data: storageSettings, ...storageQuery } = useQuery({
    queryKey: queryKeys.settings.storage,
    queryFn: () => settingsApi.getStorageSettings().then((r) => r.data),
  })

  const updateMutation = useMutation({
    mutationFn: (data: { check_quota_before_upload: boolean }) =>
      settingsApi.updateStorageSettings(data),
    onSuccess: () => {
      toast.success('Storage settings saved')
      queryClient.invalidateQueries({ queryKey: queryKeys.settings.storage })
    },
    onError: () => toast.error('Failed to save storage settings'),
  })


  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-center gap-3 mb-4">
        <div className="w-10 h-10 bg-orange-500/10 rounded-lg flex items-center justify-center">
          <Upload className="w-5 h-5 text-orange-400" />
        </div>
        <div>
          <h2 className="text-lg font-semibold text-dark-100">Storage & Upload</h2>
          <p className="text-sm text-dark-400">Upload behavior settings</p>
        </div>
      </div>

      <QueryFeedback name="Storage settings" query={storageQuery} />
      <div className="space-y-4" hidden={!storageSettings}>
        <div className="flex items-center justify-between">
          <div>
            <label className="text-sm text-dark-300">Check quota before upload</label>
            <p className="text-xs text-dark-500">
              Verify available space on remote storage before uploading.
              Disable if your storage server doesn't support quota queries.
            </p>
          </div>
          <button
            role="switch"
            aria-label="Check quota before upload"
            aria-checked={storageSettings?.check_quota_before_upload ?? false}
            disabled={updateMutation.isPending}
            onClick={() =>
              updateMutation.mutate({
                check_quota_before_upload: !(
                  storageSettings?.check_quota_before_upload ?? false
                ),
              })
            }
            className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${
              storageSettings?.check_quota_before_upload ?? false
                ? 'bg-primary-500'
                : 'bg-dark-600'
            }`}
          >
            <span
              className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                storageSettings?.check_quota_before_upload ?? false
                  ? 'translate-x-6'
                  : 'translate-x-1'
              }`}
            />
          </button>
        </div>
      </div>
    </div>
  )
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (days > 0) return `${days}d ${hours}h ${minutes}m`
  if (hours > 0) return `${hours}h ${minutes}m`
  return `${minutes}m`
}

function SystemInfoCard() {
  const { data: systemInfo, ...systemQuery } = useQuery({
    queryKey: queryKeys.systemInfo,
    queryFn: () => settingsApi.getSystemInfo().then((r) => r.data),
    refetchInterval: 30000,
  })

  const diskPercent = systemInfo && systemInfo.disk_total > 0
    ? Math.round((systemInfo.disk_used / systemInfo.disk_total) * 100)
    : 0

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-center gap-3 mb-4">
        <div className="w-10 h-10 bg-green-500/10 rounded-lg flex items-center justify-center">
          <HardDrive className="w-5 h-5 text-green-400" />
        </div>
        <div className="flex-1">
          <h2 className="text-lg font-semibold text-dark-100">System Information</h2>
          <p className="text-sm text-dark-400">Runtime & storage details</p>
        </div>
        {systemInfo && (
          <span className="text-xs text-dark-500 font-mono">v{systemInfo.app_version}</span>
        )}
      </div>

      <QueryFeedback name="System information" query={systemQuery} />
      {systemInfo ? (
        <>
          {/* Disk Usage Bar */}
          <div className="mb-4 p-3 bg-dark-900 rounded-lg">
            <div className="flex items-center justify-between text-sm mb-2">
              <span className="text-dark-400">Disk Usage</span>
              <span className="text-dark-200">
                {formatBytes(systemInfo.disk_used)} / {formatBytes(systemInfo.disk_total)}
              </span>
            </div>
            <div className="h-2 bg-dark-700 rounded-full overflow-hidden">
              <div
                className={`h-full rounded-full transition-all ${
                  diskPercent > 90 ? 'bg-red-500' : diskPercent > 70 ? 'bg-yellow-500' : 'bg-green-500'
                }`}
                style={{ width: `${diskPercent}%` }}
              />
            </div>
            <div className="flex justify-between text-xs text-dark-500 mt-1">
              <span>{diskPercent}% used</span>
              <span>{formatBytes(systemInfo.disk_free)} free</span>
            </div>
          </div>

          {/* Stats Grid */}
          <div className="grid grid-cols-3 gap-3 mb-4">
            <div className="bg-dark-900 rounded-lg p-3 text-center">
              <Archive className="w-4 h-4 text-primary-400 mx-auto mb-1" />
              <p className="text-lg font-semibold text-dark-100">{systemInfo.backup_count}</p>
              <p className="text-xs text-dark-500">Backups</p>
            </div>
            <div className="bg-dark-900 rounded-lg p-3 text-center">
              <Database className="w-4 h-4 text-blue-400 mx-auto mb-1" />
              <p className="text-lg font-semibold text-dark-100">{systemInfo.target_count}</p>
              <p className="text-xs text-dark-500">Targets</p>
            </div>
            <div className="bg-dark-900 rounded-lg p-3 text-center">
              <Clock className="w-4 h-4 text-green-400 mx-auto mb-1" />
              <p className="text-lg font-semibold text-dark-100">{formatUptime(systemInfo.uptime_seconds)}</p>
              <p className="text-xs text-dark-500">Uptime</p>
            </div>
          </div>

          {/* Details */}
          <div className="space-y-3 text-sm">
            <div className="flex justify-between py-2 border-b border-dark-700">
              <span className="text-dark-400">Backup Directory</span>
              <span className="text-dark-200 font-mono">{systemInfo.backup_dir}</span>
            </div>
            <div className="flex justify-between py-2 border-b border-dark-700">
              <span className="text-dark-400">Database</span>
              <span className="text-dark-200">
                <span className="font-mono">{systemInfo.database_path}</span>
                <span className="text-dark-500 ml-2">({formatBytes(systemInfo.db_size)})</span>
              </span>
            </div>
            <div className="flex justify-between py-2">
              <span className="text-dark-400">Timezone</span>
              <span className="text-dark-200">{systemInfo.timezone}</span>
            </div>
          </div>
        </>
      ) : (
        <div className="space-y-3 animate-pulse">
          <div className="h-16 bg-dark-700 rounded-lg" />
          <div className="grid grid-cols-3 gap-3">
            <div className="h-20 bg-dark-700 rounded-lg" />
            <div className="h-20 bg-dark-700 rounded-lg" />
            <div className="h-20 bg-dark-700 rounded-lg" />
          </div>
        </div>
      )}

      <div className="mt-4 p-4 bg-dark-700/50 rounded-lg flex items-start gap-3">
        <Shield className="w-5 h-5 text-primary-400 flex-shrink-0 mt-0.5" />
        <div className="text-sm text-dark-300">
          <p className="font-medium text-dark-200 mb-1">Retention Policies</p>
          <p>
            Configure backup retention (how many backups to keep) on the{' '}
            <a href="/protection/retention" className="text-primary-300 underline underline-offset-2">
              Retention
            </a>{' '}
            page. You can create multiple policies and assign them to targets.
          </p>
        </div>
      </div>
    </div>
  )
}

export default function Settings() {
  const { hash } = useLocation()
  useEffect(() => {
    if (hash === '#connection' || hash === '#encryption') document.getElementById(hash.slice(1))?.focus()
  }, [hash])
  const { data: dockerHealth, refetch: refetchDocker, ...dockerQuery } = useQuery({
    queryKey: ['docker-health'],
    queryFn: () => dockerApi.getHealth().then(response => response.data.status === 'healthy'),
    refetchInterval: 30000,
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Settings</h1>
        <p className="text-dark-400 mt-1">Configuration and integrations</p>
      </div>

      {/* Docker Status */}
      <section id="connection" tabIndex={-1} aria-label="Docker connection settings" className="bg-dark-800 rounded-xl border border-dark-700 p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 bg-blue-500/10 rounded-lg flex items-center justify-center">
              🐳
            </div>
            <div>
              <h2 className="text-lg font-semibold text-dark-100">Docker Connection</h2>
              <p className="text-sm text-dark-400">Docker socket connection status</p>
            </div>
          </div>
          <button
            onClick={() => refetchDocker()}
            aria-label="Refresh Docker status"
            className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
          >
            <RefreshCw className="w-4 h-4" />
          </button>
        </div>

        <div className="flex items-center gap-2">
          <QueryFeedback name="Docker" query={{ ...dockerQuery, refetch: refetchDocker }} />
          {dockerHealth === undefined ? <span>Unknown</span> : dockerHealth ? (
            <>
              <Check className="w-5 h-5 text-green-500" />
              <span className="text-green-400">Connected</span>
            </>
          ) : (
            <>
              <X className="w-5 h-5 text-red-500" />
              <span className="text-red-400">Not connected</span>
            </>
          )}
        </div>

        <div className="mt-4 text-sm text-dark-400">
          <p>Docker Socket: /var/run/docker.sock</p>
        </div>
      </section>

      {/* Encryption Settings */}
      <section id="encryption" tabIndex={-1} aria-label="Encryption settings"><EncryptionSetup /></section>

      {/* Storage & Upload Settings */}
      <StorageSettingsCard />

      <SystemInfoCard />
    </div>
  )
}
import { queryKeys } from '../api/queryKeys'
import { QueryFeedback } from '../components/QueryFeedback'
