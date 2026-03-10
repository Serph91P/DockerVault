import { useState, useMemo } from 'react'
import { Plus, Trash2, Database, Filter, FolderOpen, ChevronDown, ChevronRight, Settings2 } from 'lucide-react'
import { WizardData } from './index'
import { Container as ContainerType, Stack } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: ContainerType[]
  stacks: Stack[]
}

// Reusable path editor for both global and per-volume rules
function PathEditor({
  label,
  icon,
  colorClass,
  paths,
  onAdd,
  onRemove,
  placeholder,
  description,
}: {
  label: string
  icon: React.ReactNode
  colorClass: string
  paths: string[]
  onAdd: (path: string) => void
  onRemove: (path: string) => void
  placeholder: string
  description: string
}) {
  const [newPath, setNewPath] = useState('')

  const handleAdd = () => {
    if (newPath.trim() && !paths.includes(newPath.trim())) {
      onAdd(newPath.trim())
      setNewPath('')
    }
  }

  return (
    <div className="space-y-2">
      <h5 className="text-sm font-medium text-dark-300 flex items-center gap-1.5">
        {icon}
        {label}
      </h5>
      <p className="text-xs text-dark-500">{description}</p>
      <div className="flex gap-2">
        <input
          type="text"
          value={newPath}
          onChange={(e) => setNewPath(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
          placeholder={placeholder}
          className="flex-1 px-3 py-1.5 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 text-sm placeholder-dark-500"
        />
        <button
          onClick={handleAdd}
          disabled={!newPath.trim()}
          className={`px-2.5 py-1.5 ${colorClass} rounded-lg disabled:opacity-50 transition-colors`}
        >
          <Plus className="w-3.5 h-3.5" />
        </button>
      </div>
      {paths.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {paths.map((path) => (
            <span
              key={path}
              className={`flex items-center gap-1 px-2 py-0.5 ${colorClass.replace('hover:', '')} rounded text-xs`}
            >
              {path}
              <button onClick={() => onRemove(path)} className="opacity-70 hover:opacity-100">
                <Trash2 className="w-3 h-3" />
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  )
}

// Per-volume rule accordion
function VolumeRuleAccordion({
  volumeName,
  usedBy,
  rules,
  onUpdate,
  onClear,
}: {
  volumeName: string
  usedBy?: string[]
  rules: { includePaths: string[]; excludePaths: string[] } | undefined
  onUpdate: (rules: { includePaths: string[]; excludePaths: string[] }) => void
  onClear: () => void
}) {
  const [expanded, setExpanded] = useState(false)
  const hasRules = rules && (rules.includePaths.length > 0 || rules.excludePaths.length > 0)
  const currentRules = rules || { includePaths: [], excludePaths: [] }

  return (
    <div className={`border rounded-lg transition-colors ${hasRules ? 'border-purple-500/40 bg-purple-500/5' : 'border-dark-700 bg-dark-800'}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center justify-between p-3 text-left"
      >
        <div className="flex items-center gap-2 min-w-0">
          {expanded ? (
            <ChevronDown className="w-4 h-4 text-dark-400 flex-shrink-0" />
          ) : (
            <ChevronRight className="w-4 h-4 text-dark-400 flex-shrink-0" />
          )}
          <Database className="w-3.5 h-3.5 text-dark-400 flex-shrink-0" />
          <span className="text-sm text-dark-200 truncate">{volumeName}</span>
          {usedBy && usedBy.length > 0 && (
            <span className="text-[11px] text-dark-500 truncate flex-shrink-0">
              → {usedBy.join(', ')}
            </span>
          )}
        </div>
        {hasRules ? (
          <span className="px-2 py-0.5 bg-purple-500/20 text-purple-400 rounded text-xs">
            {(currentRules.includePaths.length || 0) + (currentRules.excludePaths.length || 0)} rule(s)
          </span>
        ) : (
          <span className="text-xs text-dark-500">Uses default rules</span>
        )}
      </button>

      {expanded && (
        <div className="px-3 pb-3 space-y-3 border-t border-dark-700 pt-3 ml-6">
          <PathEditor
            label="Include Paths"
            icon={<FolderOpen className="w-3.5 h-3.5 text-green-400" />}
            colorClass="bg-green-500/10 text-green-400"
            paths={currentRules.includePaths}
            onAdd={(path) => onUpdate({ ...currentRules, includePaths: [...currentRules.includePaths, path] })}
            onRemove={(path) => onUpdate({ ...currentRules, includePaths: currentRules.includePaths.filter((p) => p !== path) })}
            placeholder="e.g., config/ or data/*.db"
            description="Only backup these paths in this volume"
          />
          <PathEditor
            label="Exclude Paths"
            icon={<Filter className="w-3.5 h-3.5 text-orange-400" />}
            colorClass="bg-orange-500/10 text-orange-400"
            paths={currentRules.excludePaths}
            onAdd={(path) => onUpdate({ ...currentRules, excludePaths: [...currentRules.excludePaths, path] })}
            onRemove={(path) => onUpdate({ ...currentRules, excludePaths: currentRules.excludePaths.filter((p) => p !== path) })}
            placeholder="e.g., cache/ or *.log"
            description="Exclude these paths from this volume"
          />
          {hasRules && (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onClear()
              }}
              className="text-xs text-dark-500 hover:text-dark-300 transition-colors"
            >
              Clear rules (use defaults)
            </button>
          )}
        </div>
      )}
    </div>
  )
}

export default function StepVolumeConfig({
  data,
  updateData,
  containers,
  stacks,
}: Props) {
  const [defaultRulesExpanded, setDefaultRulesExpanded] = useState(false)
  const isStack = data.targetType === 'stack'

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

  // Determine which volumes are "active" (selected for backup)
  const activeVolumes = data.selectedVolumes.length === 0
    ? availableVolumes
    : data.selectedVolumes.filter((v) => availableVolumes.includes(v))

  const showPerVolumeRules = showVolumeSelection && activeVolumes.length > 1

  // V3: Map volumes to their container names
  const volumeContainerMap = useMemo(() => {
    const map: Record<string, string[]> = {}
    const relevantContainers =
      data.targetType === 'stack' && data.stackName
        ? stacks.find((s) => s.name === data.stackName)?.containers ?? []
        : data.targetType === 'container' && data.containerName
          ? containers.filter((c) => c.name === data.containerName)
          : []
    for (const container of relevantContainers) {
      for (const mount of container.mounts) {
        if (mount.type === 'volume' && mount.name) {
          if (!map[mount.name]) map[mount.name] = []
          const shortName = container.name.replace(/^\//, '')
          if (!map[mount.name].includes(shortName)) {
            map[mount.name].push(shortName)
          }
        }
      }
    }
    return map
  }, [data.targetType, data.stackName, data.containerName, stacks, containers])

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

  const updateVolumeRule = (volumeName: string, rules: { includePaths: string[]; excludePaths: string[] }) => {
    updateData({
      perVolumeRules: { ...data.perVolumeRules, [volumeName]: rules },
    })
  }

  const clearVolumeRule = (volumeName: string) => {
    const newRules = { ...data.perVolumeRules }
    delete newRules[volumeName]
    updateData({ perVolumeRules: newRules })
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
          <h4 className="font-medium text-dark-200 flex items-center gap-2">
            <Database className="w-4 h-4" />
            Volume Selection
          </h4>

          {/* V2: Radio-style selection mode */}
          <div className="grid grid-cols-2 gap-3">
            <button
              onClick={handleSelectAll}
              className={`flex items-center gap-3 p-3 rounded-lg border transition-colors text-left ${
                data.selectedVolumes.length === 0
                  ? 'border-primary-500 bg-primary-500/10 text-primary-400'
                  : 'border-dark-700 bg-dark-800 text-dark-400 hover:border-dark-600'
              }`}
            >
              <div className={`w-4 h-4 rounded-full border-2 flex items-center justify-center ${
                data.selectedVolumes.length === 0 ? 'border-primary-500' : 'border-dark-600'
              }`}>
                {data.selectedVolumes.length === 0 && (
                  <div className="w-2 h-2 rounded-full bg-primary-500" />
                )}
              </div>
              <div>
                <span className="text-sm font-medium">Backup all volumes</span>
                <p className="text-xs text-dark-500">{availableVolumes.length} volume{availableVolumes.length !== 1 ? 's' : ''}</p>
              </div>
            </button>
            <button
              onClick={() => {
                if (data.selectedVolumes.length === 0) {
                  // Switch to specific mode – pre-select all
                  updateData({ selectedVolumes: [...availableVolumes] })
                }
              }}
              className={`flex items-center gap-3 p-3 rounded-lg border transition-colors text-left ${
                data.selectedVolumes.length > 0
                  ? 'border-primary-500 bg-primary-500/10 text-primary-400'
                  : 'border-dark-700 bg-dark-800 text-dark-400 hover:border-dark-600'
              }`}
            >
              <div className={`w-4 h-4 rounded-full border-2 flex items-center justify-center ${
                data.selectedVolumes.length > 0 ? 'border-primary-500' : 'border-dark-600'
              }`}>
                {data.selectedVolumes.length > 0 && (
                  <div className="w-2 h-2 rounded-full bg-primary-500" />
                )}
              </div>
              <div>
                <span className="text-sm font-medium">Select specific volumes</span>
                <p className="text-xs text-dark-500">Choose which to backup</p>
              </div>
            </button>
          </div>

          {/* Volume checkboxes – only when "specific" mode */}
          {data.selectedVolumes.length > 0 && (
            <div className="grid grid-cols-2 gap-2">
              {availableVolumes.map((volume) => {
                const isSelected = data.selectedVolumes.includes(volume)
                const usedBy = volumeContainerMap[volume]
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
                      className="w-4 h-4 rounded border-dark-600 bg-dark-700 text-primary-500 flex-shrink-0"
                    />
                    <div className="min-w-0">
                      <span className="text-sm truncate block">{volume}</span>
                      {usedBy && usedBy.length > 0 && (
                        <span className="text-[11px] text-dark-500 truncate block">
                          → {usedBy.join(', ')}
                        </span>
                      )}
                    </div>
                  </button>
                )
              })}
            </div>
          )}

          {data.selectedVolumes.length > 0 && (
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

      {/* Stack hint */}
      {isStack && showPerVolumeRules && (
        <div className="bg-blue-500/10 border border-blue-500/30 rounded-lg p-3 text-sm text-blue-300">
          <span className="font-medium">Tip:</span> For stacks, configure rules per volume for best results — each volume typically contains different data.
        </div>
      )}

      {/* Per-Volume Rules – primary for stacks, shown first */}
      {showPerVolumeRules && isStack && (
        <div className="space-y-3">
          <h4 className="font-medium text-dark-200 flex items-center gap-2">
            <Database className="w-4 h-4 text-purple-400" />
            Per-Volume Rules
            <span className="text-xs text-dark-500 font-normal">(optional, per-volume overrides)</span>
          </h4>
          <p className="text-xs text-dark-500">
            Set specific include/exclude rules for individual volumes.
          </p>

          <div className="space-y-2">
            {activeVolumes.map((volume) => (
              <VolumeRuleAccordion
                key={volume}
                volumeName={volume}
                usedBy={volumeContainerMap[volume]}
                rules={data.perVolumeRules[volume]}
                onUpdate={(rules) => updateVolumeRule(volume, rules)}
                onClear={() => clearVolumeRule(volume)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Default Path Rules (global fallback) — collapsed by default for stacks */}
      <div className="space-y-3">
        {isStack && showPerVolumeRules ? (
          <>
            <button
              onClick={() => setDefaultRulesExpanded(!defaultRulesExpanded)}
              className="flex items-center gap-2 w-full text-left"
            >
              {defaultRulesExpanded ? (
                <ChevronDown className="w-4 h-4 text-dark-400" />
              ) : (
                <ChevronRight className="w-4 h-4 text-dark-400" />
              )}
              <h4 className="font-medium text-dark-300 flex items-center gap-2">
                <Settings2 className="w-4 h-4" />
                Default Path Rules
                <span className="text-xs text-dark-500 font-normal">(fallback for volumes without own rules)</span>
              </h4>
            </button>
            {defaultRulesExpanded && (
              <div className="ml-6 space-y-3">
                <PathEditor
                  label="Include Paths"
                  icon={<FolderOpen className="w-3.5 h-3.5 text-green-400" />}
                  colorClass="bg-green-500/10 text-green-400"
                  paths={data.includePaths}
                  onAdd={(path) => updateData({ includePaths: [...data.includePaths, path] })}
                  onRemove={(path) => updateData({ includePaths: data.includePaths.filter((p) => p !== path) })}
                  placeholder="e.g., config/ or data/*.db"
                  description="Only backup these paths within volumes. Leave empty to backup everything."
                />
                <PathEditor
                  label="Exclude Paths"
                  icon={<Filter className="w-3.5 h-3.5 text-orange-400" />}
                  colorClass="bg-orange-500/10 text-orange-400"
                  paths={data.excludePaths}
                  onAdd={(path) => updateData({ excludePaths: [...data.excludePaths, path] })}
                  onRemove={(path) => updateData({ excludePaths: data.excludePaths.filter((p) => p !== path) })}
                  placeholder="e.g., cache/ or *.log or metadata/*"
                  description="Exclude these paths/patterns from the backup. Supports wildcards."
                />
              </div>
            )}
          </>
        ) : (
          <>
            <h4 className="font-medium text-dark-200 flex items-center gap-2">
              <Settings2 className="w-4 h-4" />
              {showPerVolumeRules ? 'Default Path Rules' : 'Path Rules'}
              <span className="text-xs text-dark-500 font-normal">(optional)</span>
            </h4>
            {showPerVolumeRules && (
              <p className="text-xs text-dark-500">
                These rules apply to volumes without their own specific rules.
              </p>
            )}
            <PathEditor
              label="Include Paths"
              icon={<FolderOpen className="w-3.5 h-3.5 text-green-400" />}
              colorClass="bg-green-500/10 text-green-400"
              paths={data.includePaths}
              onAdd={(path) => updateData({ includePaths: [...data.includePaths, path] })}
              onRemove={(path) => updateData({ includePaths: data.includePaths.filter((p) => p !== path) })}
              placeholder="e.g., config/ or data/*.db"
              description="Only backup these paths within volumes. Leave empty to backup everything."
            />
            <PathEditor
              label="Exclude Paths"
              icon={<Filter className="w-3.5 h-3.5 text-orange-400" />}
              colorClass="bg-orange-500/10 text-orange-400"
              paths={data.excludePaths}
              onAdd={(path) => updateData({ excludePaths: [...data.excludePaths, path] })}
              onRemove={(path) => updateData({ excludePaths: data.excludePaths.filter((p) => p !== path) })}
              placeholder="e.g., cache/ or *.log or metadata/*"
              description="Exclude these paths/patterns from the backup. Supports wildcards."
            />
          </>
        )}
      </div>

      {/* Per-Volume Rules – for non-stacks, keep original position */}
      {showPerVolumeRules && !isStack && (
        <div className="space-y-3">
          <h4 className="font-medium text-dark-200 flex items-center gap-2">
            <Database className="w-4 h-4 text-purple-400" />
            Per-Volume Rules
            <span className="text-xs text-dark-500 font-normal">(optional, overrides defaults)</span>
          </h4>
          <p className="text-xs text-dark-500">
            Set specific include/exclude rules for individual volumes. Volumes without rules use the defaults above.
          </p>

          <div className="space-y-2">
            {activeVolumes.map((volume) => (
              <VolumeRuleAccordion
                key={volume}
                volumeName={volume}
                usedBy={volumeContainerMap[volume]}
                rules={data.perVolumeRules[volume]}
                onUpdate={(rules) => updateVolumeRule(volume, rules)}
                onClear={() => clearVolumeRule(volume)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Examples */}
      <div className="bg-dark-750 rounded-lg p-4 text-sm">
        <h5 className="font-medium text-dark-200 mb-2">Path Pattern Examples:</h5>
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
