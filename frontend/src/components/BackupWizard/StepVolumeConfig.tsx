import { useState } from 'react'
import { Plus, Trash2, Database, Filter, FolderOpen } from 'lucide-react'
import { WizardData } from './index'
import { Container as ContainerType, Stack } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: ContainerType[]
  stacks: Stack[]
}

export default function StepVolumeConfig({
  data,
  updateData,
  containers,
  stacks,
}: Props) {
  const [newIncludePath, setNewIncludePath] = useState('')
  const [newExcludePath, setNewExcludePath] = useState('')

  // Get available volumes based on target type
  const getAvailableVolumes = (): string[] => {
    if (data.targetType === 'container' && data.containerName) {
      const container = containers.find((c) => c.name === data.containerName)
      if (container) {
        return container.mounts
          .filter((m) => m.type === 'volume')
          .map((m) => m.name || '')
          .filter(Boolean)
      }
    } else if (data.targetType === 'stack' && data.stackName) {
      const stack = stacks.find((s) => s.name === data.stackName)
      if (stack) {
        return stack.volumes || []
      }
    }
    return []
  }

  const availableVolumes = getAvailableVolumes()
  const showVolumeSelection = data.targetType === 'container' || data.targetType === 'stack'

  const handleVolumeToggle = (volumeName: string) => {
    const isSelected = data.selectedVolumes.includes(volumeName)
    if (isSelected) {
      updateData({
        selectedVolumes: data.selectedVolumes.filter((v) => v !== volumeName),
      })
    } else {
      updateData({
        selectedVolumes: [...data.selectedVolumes, volumeName],
      })
    }
  }

  const handleSelectAll = () => {
    updateData({ selectedVolumes: [] }) // Empty = all volumes
  }

  const handleSelectNone = () => {
    updateData({ selectedVolumes: availableVolumes })
  }

  const addIncludePath = () => {
    if (newIncludePath.trim() && !data.includePaths.includes(newIncludePath.trim())) {
      updateData({
        includePaths: [...data.includePaths, newIncludePath.trim()],
      })
      setNewIncludePath('')
    }
  }

  const removeIncludePath = (path: string) => {
    updateData({
      includePaths: data.includePaths.filter((p) => p !== path),
    })
  }

  const addExcludePath = () => {
    if (newExcludePath.trim() && !data.excludePaths.includes(newExcludePath.trim())) {
      updateData({
        excludePaths: [...data.excludePaths, newExcludePath.trim()],
      })
      setNewExcludePath('')
    }
  }

  const removeExcludePath = (path: string) => {
    updateData({
      excludePaths: data.excludePaths.filter((p) => p !== path),
    })
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Volume & Path Configuration</h3>
        <p className="text-sm text-dark-400">
          Select which volumes to backup and configure path filters
        </p>
      </div>

      {/* Volume Selection (only for container/stack) */}
      {showVolumeSelection && availableVolumes.length > 0 && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h4 className="font-medium text-dark-200 flex items-center gap-2">
              <Database className="w-4 h-4" />
              Select Volumes
            </h4>
            <div className="flex gap-2">
              <button
                onClick={handleSelectAll}
                className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
              >
                All
              </button>
              <button
                onClick={handleSelectNone}
                className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
              >
                None
              </button>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-2">
            {availableVolumes.map((volume) => {
              // If no volumes selected = all, otherwise check if this volume is selected
              const isSelected = data.selectedVolumes.length === 0 || data.selectedVolumes.includes(volume)
              return (
                <button
                  key={volume}
                  onClick={() => handleVolumeToggle(volume)}
                  className={`flex items-center gap-2 p-3 rounded-lg border transition-colors text-left ${
                    isSelected
                      ? 'border-primary-500 bg-primary-500/10 text-primary-400'
                      : 'border-dark-700 bg-dark-800 text-dark-400 hover:border-dark-600'
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onChange={() => {}}
                    className="w-4 h-4 rounded border-dark-600 bg-dark-700 text-primary-500"
                  />
                  <span className="text-sm truncate">{volume}</span>
                </button>
              )
            })}
          </div>

          {data.selectedVolumes.length === 0 ? (
            <p className="text-xs text-green-400">All volumes will be backed up</p>
          ) : (
            <p className="text-xs text-dark-400">
              {data.selectedVolumes.length} of {availableVolumes.length} volumes selected
            </p>
          )}
        </div>
      )}

      {showVolumeSelection && availableVolumes.length === 0 && (
        <div className="bg-yellow-500/10 border border-yellow-500/20 rounded-lg p-4 text-sm text-yellow-400">
          No volumes found for this {data.targetType}. The backup may not contain any data.
        </div>
      )}

      {/* Include Paths */}
      <div className="space-y-3">
        <h4 className="font-medium text-dark-200 flex items-center gap-2">
          <FolderOpen className="w-4 h-4 text-green-400" />
          Include Paths
          <span className="text-xs text-dark-500 font-normal">(optional)</span>
        </h4>
        <p className="text-xs text-dark-400">
          Only backup these paths within the volumes. Leave empty to backup everything.
        </p>

        <div className="flex gap-2">
          <input
            type="text"
            value={newIncludePath}
            onChange={(e) => setNewIncludePath(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addIncludePath()}
            placeholder="e.g., config/ or data/*.db"
            className="flex-1 px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 text-sm placeholder-dark-500"
          />
          <button
            onClick={addIncludePath}
            disabled={!newIncludePath.trim()}
            className="px-3 py-2 bg-green-500/20 hover:bg-green-500/30 text-green-400 rounded-lg disabled:opacity-50 transition-colors"
          >
            <Plus className="w-4 h-4" />
          </button>
        </div>

        {data.includePaths.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {data.includePaths.map((path) => (
              <span
                key={path}
                className="flex items-center gap-1 px-2 py-1 bg-green-500/10 text-green-400 rounded text-sm"
              >
                {path}
                <button
                  onClick={() => removeIncludePath(path)}
                  className="hover:text-green-300"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Exclude Paths */}
      <div className="space-y-3">
        <h4 className="font-medium text-dark-200 flex items-center gap-2">
          <Filter className="w-4 h-4 text-orange-400" />
          Exclude Paths
          <span className="text-xs text-dark-500 font-normal">(optional)</span>
        </h4>
        <p className="text-xs text-dark-400">
          Exclude these paths/patterns from the backup. Supports wildcards like *.log or cache/*
        </p>

        <div className="flex gap-2">
          <input
            type="text"
            value={newExcludePath}
            onChange={(e) => setNewExcludePath(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addExcludePath()}
            placeholder="e.g., cache/ or *.log or metadata/*"
            className="flex-1 px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 text-sm placeholder-dark-500"
          />
          <button
            onClick={addExcludePath}
            disabled={!newExcludePath.trim()}
            className="px-3 py-2 bg-orange-500/20 hover:bg-orange-500/30 text-orange-400 rounded-lg disabled:opacity-50 transition-colors"
          >
            <Plus className="w-4 h-4" />
          </button>
        </div>

        {data.excludePaths.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {data.excludePaths.map((path) => (
              <span
                key={path}
                className="flex items-center gap-1 px-2 py-1 bg-orange-500/10 text-orange-400 rounded text-sm"
              >
                {path}
                <button
                  onClick={() => removeExcludePath(path)}
                  className="hover:text-orange-300"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Examples */}
      <div className="bg-dark-750 rounded-lg p-4 text-sm">
        <h5 className="font-medium text-dark-200 mb-2">Examples:</h5>
        <ul className="space-y-1 text-dark-400">
          <li>
            <code className="text-primary-400">config/</code> - Include/exclude the config folder
          </li>
          <li>
            <code className="text-primary-400">*.log</code> - Match all .log files
          </li>
          <li>
            <code className="text-primary-400">cache/*</code> - Match everything in cache folder
          </li>
          <li>
            <code className="text-primary-400">metadata/</code> - Exclude metadata folder
          </li>
        </ul>
      </div>
    </div>
  )
}
