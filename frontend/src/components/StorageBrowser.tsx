import { useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  X,
  Folder,
  File,
  Download,
  Trash2,
  ChevronRight,
  ArrowLeft,
  Loader2,
  HardDrive,
  FileArchive,
  RefreshCw,
  AlertTriangle,
  CheckSquare,
  Square,
  Check,
} from 'lucide-react'
import { RemoteStorage, storageApi } from '../api'
import toast from 'react-hot-toast'
import { clsx } from 'clsx'
import ConfirmDialog from './ConfirmDialog'

interface StorageFile {
  name: string
  size: number
  is_dir: boolean
  modified: number | string
}

export interface StorageBrowserTarget {
  id: number | 'local'
  name: string
  storage_type: string
  base_path: string
}

interface StorageBrowserProps {
  storage: RemoteStorage | StorageBrowserTarget
  onClose: () => void
}

function formatSize(bytes: number): string {
  if (!bytes || bytes <= 0) return '—'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

function formatDate(timestamp: number | string): string {
  if (!timestamp) return '—'
  let date: Date
  if (typeof timestamp === 'number') {
    date = timestamp > 1e12 ? new Date(timestamp) : new Date(timestamp * 1000)
  } else {
    date = new Date(timestamp)
  }
  if (isNaN(date.getTime())) return '—'
  return date.toLocaleString()
}

function getFileIcon(name: string, isDir: boolean) {
  if (isDir) return Folder
  const ext = name.split('.').pop()?.toLowerCase() || ''
  if (['tar', 'gz', 'bz2', 'xz', 'zip', '7z', 'enc'].includes(ext)) return FileArchive
  return File
}

export default function StorageBrowser({ storage, onClose }: StorageBrowserProps) {
  const queryClient = useQueryClient()
  const [currentPath, setCurrentPath] = useState('')
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set())
  const [isSelecting, setIsSelecting] = useState(false)
  const [bulkDeleteTarget, setBulkDeleteTarget] = useState<string[] | null>(null)

  const isLocal = storage.id === 'local'

  const buildFilePath = (name: string) =>
    currentPath ? `${currentPath}/${name}` : name

  const { data, isLoading, error, refetch } = useQuery({
    queryKey: ['storage-files', storage.id, currentPath],
    queryFn: async () => {
      const response = isLocal
        ? await storageApi.listLocalFiles(currentPath)
        : await storageApi.listFiles(storage.id as number, currentPath)
      return response.data as { files: StorageFile[]; path: string }
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (filePath: string) =>
      isLocal
        ? storageApi.deleteLocalFile(filePath)
        : storageApi.deleteFile(storage.id as number, filePath),
    onSuccess: (_data, filePath) => {
      const fileName = filePath.split('/').pop()
      toast.success(`Deleted ${fileName}`)
      queryClient.invalidateQueries({ queryKey: ['storage-files', storage.id] })
      setDeleteTarget(null)
    },
    onError: () => {
      toast.error('Failed to delete file')
      setDeleteTarget(null)
    },
  })

  const bulkDeleteMutation = useMutation({
    mutationFn: (paths: string[]) =>
      isLocal
        ? storageApi.bulkDeleteLocalFiles(paths)
        : storageApi.bulkDeleteFiles(storage.id as number, paths),
    onSuccess: (response) => {
      const { succeeded, failed } = response.data
      if (failed > 0) {
        toast.error(`Deleted ${succeeded} files, ${failed} failed`)
      } else {
        toast.success(`Deleted ${succeeded} files`)
      }
      queryClient.invalidateQueries({ queryKey: ['storage-files', storage.id] })
      setSelectedFiles(new Set())
      setBulkDeleteTarget(null)
    },
    onError: () => {
      toast.error('Bulk delete failed')
      setBulkDeleteTarget(null)
    },
  })

  const handleNavigate = (dirName: string) => {
    const newPath = currentPath ? `${currentPath}/${dirName}` : dirName
    setCurrentPath(newPath)
    setDeleteTarget(null)
    setSelectedFiles(new Set())
  }

  const handleBack = () => {
    const parts = currentPath.split('/')
    parts.pop()
    setCurrentPath(parts.join('/'))
    setDeleteTarget(null)
    setSelectedFiles(new Set())
  }

  const handleDownload = (fileName: string) => {
    const filePath = buildFilePath(fileName)
    const downloadUrl = isLocal
      ? `/api/v1/storage/local/files/download?path=${encodeURIComponent(filePath)}`
      : `/api/v1/storage/${storage.id}/files/download?path=${encodeURIComponent(filePath)}`
    const a = document.createElement('a')
    a.href = downloadUrl
    a.download = fileName
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
  }

  const handleDelete = (fileName: string) => {
    setDeleteTarget(buildFilePath(fileName))
  }

  const toggleSelection = (name: string) => {
    setSelectedFiles((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  const selectAll = () => {
    if (selectedFiles.size === sortedFiles.length) {
      setSelectedFiles(new Set())
    } else {
      setSelectedFiles(new Set(sortedFiles.map((f) => f.name)))
    }
  }

  const handleBulkDownload = async () => {
    const paths = Array.from(selectedFiles).map(buildFilePath)
    try {
      const response = isLocal
        ? await storageApi.bulkDownloadLocalFiles(paths)
        : await storageApi.bulkDownloadFiles(storage.id as number, paths)
      const url = window.URL.createObjectURL(new Blob([response.data]))
      const a = document.createElement('a')
      a.href = url
      const dirName = currentPath
        ? currentPath.split('/').pop()
        : storage.base_path?.split('/').pop() || storage.name
      a.download = `${dirName || 'download'}.zip`
      document.body.appendChild(a)
      a.click()
      document.body.removeChild(a)
      window.URL.revokeObjectURL(url)
    } catch {
      toast.error('Bulk download failed')
    }
  }

  const files = data?.files || []
  const sortedFiles = [...files].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1
    return a.name.localeCompare(b.name)
  })

  const breadcrumbs = currentPath ? currentPath.split('/') : []

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/50 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="bg-dark-800 border-l border-dark-700 shadow-2xl w-fit min-w-[48rem] max-w-[90vw] h-full flex flex-col animate-slide-in-right"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-dark-700">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 bg-indigo-500/10 rounded-lg flex items-center justify-center">
              <HardDrive className="w-5 h-5 text-indigo-400" />
            </div>
            <div>
              <h2 className="text-lg font-semibold text-dark-100">
                Browse {storage.name}
              </h2>
              <p className="text-sm text-dark-400 capitalize">
                {storage.storage_type}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => {
                setIsSelecting(!isSelecting)
                if (isSelecting) setSelectedFiles(new Set())
              }}
              className={clsx(
                'p-2 rounded-lg transition-colors',
                isSelecting
                  ? 'text-primary-400 bg-primary-500/10'
                  : 'text-dark-400 hover:text-dark-200 hover:bg-dark-700'
              )}
              title="Select files"
            >
              <CheckSquare className="w-4 h-4" />
            </button>
            <button
              onClick={() => refetch()}
              className="p-2 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
              title="Refresh"
            >
              <RefreshCw className="w-4 h-4" />
            </button>
            <button
              onClick={onClose}
              className="p-2 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* Breadcrumbs */}
        <div className="flex items-center gap-1 px-4 py-2 border-b border-dark-700 text-sm overflow-x-auto">
          <button
            onClick={() => {
              setCurrentPath('')
              setDeleteTarget(null)
              setSelectedFiles(new Set())
            }}
            className={clsx(
              'px-2 py-1 rounded hover:bg-dark-700 transition-colors shrink-0',
              !currentPath ? 'text-primary-400 font-medium' : 'text-dark-400'
            )}
          >
            {storage.base_path || '/'}
          </button>
          {breadcrumbs.map((segment, i) => (
            <div key={i} className="flex items-center gap-1 shrink-0">
              <ChevronRight className="w-3 h-3 text-dark-600" />
              <button
                onClick={() => {
                  setCurrentPath(breadcrumbs.slice(0, i + 1).join('/'))
                  setDeleteTarget(null)
                  setSelectedFiles(new Set())
                }}
                className={clsx(
                  'px-2 py-1 rounded hover:bg-dark-700 transition-colors',
                  i === breadcrumbs.length - 1
                    ? 'text-primary-400 font-medium'
                    : 'text-dark-400'
                )}
              >
                {segment}
              </button>
            </div>
          ))}
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto">
          {isLoading ? (
            <div className="flex flex-col items-center justify-center py-16">
              <Loader2 className="w-8 h-8 text-primary-500 animate-spin mb-3" />
              <p className="text-sm text-dark-400">Loading files...</p>
            </div>
          ) : error ? (
            <div className="text-center py-16">
              <AlertTriangle className="w-10 h-10 text-red-400 mx-auto mb-3" />
              <p className="text-red-400 font-medium">Failed to list files</p>
              <p className="text-sm text-dark-500 mt-1">
                Check that the storage connection is working
              </p>
              <button
                onClick={() => refetch()}
                className="mt-4 px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg transition-colors"
              >
                Retry
              </button>
            </div>
          ) : sortedFiles.length === 0 ? (
            <div className="text-center py-16">
              <Folder className="w-12 h-12 text-dark-600 mx-auto mb-4" />
              <p className="text-dark-400">This directory is empty</p>
            </div>
          ) : (
            <table className="w-full">
              <thead className="text-xs text-dark-500 uppercase bg-dark-900/50 sticky top-0">
                <tr>
                  <th className="text-left px-4 py-2 font-medium">Name</th>
                  <th className="text-right px-4 py-2 font-medium w-28">
                    Size
                  </th>
                  <th className="text-right px-4 py-2 font-medium w-44">
                    Modified
                  </th>
                  <th className="text-right px-4 py-2 font-medium w-24">
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-700/50">
                {currentPath && (
                  <tr
                    onClick={isSelecting ? undefined : handleBack}
                    className={clsx(
                      'hover:bg-dark-700/50 transition-colors',
                      !isSelecting && 'cursor-pointer'
                    )}
                  >
                    <td className="px-4 py-2.5" colSpan={4}>
                      <div className="flex items-center gap-2 text-dark-400">
                        {isSelecting && <span className="w-4" />}
                        <ArrowLeft className="w-4 h-4" />
                        <span className="text-sm">..</span>
                      </div>
                    </td>
                  </tr>
                )}
                {sortedFiles.map((file) => {
                  const Icon = getFileIcon(file.name, file.is_dir)
                  const isSelected = selectedFiles.has(file.name)
                  return (
                    <tr
                      key={file.name}
                      className={clsx(
                        'hover:bg-dark-700/50 transition-colors',
                        file.is_dir && !isSelecting && 'cursor-pointer',
                        isSelected && 'bg-primary-500/5'
                      )}
                      onClick={
                        isSelecting
                          ? () => toggleSelection(file.name)
                          : file.is_dir
                            ? () => handleNavigate(file.name)
                            : undefined
                      }
                    >
                      <td className="px-4 py-2.5">
                        <div className="flex items-center gap-2">
                          {isSelecting && (
                            <span className="shrink-0">
                              {isSelected ? (
                                <Check className="w-4 h-4 text-primary-400" />
                              ) : (
                                <Square className="w-4 h-4 text-dark-600" />
                              )}
                            </span>
                          )}
                          <Icon
                            className={clsx(
                              'w-4 h-4 shrink-0',
                              file.is_dir
                                ? 'text-yellow-500'
                                : 'text-dark-400'
                            )}
                          />
                          <span className="text-sm text-dark-200 whitespace-nowrap">
                            {file.name}
                          </span>
                          {file.is_dir && !isSelecting && (
                            <ChevronRight className="w-3 h-3 text-dark-600 ml-auto shrink-0" />
                          )}
                        </div>
                      </td>
                      <td className="px-4 py-2.5 text-right">
                        <span className="text-xs text-dark-500">
                          {file.is_dir ? '—' : formatSize(file.size)}
                        </span>
                      </td>
                      <td className="px-4 py-2.5 text-right">
                        <span className="text-xs text-dark-500">
                          {formatDate(file.modified)}
                        </span>
                      </td>
                      <td className="px-4 py-2.5">
                        <div
                          className="flex items-center justify-end gap-1"
                          onClick={(e) => e.stopPropagation()}
                        >
                          {!isSelecting && (
                            <>
                              {!file.is_dir && (
                                <button
                                  onClick={() => handleDownload(file.name)}
                                  className="p-1.5 text-dark-400 hover:text-primary-400 hover:bg-dark-600 rounded transition-colors"
                                  title="Download"
                                >
                                  <Download className="w-3.5 h-3.5" />
                                </button>
                              )}
                              <button
                                onClick={() => handleDelete(file.name)}
                                disabled={deleteMutation.isPending}
                                className="p-1.5 rounded transition-colors text-dark-400 hover:text-red-400 hover:bg-dark-600"
                                title="Delete"
                              >
                                {deleteMutation.isPending ? (
                                  <Loader2 className="w-3.5 h-3.5 animate-spin" />
                                ) : (
                                  <Trash2 className="w-3.5 h-3.5" />
                                )}
                              </button>
                            </>
                          )}
                        </div>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
        </div>

        {/* Selection action bar */}
        {selectedFiles.size > 0 && (
          <div className="border-t border-dark-700 px-4 py-3 flex items-center justify-between bg-dark-800">
            <span className="text-sm text-dark-300">
              {selectedFiles.size} selected
            </span>
            <div className="flex gap-2">
              <button
                onClick={selectAll}
                className="text-xs text-dark-400 hover:text-dark-200 transition-colors"
              >
                {selectedFiles.size === sortedFiles.length
                  ? 'Deselect all'
                  : 'Select all'}
              </button>
              <button
                onClick={handleBulkDownload}
                className="px-3 py-1.5 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg text-sm flex items-center gap-1 transition-colors"
              >
                <Download className="w-3.5 h-3.5" /> Download
              </button>
              <button
                onClick={() =>
                  setBulkDeleteTarget(
                    Array.from(selectedFiles).map(buildFilePath)
                  )
                }
                className="px-3 py-1.5 bg-red-600/10 hover:bg-red-600/20 text-red-400 rounded-lg text-sm flex items-center gap-1 transition-colors"
              >
                <Trash2 className="w-3.5 h-3.5" /> Delete
              </button>
            </div>
          </div>
        )}

        {/* Footer */}
        {selectedFiles.size === 0 && (
          <div className="flex items-center justify-between p-4 border-t border-dark-700">
            <div className="text-sm text-dark-400">
              {sortedFiles.length} items
            </div>
            <button
              onClick={onClose}
              className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg transition-colors"
            >
              Close
            </button>
          </div>
        )}
      </div>

      {/* Single delete confirm */}
      <ConfirmDialog
        isOpen={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget)
        }}
        title="Delete File"
        message={`Delete "${deleteTarget?.split('/').pop()}"? This cannot be undone.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deleteMutation.isPending}
      />

      {/* Bulk delete confirm */}
      <ConfirmDialog
        isOpen={bulkDeleteTarget !== null}
        onClose={() => setBulkDeleteTarget(null)}
        onConfirm={() => {
          if (bulkDeleteTarget) bulkDeleteMutation.mutate(bulkDeleteTarget)
        }}
        title="Delete Files"
        message={`Delete ${bulkDeleteTarget?.length} files? This cannot be undone.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={bulkDeleteMutation.isPending}
      />
    </div>
  )
}
