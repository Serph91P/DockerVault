import { Container, Database, FolderOpen, Layers } from 'lucide-react'
import { WizardData } from './index'
import { Container as ContainerType, Volume, Stack } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: ContainerType[]
  volumes: Volume[]
  stacks: Stack[]
}

const TARGET_TYPES = [
  {
    id: 'container' as const,
    name: 'Container',
    description: 'Backup all volumes attached to a container',
    icon: Container,
  },
  {
    id: 'volume' as const,
    name: 'Volume',
    description: 'Backup a specific Docker volume',
    icon: Database,
  },
  {
    id: 'stack' as const,
    name: 'Stack',
    description: 'Backup an entire Docker Compose stack',
    icon: Layers,
  },
  {
    id: 'path' as const,
    name: 'Host Path',
    description: 'Backup a directory on the host system',
    icon: FolderOpen,
  },
]

export default function StepTargetSelect({
  data,
  updateData,
  containers,
  volumes,
  stacks,
}: Props) {
  const handleTypeSelect = (type: WizardData['targetType']) => {
    updateData({
      targetType: type,
      containerName: '',
      volumeName: '',
      hostPath: '',
      stackName: '',
      targetName: '',
      dependencies: [],
    })
  }

  const handleTargetChange = (value: string) => {
    switch (data.targetType) {
      case 'container':
        updateData({
          containerName: value,
          targetName: data.targetName || `Backup: ${value}`,
        })
        break
      case 'volume':
        updateData({
          volumeName: value,
          targetName: data.targetName || `Volume: ${value}`,
        })
        break
      case 'stack':
        updateData({
          stackName: value,
          targetName: data.targetName || `Stack: ${value}`,
        })
        break
      case 'path':
        updateData({
          hostPath: value,
          targetName: data.targetName || `Path: ${value.split('/').pop()}`,
        })
        break
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Select Target Type</h3>
        <p className="text-sm text-dark-400">Choose what you want to backup</p>
      </div>

      {/* Target Type Selection */}
      <div className="grid grid-cols-2 gap-4">
        {TARGET_TYPES.map((type) => {
          const Icon = type.icon
          const isSelected = data.targetType === type.id
          return (
            <button
              key={type.id}
              onClick={() => handleTypeSelect(type.id)}
              className={`flex items-start gap-4 p-4 rounded-xl border-2 transition-all text-left ${
                isSelected
                  ? 'border-primary-500 bg-primary-500/10'
                  : 'border-dark-700 hover:border-dark-600 bg-dark-800'
              }`}
            >
              <div
                className={`p-2 rounded-lg ${
                  isSelected ? 'bg-primary-500/20 text-primary-400' : 'bg-dark-700 text-dark-400'
                }`}
              >
                <Icon className="w-6 h-6" />
              </div>
              <div>
                <h4 className="font-medium text-dark-100">{type.name}</h4>
                <p className="text-sm text-dark-400 mt-1">{type.description}</p>
              </div>
            </button>
          )
        })}
      </div>

      {/* Target Selection based on type */}
      {data.targetType && (
        <div className="space-y-4 pt-4 border-t border-dark-700">
          <h3 className="text-lg font-semibold text-dark-100">Select {data.targetType}</h3>

          {data.targetType === 'container' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">
                Container
              </label>
              <select
                value={data.containerName}
                onChange={(e) => handleTargetChange(e.target.value)}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
              >
                <option value="">Select a container...</option>
                {containers.map((c) => (
                  <option key={c.id} value={c.name}>
                    {c.name} ({c.image})
                  </option>
                ))}
              </select>
              {containers.length === 0 && (
                <p className="text-sm text-orange-400 mt-2">No containers found</p>
              )}
            </div>
          )}

          {data.targetType === 'volume' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Volume</label>
              <select
                value={data.volumeName}
                onChange={(e) => handleTargetChange(e.target.value)}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
              >
                <option value="">Select a volume...</option>
                {volumes.map((v) => (
                  <option key={v.name} value={v.name}>
                    {v.name}
                  </option>
                ))}
              </select>
              {volumes.length === 0 && (
                <p className="text-sm text-orange-400 mt-2">No volumes found</p>
              )}
            </div>
          )}

          {data.targetType === 'stack' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Stack</label>
              <select
                value={data.stackName}
                onChange={(e) => handleTargetChange(e.target.value)}
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
              >
                <option value="">Select a stack...</option>
                {stacks.map((s) => (
                  <option key={s.name} value={s.name}>
                    {s.name} ({s.containers.length} containers)
                  </option>
                ))}
              </select>
              {stacks.length === 0 && (
                <p className="text-sm text-orange-400 mt-2">No stacks found</p>
              )}
            </div>
          )}

          {data.targetType === 'path' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">
                Host Path
              </label>
              <input
                type="text"
                value={data.hostPath}
                onChange={(e) => handleTargetChange(e.target.value)}
                placeholder="/path/to/directory"
                className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 font-mono"
              />
              <p className="text-sm text-dark-500 mt-2">
                Enter the absolute path to the directory you want to backup
              </p>
            </div>
          )}

          {/* Target Name */}
          <div className="pt-4">
            <label className="block text-sm font-medium text-dark-300 mb-2">
              Backup Name
            </label>
            <input
              type="text"
              value={data.targetName}
              onChange={(e) => updateData({ targetName: e.target.value })}
              placeholder="Enter a name for this backup target"
              className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
            />
            <p className="text-sm text-dark-500 mt-2">
              A descriptive name to identify this backup target
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
