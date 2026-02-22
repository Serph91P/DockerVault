import { useState, useMemo } from 'react'
import { Container, Database, FolderOpen, Layers, Search, X, CheckCircle } from 'lucide-react'
import { WizardData } from './index'
import { Container as ContainerType, Volume, Stack, BackupTarget } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  containers: ContainerType[]
  volumes: Volume[]
  stacks: Stack[]
  existingTargets?: BackupTarget[]
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

interface SearchableListProps {
  items: { id: string; label: string; sublabel?: string; status?: string; configured?: boolean }[]
  selectedId: string
  onSelect: (id: string) => void
  placeholder: string
  emptyMessage: string
  icon: React.ComponentType<{ className?: string }>
}

function SearchableList({ items, selectedId, onSelect, placeholder, emptyMessage, icon: Icon }: SearchableListProps) {
  const [search, setSearch] = useState('')

  const filtered = useMemo(() => {
    const sorted = [...items].sort((a, b) => a.label.localeCompare(b.label))
    if (!search.trim()) return sorted
    const q = search.toLowerCase()
    return sorted.filter(
      (item) =>
        item.label.toLowerCase().includes(q) ||
        (item.sublabel && item.sublabel.toLowerCase().includes(q))
    )
  }, [items, search])

  if (items.length === 0) {
    return <p className="text-sm text-orange-400 mt-2">{emptyMessage}</p>
  }

  return (
    <div className="space-y-2">
      {/* Search */}
      <div className="relative">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-dark-500" />
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={placeholder}
          className="w-full pl-9 pr-8 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 text-sm placeholder:text-dark-500 focus:outline-none focus:border-primary-500"
        />
        {search && (
          <button
            onClick={() => setSearch('')}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-dark-500 hover:text-dark-300"
          >
            <X className="w-4 h-4" />
          </button>
        )}
      </div>

      {/* List */}
      <div className="max-h-64 overflow-y-auto rounded-lg border border-dark-700 divide-y divide-dark-700">
        {filtered.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-dark-500">
            No results for &quot;{search}&quot;
          </div>
        ) : (
          filtered.map((item) => {
            const isSelected = selectedId === item.id
            return (
              <button
                key={item.id}
                onClick={() => onSelect(item.id)}
                className={`w-full flex items-center gap-3 px-3 py-2.5 text-left transition-colors ${
                  isSelected
                    ? 'bg-primary-500/15 text-primary-300'
                    : 'bg-dark-800 text-dark-200 hover:bg-dark-750'
                }`}
              >
                <Icon className={`w-4 h-4 flex-shrink-0 ${isSelected ? 'text-primary-400' : 'text-dark-500'}`} />
                <div className="min-w-0 flex-1">
                  <span className="block text-sm font-medium truncate">{item.label}</span>
                  {item.sublabel && (
                    <span className="block text-xs text-dark-500 truncate">{item.sublabel}</span>
                  )}
                </div>
                {item.status && (
                  <span className={`flex-shrink-0 text-xs px-1.5 py-0.5 rounded ${
                    item.status === 'running'
                      ? 'bg-green-500/20 text-green-400'
                      : 'bg-dark-600 text-dark-400'
                  }`}>
                    {item.status}
                  </span>
                )}
                {item.configured && (
                  <span className="flex-shrink-0 flex items-center gap-1 text-xs px-1.5 py-0.5 rounded bg-yellow-500/15 text-yellow-400" title="A backup target already exists for this item">
                    <CheckCircle className="w-3 h-3" />
                    Configured
                  </span>
                )}
                {isSelected && (
                  <span className="flex-shrink-0 w-2 h-2 rounded-full bg-primary-400" />
                )}
              </button>
            )
          })
        )}
      </div>
      <p className="text-xs text-dark-500">
        {filtered.length} of {items.length} items
        {selectedId && <> &middot; 1 selected</>}
      </p>
    </div>
  )
}

export default function StepTargetSelect({
  data,
  updateData,
  containers,
  volumes,
  stacks,
  existingTargets = [],
}: Props) {
  // T3: Build sets of already-configured target names per type
  const configuredContainers = useMemo(() => new Set(
    existingTargets.filter((t) => t.target_type === 'container').map((t) => t.container_name)
  ), [existingTargets])
  const configuredVolumes = useMemo(() => new Set(
    existingTargets.filter((t) => t.target_type === 'volume').map((t) => t.volume_name)
  ), [existingTargets])
  const configuredStacks = useMemo(() => new Set(
    existingTargets.filter((t) => t.target_type === 'stack').map((t) => t.stack_name)
  ), [existingTargets])
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
          targetName: data.targetName || `${value} Backup`,
        })
        break
      case 'volume':
        updateData({
          volumeName: value,
          targetName: data.targetName || `${value} Backup`,
        })
        break
      case 'stack':
        updateData({
          stackName: value,
          targetName: data.targetName || `${value} Stack Backup`,
        })
        break
      case 'path':
        updateData({
          hostPath: value,
          targetName: data.targetName || `${value.split('/').pop() || value} Backup`,
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
              <SearchableList
                items={containers.map((c) => ({
                  id: c.name,
                  label: c.name,
                  sublabel: c.image,
                  status: c.status,
                  configured: configuredContainers.has(c.name),
                }))}
                selectedId={data.containerName}
                onSelect={handleTargetChange}
                placeholder="Search containers..."
                emptyMessage="No containers found"
                icon={Container}
              />
            </div>
          )}

          {data.targetType === 'volume' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Volume</label>
              <SearchableList
                items={volumes.map((v) => ({
                  id: v.name,
                  label: v.name,
                  sublabel: v.used_by?.length ? `Used by: ${v.used_by.join(', ')}` : 'Unused',
                  configured: configuredVolumes.has(v.name),
                }))}
                selectedId={data.volumeName}
                onSelect={handleTargetChange}
                placeholder="Search volumes..."
                emptyMessage="No volumes found"
                icon={Database}
              />
            </div>
          )}

          {data.targetType === 'stack' && (
            <div>
              <label className="block text-sm font-medium text-dark-300 mb-2">Stack</label>
              <SearchableList
                items={stacks.map((s) => ({
                  id: s.name,
                  label: s.name,
                  sublabel: `${s.containers.length} container${s.containers.length !== 1 ? 's' : ''} · ${s.volumes.length} volume${s.volumes.length !== 1 ? 's' : ''}`,
                  configured: configuredStacks.has(s.name),
                }))}
                selectedId={data.stackName}
                onSelect={handleTargetChange}
                placeholder="Search stacks..."
                emptyMessage="No stacks found"
                icon={Layers}
              />
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
