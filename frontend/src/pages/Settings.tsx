import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Link2, Check, X, RefreshCw, Save, Eye, EyeOff, TestTube, HardDrive, Shield, Clock, Database, Archive } from 'lucide-react'
import { dockerApi, settingsApi } from '../api'
import toast from 'react-hot-toast'
import EncryptionSetup from '../components/EncryptionSetup'
import { useState, useMemo } from 'react'

function KomodoSettingsCard() {
  const queryClient = useQueryClient()
  const [isEditing, setIsEditing] = useState(false)
  const [showApiKey, setShowApiKey] = useState(false)
  const { data: komodoSettings, isLoading, refetch } = useQuery({
    queryKey: ['komodo-settings'],
    queryFn: () => settingsApi.getKomodo().then((r) => r.data),
  })

  // Initialize form data from query result
  const initialFormData = useMemo(() => ({
    enabled: komodoSettings?.enabled ?? false,
    api_url: komodoSettings?.api_url || '',
    api_key: '',
  }), [komodoSettings])

  const [formData, setFormData] = useState({
    enabled: false,
    api_url: '',
    api_key: '',
  })

  // Reset form data when entering edit mode
  const handleStartEdit = () => {
    if (isLoading) return
    setFormData(initialFormData)
    setIsEditing(true)
  }

  const updateMutation = useMutation({
    mutationFn: (data: { enabled: boolean; api_url?: string; api_key?: string }) =>
      settingsApi.updateKomodo(data),
    onSuccess: () => {
      toast.success('Komodo settings saved')
      queryClient.invalidateQueries({ queryKey: ['komodo-settings'] })
      setIsEditing(false)
      setFormData((prev) => ({ ...prev, api_key: '' }))
    },
    onError: () => toast.error('Failed to save settings'),
  })

  const testMutation = useMutation({
    mutationFn: () => settingsApi.testKomodo(),
    onSuccess: (response) => {
      const result = response.data
      if (result.success) {
        toast.success(`Connection successful! Version: ${result.version || 'unknown'}`)
      } else {
        toast.error(result.message)
      }
      refetch()
    },
    onError: () => toast.error('Connection test failed'),
  })

  const handleSave = () => {
    const data: { enabled: boolean; api_url?: string; api_key?: string } = {
      enabled: formData.enabled,
    }
    if (formData.api_url) {
      data.api_url = formData.api_url
    }
    if (formData.api_key) {
      data.api_key = formData.api_key
    }
    updateMutation.mutate(data)
  }

  const handleCancel = () => {
    setFormData(initialFormData)
    setIsEditing(false)
  }

  if (isLoading) {
    return (
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse">
        <div className="h-6 bg-dark-700 rounded w-1/3 mb-4" />
        <div className="h-4 bg-dark-700 rounded w-2/3" />
      </div>
    )
  }

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-purple-500/10 rounded-lg flex items-center justify-center">
            <Link2 className="w-5 h-5 text-purple-400" />
          </div>
          <div>
            <h2 className="text-lg font-semibold text-dark-100">Komodo Integration</h2>
            <p className="text-sm text-dark-400">Container orchestration integration</p>
          </div>
        </div>
        <div className="flex gap-2">
          {!isEditing && (
            <button
              onClick={() => refetch()}
              className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
            >
              <RefreshCw className="w-4 h-4" />
            </button>
          )}
        </div>
      </div>

      {isEditing ? (
        <div className="space-y-4">
          {/* Enable/Disable */}
          <div className="flex items-center justify-between">
            <label className="text-sm text-dark-300">Enable Integration</label>
            <button
              onClick={() => setFormData((prev) => ({ ...prev, enabled: !prev.enabled }))}
              className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${
                formData.enabled ? 'bg-primary-500' : 'bg-dark-600'
              }`}
            >
              <span
                className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                  formData.enabled ? 'translate-x-6' : 'translate-x-1'
                }`}
              />
            </button>
          </div>

          {/* API URL */}
          <div>
            <label className="block text-sm text-dark-400 mb-1">API URL</label>
            <input
              type="text"
              value={formData.api_url}
              onChange={(e) => setFormData((prev) => ({ ...prev, api_url: e.target.value }))}
              placeholder="https://komodo.example.com"
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
            />
          </div>

          {/* API Key */}
          <div>
            <label className="block text-sm text-dark-400 mb-1">
              API Key {komodoSettings?.has_api_key && '(leave empty to keep current)'}
            </label>
            <div className="relative">
              <input
                type={showApiKey ? 'text' : 'password'}
                value={formData.api_key}
                onChange={(e) => setFormData((prev) => ({ ...prev, api_key: e.target.value }))}
                placeholder={komodoSettings?.has_api_key ? '••••••••' : 'Enter API key'}
                className="w-full px-3 py-2 pr-10 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100"
              />
              <button
                type="button"
                onClick={() => setShowApiKey(!showApiKey)}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-dark-400 hover:text-dark-200"
              >
                {showApiKey ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            </div>
          </div>

          {/* Actions */}
          <div className="flex gap-2 pt-2">
            <button
              onClick={handleSave}
              disabled={updateMutation.isPending}
              className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors text-sm"
            >
              <Save className="w-4 h-4" />
              Save
            </button>
            <button
              onClick={handleCancel}
              className="px-4 py-2 bg-dark-600 text-dark-300 rounded-lg hover:bg-dark-500 transition-colors text-sm"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <div className="space-y-4">
          {/* Status */}
          <div className="flex items-center gap-2">
            <span className="text-dark-400">Status:</span>
            {komodoSettings?.enabled ? (
              komodoSettings?.connected ? (
                <>
                  <Check className="w-4 h-4 text-green-500" />
                  <span className="text-green-400">Connected</span>
                </>
              ) : (
                <>
                  <X className="w-4 h-4 text-yellow-500" />
                  <span className="text-yellow-400">Enabled, not connected</span>
                </>
              )
            ) : (
              <>
                <X className="w-4 h-4 text-dark-500" />
                <span className="text-dark-400">Disabled</span>
              </>
            )}
          </div>

          {/* API URL */}
          {komodoSettings?.api_url && (
            <div className="text-sm">
              <span className="text-dark-400">API URL: </span>
              <span className="text-dark-200 font-mono">{komodoSettings.api_url}</span>
            </div>
          )}

          {/* API Key Status */}
          <div className="text-sm">
            <span className="text-dark-400">API Key: </span>
            {komodoSettings?.has_api_key ? (
              <span className="text-green-400">Configured</span>
            ) : (
              <span className="text-dark-500">Not set</span>
            )}
          </div>

          {/* Actions */}
          <div className="flex gap-2 pt-2">
            <button
              onClick={handleStartEdit}
              disabled={isLoading}
              className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg hover:bg-dark-600 transition-colors text-sm disabled:opacity-50"
            >
              Edit Settings
            </button>
            {komodoSettings?.enabled && (
              <button
                onClick={() => testMutation.mutate()}
                disabled={testMutation.isPending}
                className="flex items-center gap-2 px-4 py-2 bg-purple-500/10 text-purple-400 rounded-lg hover:bg-purple-500/20 transition-colors text-sm"
              >
                <TestTube className="w-4 h-4" />
                Test Connection
              </button>
            )}
          </div>
        </div>
      )}
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
  const { data: systemInfo } = useQuery({
    queryKey: ['system-info'],
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
            <a href="/retention" className="text-primary-400 hover:underline">
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
  const { data: dockerHealth, refetch: refetchDocker } = useQuery({
    queryKey: ['docker-health'],
    queryFn: () => dockerApi.getHealth().then(() => true).catch(() => false),
    refetchInterval: 30000,
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Settings</h1>
        <p className="text-dark-400 mt-1">Configuration and integrations</p>
      </div>

      {/* Docker Status */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
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
            className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
          >
            <RefreshCw className="w-4 h-4" />
          </button>
        </div>

        <div className="flex items-center gap-2">
          {dockerHealth ? (
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
      </div>

      {/* Encryption Settings */}
      <EncryptionSetup />

      {/* Komodo Integration */}
      <KomodoSettingsCard />

      {/* System Info */}
      <SystemInfoCard />
    </div>
  )
}
