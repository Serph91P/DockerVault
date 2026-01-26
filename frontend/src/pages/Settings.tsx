import { useQuery, useMutation } from '@tanstack/react-query'
import { Settings as SettingsIcon, Link2, Check, X, RefreshCw } from 'lucide-react'
import { komodoApi, dockerApi } from '../api'
import toast from 'react-hot-toast'

export default function Settings() {
  const { data: dockerHealth, refetch: refetchDocker } = useQuery({
    queryKey: ['docker-health'],
    queryFn: () => dockerApi.getHealth().then(() => true).catch(() => false),
    refetchInterval: 30000,
  })

  const { data: komodoStatus, refetch: refetchKomodo } = useQuery({
    queryKey: ['komodo-status'],
    queryFn: () => komodoApi.getStatus().then((r) => r.data),
  })

  const testKomodoMutation = useMutation({
    mutationFn: () => komodoApi.test(),
    onSuccess: () => toast.success('Komodo connection successful'),
    onError: () => toast.error('Komodo connection failed'),
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

      {/* Komodo Integration */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 bg-purple-500/10 rounded-lg flex items-center justify-center">
              <Link2 className="w-5 h-5 text-purple-400" />
            </div>
            <div>
              <h2 className="text-lg font-semibold text-dark-100">Komodo Integration</h2>
              <p className="text-sm text-dark-400">
                Connection to Komodo for container orchestration
              </p>
            </div>
          </div>
          <button
            onClick={() => refetchKomodo()}
            className="p-2 text-dark-400 hover:bg-dark-700 rounded-lg"
          >
            <RefreshCw className="w-4 h-4" />
          </button>
        </div>

        <div className="space-y-4">
          <div className="flex items-center gap-2">
            <span className="text-dark-400">Status:</span>
            {komodoStatus?.enabled ? (
              komodoStatus?.connected ? (
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

          {komodoStatus?.api_url && (
            <div className="text-sm">
              <span className="text-dark-400">API URL: </span>
              <span className="text-dark-200 font-mono">{komodoStatus.api_url}</span>
            </div>
          )}

          {komodoStatus?.enabled && (
            <button
              onClick={() => testKomodoMutation.mutate()}
              disabled={testKomodoMutation.isPending}
              className="px-4 py-2 bg-purple-500/10 text-purple-400 rounded-lg hover:bg-purple-500/20 transition-colors text-sm"
            >
              Test Connection
            </button>
          )}

          {!komodoStatus?.enabled && (
            <div className="p-4 bg-dark-700 rounded-lg">
              <p className="text-sm text-dark-300">
                To enable Komodo, set the following environment variables:
              </p>
              <pre className="mt-2 text-xs text-dark-400 font-mono">
{`KOMODO_ENABLED=true
KOMODO_API_URL=http://komodo:8080
KOMODO_API_KEY=your-api-key`}
              </pre>
            </div>
          )}
        </div>
      </div>

      {/* Backup Settings */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
        <div className="flex items-center gap-3 mb-4">
          <div className="w-10 h-10 bg-green-500/10 rounded-lg flex items-center justify-center">
            <SettingsIcon className="w-5 h-5 text-green-400" />
          </div>
          <div>
            <h2 className="text-lg font-semibold text-dark-100">Backup Settings</h2>
            <p className="text-sm text-dark-400">General backup configuration</p>
          </div>
        </div>

        <div className="space-y-4 text-sm">
          <div className="flex justify-between py-2 border-b border-dark-700">
            <span className="text-dark-400">Backup Directory</span>
            <span className="text-dark-200 font-mono">/backups</span>
          </div>
          <div className="flex justify-between py-2 border-b border-dark-700">
            <span className="text-dark-400">Default Retention (Days)</span>
            <span className="text-dark-200">30</span>
          </div>
          <div className="flex justify-between py-2 border-b border-dark-700">
            <span className="text-dark-400">Default Retention (Count)</span>
            <span className="text-dark-200">10</span>
          </div>
          <div className="flex justify-between py-2 border-b border-dark-700">
            <span className="text-dark-400">Max Parallel Backups</span>
            <span className="text-dark-200">2</span>
          </div>
          <div className="flex justify-between py-2 border-b border-dark-700">
            <span className="text-dark-400">Compression Level</span>
            <span className="text-dark-200">6 (1-9)</span>
          </div>
          <div className="flex justify-between py-2">
            <span className="text-dark-400">Timezone</span>
            <span className="text-dark-200">Europe/Berlin</span>
          </div>
        </div>

        <div className="mt-4 p-4 bg-dark-700 rounded-lg">
          <p className="text-sm text-dark-300">
            These settings can be customized via environment variables.
            See the .env.example file for all available options.
          </p>
        </div>
      </div>
    </div>
  )
}
