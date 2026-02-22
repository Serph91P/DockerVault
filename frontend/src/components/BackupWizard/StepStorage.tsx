import { useState, useEffect } from 'react'
import { Cloud, HardDrive, Plus, Check, ExternalLink } from 'lucide-react'
import { WizardData } from './index'
import { RemoteStorage } from '../../api'

interface Props {
  data: WizardData
  updateData: (updates: Partial<WizardData>) => void
  storages: RemoteStorage[]
  isLoadingStorages: boolean
}

// Storage type icons and colors
const STORAGE_TYPES: Record<string, { icon: React.ReactNode; color: string; name: string }> = {
  s3: {
    icon: <Cloud className="w-5 h-5" />,
    color: 'text-orange-400',
    name: 'Amazon S3',
  },
  ftp: {
    icon: <HardDrive className="w-5 h-5" />,
    color: 'text-blue-400',
    name: 'FTP Server',
  },
  sftp: {
    icon: <HardDrive className="w-5 h-5" />,
    color: 'text-green-400',
    name: 'SFTP Server',
  },
  webdav: {
    icon: <Cloud className="w-5 h-5" />,
    color: 'text-purple-400',
    name: 'WebDAV',
  },
  local: {
    icon: <HardDrive className="w-5 h-5" />,
    color: 'text-gray-400',
    name: 'Local Path',
  },
}

export default function StepStorage({ data, updateData, storages, isLoadingStorages }: Props) {
  const [hasAutoSelected, setHasAutoSelected] = useState(false)

  const enabledStorages = storages.filter((s) => s.enabled !== false)

  // Smart default: auto-select the single enabled storage if only one exists
  useEffect(() => {
    if (hasAutoSelected || data.remoteStorageIds.length > 0) return
    if (isLoadingStorages) return

    if (enabledStorages.length === 1) {
      updateData({ remoteStorageIds: [enabledStorages[0].id] })
    }
    setHasAutoSelected(true)
  }, [enabledStorages, isLoadingStorages, hasAutoSelected])

  const toggleStorage = (storageId: number) => {
    const newIds = data.remoteStorageIds.includes(storageId)
      ? data.remoteStorageIds.filter((id) => id !== storageId)
      : [...data.remoteStorageIds, storageId]
    updateData({ remoteStorageIds: newIds })
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Remote Storage</h3>
        <p className="text-sm text-dark-400">
          Select remote storage locations to sync backups to (optional)
        </p>
      </div>

      {/* Local storage info */}
      <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
        <div className="flex items-start gap-3">
          <div className="p-2 bg-dark-700 rounded-lg">
            <HardDrive className="w-5 h-5 text-dark-300" />
          </div>
          <div>
            <h4 className="text-dark-100 font-medium">Local Storage</h4>
            <p className="text-sm text-dark-400 mt-1">
              Backups are always stored locally first. Remote storage provides additional redundancy.
            </p>
          </div>
          <Check className="w-5 h-5 text-green-400 ml-auto" />
        </div>
      </div>

      {/* Remote storage selection */}
      {isLoadingStorages ? (
        <div className="flex items-center justify-center py-8">
          <div className="animate-spin rounded-full h-8 w-8 border-2 border-primary-500 border-t-transparent" />
        </div>
      ) : enabledStorages.length === 0 ? (
        <div className="bg-dark-800 rounded-xl border border-dark-700 p-6 text-center">
          <Cloud className="w-12 h-12 text-dark-500 mx-auto mb-3" />
          <h4 className="text-dark-300 font-medium mb-2">No Remote Storage Configured</h4>
          <p className="text-sm text-dark-400 mb-4">
            You can configure remote storage locations in Settings to enable backup sync.
          </p>
          <a
            href="/storage"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 px-4 py-2 bg-primary-500/20 text-primary-400 rounded-lg hover:bg-primary-500/30 transition-colors text-sm"
          >
            <Plus className="w-4 h-4" />
            Configure Storage
            <ExternalLink className="w-3 h-3" />
          </a>
        </div>
      ) : (
        <div className="space-y-3">
          <p className="text-sm text-dark-300">
            Select one or more remote storage locations:
          </p>
          
          {enabledStorages.map((storage) => {
            const isSelected = data.remoteStorageIds.includes(storage.id)
            const typeInfo = STORAGE_TYPES[storage.storage_type] || STORAGE_TYPES.local

            return (
              <button
                key={storage.id}
                onClick={() => toggleStorage(storage.id)}
                className={`w-full flex items-center gap-4 p-4 rounded-xl border transition-all text-left ${
                  isSelected
                    ? 'border-primary-500 bg-primary-500/10'
                    : 'border-dark-700 hover:border-dark-600 bg-dark-800'
                }`}
              >
                <div className={`p-2 rounded-lg ${isSelected ? 'bg-primary-500/20' : 'bg-dark-700'}`}>
                  <span className={typeInfo.color}>{typeInfo.icon}</span>
                </div>

                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <h4 className="text-dark-100 font-medium truncate">{storage.name}</h4>
                    <span className="px-2 py-0.5 bg-dark-700 text-dark-400 text-xs rounded">
                      {typeInfo.name}
                    </span>
                  </div>
                  {storage.host && (
                    <p className="text-sm text-dark-400 truncate mt-1">
                      {storage.host}
                      {storage.base_path && `:${storage.base_path}`}
                    </p>
                  )}
                  {storage.s3_bucket && (
                    <p className="text-sm text-dark-400 truncate mt-1">
                      Bucket: {storage.s3_bucket}
                    </p>
                  )}
                </div>

                <div
                  className={`w-6 h-6 rounded-full border-2 flex items-center justify-center transition-colors ${
                    isSelected
                      ? 'border-primary-500 bg-primary-500'
                      : 'border-dark-600 bg-transparent'
                  }`}
                >
                  {isSelected && <Check className="w-4 h-4 text-white" />}
                </div>
              </button>
            )
          })}
        </div>
      )}

      {/* Selected storage summary */}
      {data.remoteStorageIds.length > 0 && (
        <div className="bg-primary-500/10 border border-primary-500/30 rounded-xl p-4">
          <div className="flex items-start gap-3">
            <Cloud className="w-5 h-5 text-primary-400 mt-0.5" />
            <div>
              <h4 className="text-primary-400 font-medium">
                {data.remoteStorageIds.length} Remote Location{data.remoteStorageIds.length > 1 ? 's' : ''} Selected
              </h4>
              <p className="text-sm text-dark-300 mt-1">
                Backups will be automatically synced to these locations after each backup completes.
              </p>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
