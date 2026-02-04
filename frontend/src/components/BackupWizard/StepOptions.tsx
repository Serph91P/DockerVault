import { useState } from 'react'
import { Settings, Zap, Terminal, ChevronDown, ChevronUp } from 'lucide-react'
import { WizardData } from './index'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
}

// Compression options
const COMPRESSION_OPTIONS = [
  { value: 'none', label: 'None', description: 'No compression, fastest but largest files' },
  { value: 'gzip', label: 'Gzip', description: 'Good balance of speed and compression' },
  { value: 'zstd', label: 'Zstandard', description: 'Best compression ratio, slightly slower' },
]

export default function StepOptions({ data, updateData }: Props) {
  const [showAdvanced, setShowAdvanced] = useState(false)

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Backup Options</h3>
        <p className="text-sm text-dark-400">
          Configure compression, encryption, and other advanced options
        </p>
      </div>

      {/* Compression */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
        <div className="flex items-center gap-2 mb-4">
          <Zap className="w-4 h-4 text-primary-400" />
          <h4 className="text-dark-100 font-medium">Compression</h4>
        </div>

        <div className="space-y-2">
          {COMPRESSION_OPTIONS.map((option) => (
            <button
              key={option.value}
              onClick={() => updateData({ compression: option.value as WizardData['compression'] })}
              className={`w-full flex items-center gap-3 p-3 rounded-lg border transition-all text-left ${
                data.compression === option.value
                  ? 'border-primary-500 bg-primary-500/10'
                  : 'border-dark-600 hover:border-dark-500 bg-dark-900'
              }`}
            >
              <div
                className={`w-4 h-4 rounded-full border-2 flex items-center justify-center ${
                  data.compression === option.value
                    ? 'border-primary-500 bg-primary-500'
                    : 'border-dark-500'
                }`}
              >
                {data.compression === option.value && (
                  <div className="w-2 h-2 rounded-full bg-white" />
                )}
              </div>
              <div>
                <p className="text-dark-100 font-medium">{option.label}</p>
                <p className="text-sm text-dark-400">{option.description}</p>
              </div>
            </button>
          ))}
        </div>
      </div>

      {/* Enable Target */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
        <label className="flex items-center gap-3 cursor-pointer">
          <input
            type="checkbox"
            checked={data.enabled}
            onChange={(e) => updateData({ enabled: e.target.checked })}
            className="w-5 h-5 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
          />
          <div>
            <span className="text-dark-100 font-medium">Enable backup target</span>
            <p className="text-sm text-dark-400 mt-1">
              When disabled, scheduled backups won't run for this target
            </p>
          </div>
        </label>
      </div>

      {/* Advanced Options Toggle */}
      <button
        onClick={() => setShowAdvanced(!showAdvanced)}
        className="w-full flex items-center justify-between p-4 bg-dark-800 rounded-xl border border-dark-700 hover:border-dark-600 transition-colors"
      >
        <div className="flex items-center gap-2">
          <Settings className="w-4 h-4 text-dark-400" />
          <span className="text-dark-300">Advanced Options</span>
        </div>
        {showAdvanced ? (
          <ChevronUp className="w-4 h-4 text-dark-400" />
        ) : (
          <ChevronDown className="w-4 h-4 text-dark-400" />
        )}
      </button>

      {/* Advanced Options */}
      {showAdvanced && (
        <div className="space-y-4 pl-4 border-l-2 border-dark-700">
          {/* Pre-backup Command */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <Terminal className="w-4 h-4" />
              Pre-backup Command
            </label>
            <input
              type="text"
              value={data.preCommand}
              onChange={(e) => updateData({ preCommand: e.target.value })}
              placeholder="e.g., docker exec db pg_dump > /backup/dump.sql"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 font-mono text-sm focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
            <p className="text-xs text-dark-500 mt-1">
              Command to run before the backup starts
            </p>
          </div>

          {/* Post-backup Command */}
          <div>
            <label className="flex items-center gap-2 text-sm font-medium text-dark-300 mb-2">
              <Terminal className="w-4 h-4" />
              Post-backup Command
            </label>
            <input
              type="text"
              value={data.postCommand}
              onChange={(e) => updateData({ postCommand: e.target.value })}
              placeholder="e.g., rm /backup/dump.sql"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 font-mono text-sm focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
            <p className="text-xs text-dark-500 mt-1">
              Command to run after the backup completes
            </p>
          </div>

          {/* Custom Backup Path */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">
              Custom Backup Path
            </label>
            <input
              type="text"
              value={data.customPath}
              onChange={(e) => updateData({ customPath: e.target.value })}
              placeholder="Leave empty for default path"
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
            <p className="text-xs text-dark-500 mt-1">
              Override the default backup storage path
            </p>
          </div>

          {/* Exclude Patterns */}
          <div>
            <label className="block text-sm font-medium text-dark-300 mb-2">
              Exclude Patterns
            </label>
            <textarea
              value={data.excludePatterns}
              onChange={(e) => updateData({ excludePatterns: e.target.value })}
              placeholder="*.tmp&#10;*.log&#10;node_modules/"
              rows={3}
              className="w-full px-4 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 font-mono text-sm focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
            />
            <p className="text-xs text-dark-500 mt-1">
              One pattern per line. Files matching these patterns will be excluded.
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
