import { AlertCircle, Link2, Power } from 'lucide-react'
import { WizardData } from './index'
import { Container, Stack } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: Container[]
  stacks: Stack[]
}

export default function StepDependencies({ data, updateData, containers, stacks }: Props) {
  // Get available containers for dependencies
  const availableContainers = containers.filter((c) => {
    // Don't show the selected container as a dependency
    if (data.targetType === 'container' && c.name === data.containerName) {
      return false
    }
    return true
  })

  // Get stack containers for auto-detection
  const stackContainers =
    data.targetType === 'stack'
      ? stacks.find((s) => s.name === data.stackName)?.containers || []
      : []

  const toggleDependency = (containerName: string) => {
    const newDeps = data.dependencies.includes(containerName)
      ? data.dependencies.filter((d) => d !== containerName)
      : [...data.dependencies, containerName]
    updateData({ dependencies: newDeps })
  }

  // Auto-detect dependencies from stack
  const autoDetectDependencies = () => {
    if (data.targetType === 'stack' && stackContainers.length > 0) {
      const detected: string[] = []
      stackContainers.forEach((container) => {
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
      {data.targetType === 'stack' && stackContainers.length > 0 && (
        <div className="bg-blue-500/10 border border-blue-500/30 rounded-xl p-4">
          <div className="flex items-start gap-3">
            <Link2 className="w-5 h-5 text-blue-400 mt-0.5" />
            <div className="flex-1">
              <h4 className="text-blue-400 font-medium">Stack Dependencies Detected</h4>
              <p className="text-sm text-dark-300 mt-1">
                We found {stackContainers.length} containers in this stack. Dependencies can be
                auto-detected from docker-compose configuration.
              </p>
              <button
                onClick={autoDetectDependencies}
                className="mt-3 px-4 py-2 bg-blue-500/20 text-blue-400 rounded-lg hover:bg-blue-500/30 transition-colors text-sm"
              >
                Auto-detect Dependencies
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Manual dependency selection */}
      <div>
        <h4 className="text-sm font-medium text-dark-300 mb-3">
          Select containers to manage during backup:
        </h4>

        {availableContainers.length === 0 ? (
          <div className="bg-dark-800 rounded-xl border border-dark-700 p-6 text-center">
            <AlertCircle className="w-8 h-8 text-dark-500 mx-auto mb-2" />
            <p className="text-dark-400">No other containers available</p>
          </div>
        ) : (
          <div className="space-y-2 max-h-64 overflow-y-auto">
            {availableContainers.map((container) => {
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
        )}
      </div>

      {/* Selected dependencies summary */}
      {data.dependencies.length > 0 && (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
          <h4 className="text-sm font-medium text-dark-300 mb-2">
            Backup order ({data.dependencies.length} dependencies):
          </h4>
          <div className="text-sm text-dark-400">
            <p>
              <span className="text-orange-400">Stop:</span>{' '}
              {data.dependencies.join(' → ')}
            </p>
            <p className="mt-1">
              <span className="text-green-400">Start:</span>{' '}
              {[...data.dependencies].reverse().join(' → ')}
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
