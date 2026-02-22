import { useState, useMemo, useEffect, useRef } from 'react'
import { AlertCircle, Link2, Power, Search, X, Layers, ChevronDown, ChevronRight, ArrowUp, ArrowDown, GripVertical } from 'lucide-react'
import { WizardData } from './index'
import { Container, Stack } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: Container[]
  stacks: Stack[]
}

export default function StepDependencies({ data, updateData, containers, stacks }: Props) {
  const [search, setSearch] = useState('')
  const [otherExpanded, setOtherExpanded] = useState(false)
  const isStack = data.targetType === 'stack'

  // Get available containers for dependencies
  const availableContainers = containers.filter((c) => {
    // Don't show the selected container as a dependency
    if (data.targetType === 'container' && c.name === data.containerName) {
      return false
    }
    return true
  })

  // Get stack containers for auto-detection
  const stackContainerInfo =
    isStack
      ? stacks.find((s) => s.name === data.stackName)?.containers || []
      : []
  const stackContainerNames = new Set(stackContainerInfo.map((c) => c.name))

  // Split containers into stack-own and external
  const stackOwnContainers = isStack
    ? availableContainers.filter((c) => stackContainerNames.has(c.name))
    : []
  const externalContainers = isStack
    ? availableContainers.filter((c) => !stackContainerNames.has(c.name))
    : availableContainers

  // Filter containers by search query
  const filteredExternalContainers = useMemo(() => {
    if (!search.trim()) return externalContainers
    const q = search.toLowerCase()
    return externalContainers.filter(
      (c) => c.name.toLowerCase().includes(q) || c.image.toLowerCase().includes(q)
    )
  }, [externalContainers, search])

  // Group filtered external containers by compose project
  const groupedContainers = useMemo(() => {
    const groups: Record<string, typeof filteredExternalContainers> = {}
    filteredExternalContainers.forEach((c) => {
      const project = c.compose_project || 'Other containers'
      if (!groups[project]) groups[project] = []
      groups[project].push(c)
    })
    const sortedKeys = Object.keys(groups).sort((a, b) => {
      if (a === 'Other containers') return 1
      if (b === 'Other containers') return -1
      return a.localeCompare(b)
    })
    return sortedKeys.map((key) => ({ name: key, containers: groups[key] }))
  }, [filteredExternalContainers])

  const toggleDependency = (containerName: string) => {
    const newDeps = data.dependencies.includes(containerName)
      ? data.dependencies.filter((d) => d !== containerName)
      : [...data.dependencies, containerName]
    updateData({ dependencies: newDeps })
  }

  // Auto-detect dependencies from stack
  const autoDetectDependencies = () => {
    if (isStack && stackContainerInfo.length > 0) {
      const detected: string[] = []
      stackContainerInfo.forEach((container) => {
        if (container.depends_on && container.depends_on.length > 0) {
          container.depends_on.forEach((dep) => {
            if (!detected.includes(dep)) {
              detected.push(dep)
            }
          })
        }
      })
      updateData({ dependencies: detected })
    }
  }

  // D2: Auto-run detection for stacks when no dependencies are selected yet
  const hasAutoDetected = useRef(false)
  useEffect(() => {
    if (hasAutoDetected.current || data.dependencies.length > 0) return
    if (isStack && stackContainerInfo.length > 0) {
      autoDetectDependencies()
      hasAutoDetected.current = true
    }
  }, [isStack, stackContainerInfo.length])

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Container Dependencies</h3>
        <p className="text-sm text-dark-400">
          Specify containers that should be stopped before backup and started after
        </p>
      </div>

      {/* Stop Container Option */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
        <label className="flex items-center gap-3 cursor-pointer">
          <input
            type="checkbox"
            checked={data.stopContainer}
            onChange={(e) => updateData({ stopContainer: e.target.checked })}
            className="w-5 h-5 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
          />
          <div className="flex items-center gap-2">
            <Power className="w-4 h-4 text-orange-400" />
            <div>
              <span className="text-dark-100 font-medium">Stop container(s) during backup</span>
              <p className="text-sm text-dark-400 mt-1">
                Ensures data consistency by stopping services before backup
              </p>
            </div>
          </div>
        </label>
      </div>

      {/* Auto-detect for stacks */}
      {isStack && stackContainerInfo.length > 0 && (
        <div className="bg-blue-500/10 border border-blue-500/30 rounded-xl p-4">
          <div className="flex items-start gap-3">
            <Link2 className="w-5 h-5 text-blue-400 mt-0.5" />
            <div className="flex-1">
              <h4 className="text-blue-400 font-medium">Stack Dependencies {data.dependencies.length > 0 ? 'Applied' : 'Detected'}</h4>
              <p className="text-sm text-dark-300 mt-1">
                {data.dependencies.length > 0
                  ? `${data.dependencies.length} dependencies detected from docker-compose configuration. You can adjust the selection below.`
                  : `We found ${stackContainerInfo.length} containers in this stack. Dependencies can be auto-detected from docker-compose configuration.`}
              </p>
              <button
                onClick={autoDetectDependencies}
                className="mt-3 px-4 py-2 bg-blue-500/20 text-blue-400 rounded-lg hover:bg-blue-500/30 transition-colors text-sm"
              >
                Re-detect Dependencies
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Stack-own containers — shown first for stacks */}
      {isStack && stackOwnContainers.length > 0 && (
        <div>
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-sm font-medium text-dark-300 flex items-center gap-2">
              <Layers className="w-4 h-4 text-primary-400" />
              Stack Containers
              <span className="text-xs text-dark-500 font-normal">
                ({stackOwnContainers.length})
              </span>
            </h4>
            <div className="flex gap-2">
              <button
                onClick={() => updateData({ dependencies: [...new Set([...data.dependencies, ...stackOwnContainers.map(c => c.name)])] })}
                className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
              >
                Select all
              </button>
              <button
                onClick={() => updateData({ dependencies: data.dependencies.filter(d => !stackContainerNames.has(d)) })}
                className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
              >
                Deselect all
              </button>
            </div>
          </div>
          <p className="text-xs text-dark-500 mb-2">These containers belong to this stack and will be managed during backup.</p>
          <div className="space-y-1.5">
            {stackOwnContainers.map((container) => {
              const isSelected = data.dependencies.includes(container.name)
              return (
                <button
                  key={container.id}
                  onClick={() => toggleDependency(container.name)}
                  className={`w-full flex items-center gap-3 p-3 rounded-lg border transition-all text-left ${
                    isSelected
                      ? 'border-primary-500 bg-primary-500/10'
                      : 'border-dark-700 hover:border-dark-600 bg-dark-800'
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onChange={() => {}}
                    className="w-4 h-4 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
                  />
                  <div className="flex-1 min-w-0">
                    <p className="text-dark-100 font-medium truncate">{container.name}</p>
                    <p className="text-sm text-dark-400 truncate">{container.image}</p>
                  </div>
                  <span
                    className={`px-2 py-1 rounded text-xs ${
                      container.state === 'running'
                        ? 'bg-green-500/10 text-green-400'
                        : 'bg-dark-600 text-dark-400'
                    }`}
                  >
                    {container.state}
                  </span>
                </button>
              )
            })}
          </div>
        </div>
      )}

      {/* Other containers — collapsible for stacks */}
      <div>
        {isStack ? (
          <button
            onClick={() => setOtherExpanded(!otherExpanded)}
            className="flex items-center gap-2 mb-3 w-full text-left"
          >
            {otherExpanded ? (
              <ChevronDown className="w-4 h-4 text-dark-400" />
            ) : (
              <ChevronRight className="w-4 h-4 text-dark-400" />
            )}
            <h4 className="text-sm font-medium text-dark-300">
              Other Containers
              <span className="text-xs text-dark-500 font-normal ml-2">
                ({externalContainers.length} — external dependencies)
              </span>
            </h4>
          </button>
        ) : (
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-sm font-medium text-dark-300">
              Select containers to manage during backup:
            </h4>
            {availableContainers.length > 0 && (
              <div className="flex gap-2">
                <button
                  onClick={() => updateData({ dependencies: availableContainers.map(c => c.name) })}
                  className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
                >
                  All
                </button>
                <button
                  onClick={() => updateData({ dependencies: [] })}
                  className="text-xs px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 rounded"
                >
                  None
                </button>
              </div>
            )}
          </div>
        )}

        {(!isStack || otherExpanded) && (
          <>
            {(isStack ? externalContainers : availableContainers).length === 0 ? (
              <div className="bg-dark-800 rounded-xl border border-dark-700 p-6 text-center">
                <AlertCircle className="w-8 h-8 text-dark-500 mx-auto mb-2" />
                <p className="text-dark-400">No other containers available</p>
              </div>
            ) : (
              <>
                {/* Search */}
                <div className="relative mb-3">
                  <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-500" />
                  <input
                    type="text"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search containers..."
                    className="w-full pl-9 pr-8 py-2 bg-dark-800 border border-dark-700 rounded-lg text-dark-100 text-sm placeholder:text-dark-500 focus:border-primary-500 focus:ring-1 focus:ring-primary-500"
                  />
                  {search && (
                    <button
                      onClick={() => setSearch('')}
                      className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-dark-500 hover:text-dark-300"
                    >
                      <X className="w-3.5 h-3.5" />
                    </button>
                  )}
                </div>

                {/* Count */}
                <p className="text-xs text-dark-500 mb-2">
                  {data.dependencies.filter(d => isStack ? !stackContainerNames.has(d) : true).length} selected of {(isStack ? externalContainers : availableContainers).length} containers
                  {search && ` (${filteredExternalContainers.length} matching)`}
                </p>

                {/* Grouped container list */}
                <div className="space-y-3 max-h-80 overflow-y-auto">
                  {groupedContainers.map((group) => (
                <div key={group.name}>
                  <div className="flex items-center gap-2 px-2 py-1.5 bg-dark-750 rounded-lg mb-1.5">
                    <Layers className="w-3.5 h-3.5 text-dark-400" />
                    <span className="text-xs font-medium text-dark-300 uppercase tracking-wider">
                      {group.name}
                    </span>
                    <span className="text-xs text-dark-500">
                      ({group.containers.length})
                    </span>
                  </div>
                  <div className="space-y-1.5 ml-2">
                    {group.containers.map((container) => {
                      const isSelected = data.dependencies.includes(container.name)
                      return (
                        <button
                          key={container.id}
                          onClick={() => toggleDependency(container.name)}
                          className={`w-full flex items-center gap-3 p-3 rounded-lg border transition-all text-left ${
                            isSelected
                              ? 'border-primary-500 bg-primary-500/10'
                              : 'border-dark-700 hover:border-dark-600 bg-dark-800'
                          }`}
                        >
                          <input
                            type="checkbox"
                            checked={isSelected}
                            onChange={() => {}}
                            className="w-4 h-4 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
                          />
                          <div className="flex-1 min-w-0">
                            <p className="text-dark-100 font-medium truncate">{container.name}</p>
                            <p className="text-sm text-dark-400 truncate">{container.image}</p>
                          </div>
                          <span
                            className={`px-2 py-1 rounded text-xs ${
                              container.state === 'running'
                                ? 'bg-green-500/10 text-green-400'
                                : 'bg-dark-600 text-dark-400'
                            }`}
                          >
                            {container.state}
                          </span>
                        </button>
                      )
                    })}
                  </div>
                </div>
              ))}
              {filteredExternalContainers.length === 0 && search && (
                <div className="text-center py-4">
                  <p className="text-sm text-dark-400">No containers matching &quot;{search}&quot;</p>
                </div>
              )}
            </div>
          </>
        )}
          </>
        )}
      </div>

      {/* Selected dependencies summary with reorder */}
      {data.dependencies.length > 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <h4 className="text-sm font-medium text-dark-300 mb-3">
            Stop/Start order ({data.dependencies.length} dependencies):
          </h4>
          <div className="space-y-1.5 mb-3">
            {data.dependencies.map((dep, index) => (
              <div
                key={dep}
                className="flex items-center gap-2 px-3 py-2 bg-dark-750 rounded-lg border border-dark-700"
              >
                <GripVertical className="w-3.5 h-3.5 text-dark-500 flex-shrink-0" />
                <span className="text-xs text-dark-500 w-5 text-center font-mono">{index + 1}</span>
                <span className="text-sm text-dark-100 flex-1 truncate">{dep}</span>
                <div className="flex gap-1 flex-shrink-0">
                  <button
                    type="button"
                    onClick={() => {
                      if (index === 0) return
                      const newDeps = [...data.dependencies]
                      ;[newDeps[index - 1], newDeps[index]] = [newDeps[index], newDeps[index - 1]]
                      updateData({ dependencies: newDeps })
                    }}
                    disabled={index === 0}
                    className="p-1 rounded hover:bg-dark-600 text-dark-400 hover:text-dark-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                    title="Move up"
                  >
                    <ArrowUp className="w-3.5 h-3.5" />
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      if (index === data.dependencies.length - 1) return
                      const newDeps = [...data.dependencies]
                      ;[newDeps[index], newDeps[index + 1]] = [newDeps[index + 1], newDeps[index]]
                      updateData({ dependencies: newDeps })
                    }}
                    disabled={index === data.dependencies.length - 1}
                    className="p-1 rounded hover:bg-dark-600 text-dark-400 hover:text-dark-200 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                    title="Move down"
                  >
                    <ArrowDown className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            ))}
          </div>
          <div className="text-xs text-dark-500 space-y-1 border-t border-dark-700 pt-2">
            <p>
              <span className="text-orange-400">Stop:</span>{' '}
              {data.dependencies.join(' \u2192 ')}
            </p>
            <p>
              <span className="text-green-400">Start:</span>{' '}
              {[...data.dependencies].reverse().join(' \u2192 ')}
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
