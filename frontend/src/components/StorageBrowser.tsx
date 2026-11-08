import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
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
import { responseStatus } from '../api/responses'
import { clsx } from 'clsx'
import ConfirmDialog from './ConfirmDialog'
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session'
import { operationsApi, operationActive, type CreateStorageOperation, type StorageOperation } from '../api/operations'
import StorageOperationResults from './StorageOperationResults'
import { queryKeys } from '../api/queryKeys'

const NO_OPERATIONS: StorageOperation[] = []

function subscribeSession(listener: () => void) {
  const signal = sessionSignal()
  signal.addEventListener('abort', listener)
  return () => signal.removeEventListener('abort', listener)
}

interface StorageFile {
  name: string
  is_symlink?: boolean
  size: number | null
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
  initialPath?: string
  inline?: boolean
  onPathChange?: (path: string) => void
  filter?: string
  kind?: string
  sort?: string
  direction?: string
  onViewChange?: (key: string, value: string) => void
}

function formatSize(bytes: number | null): string {
  if (bytes === null || bytes < 0) return 'Unavailable'
  if (bytes === 0) return '0 B'
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

function DirectorySize({ storage, path, revision }: { storage: StorageBrowserTarget; path: string; revision: number }) {
  const { data, isPending } = useQuery({
    queryKey: queryKeys.storageSize(storage.id, storage.base_path, path, revision, sessionGeneration()),
    queryFn: async ({ signal }) => (await storageApi.directorySize(storage.id, path, signal)).data,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  })
  return <>{isPending ? 'Loading size...' : data?.state === 'completed' && data.bytes !== null ? formatSize(data.bytes) : 'Unavailable'}</>
}

export default function StorageBrowser(props: StorageBrowserProps) {
  const generation = useSyncExternalStore(subscribeSession, sessionGeneration)
  const [owner] = useState(generation)
  if (generation !== owner) return null
  return <StorageBrowserContent key={`${props.storage.id}:${props.storage.base_path}:${owner}`} {...props} />
}

function StorageBrowserContent({ storage, onClose, initialPath = '', inline = false, onPathChange, filter = '', kind = 'all', sort = 'name', direction = 'asc', onViewChange }: StorageBrowserProps) {
  const queryClient = useQueryClient()
  const [localPath, setLocalPath] = useState(initialPath)
  const currentPath = onPathChange ? initialPath : localPath
  const setCurrentPath = (path: string) => { if (onPathChange) onPathChange(path); else setLocalPath(path) }
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set())
  const [isSelecting, setIsSelecting] = useState(false)
  const [bulkDeleteTarget, setBulkDeleteTarget] = useState<string[] | null>(null)
  const [selectionNotice, setSelectionNotice] = useState('')
  const scope = `${currentPath}:${filter}:${kind}`
  const [selectionScope, setSelectionScope] = useState(scope)
  const [downloadError, setDownloadError] = useState('')
  const [downloadUrl, setDownloadUrl] = useState('')
  const [downloadRequest, setDownloadRequest] = useState(0)
  if (scope !== selectionScope) {
    setSelectionScope(scope)
    setSelectedFiles(new Set())
    setDeleteTarget(null)
    setBulkDeleteTarget(null)
    setSelectionNotice('Selection cleared after folder or filter change.')
    setDownloadError('')
    setDownloadUrl('')
  }

  const isLocal = storage.id === 'local'

  const buildFilePath = (name: string) =>
    currentPath ? `${currentPath}/${name}` : name

  const { data, dataUpdatedAt, isLoading, error, refetch } = useQuery({
    queryKey: queryKeys.storageFiles(storage.id, currentPath, storage.base_path, sessionGeneration()),
    queryFn: async ({ signal }) => {
      const response = isLocal
        ? await storageApi.listLocalFiles(currentPath, signal)
        : await storageApi.listFiles(storage.id as number, currentPath, signal)
      return response.data as { files: StorageFile[]; path: string }
    },
  })

  const [owner] = useState(sessionGeneration)
  const mounted = useRef(true)
  const submitting = useRef(false)
  const unfinishedRequest = useRef<CreateStorageOperation | null>(null)
  const retryRequests = useRef(new Map<string, string>())
  const settled = useRef(new Set<string>())
  const operationsKey = queryKeys.storageOperations(storage.id, owner)
  const operations = useQuery({
    queryKey: operationsKey,
    queryFn: async ({ signal }) => (await operationsApi.list(storage.id, signal)).data,
    retry: 2,
    retryDelay: attempt => Math.min(1000 * 2 ** attempt, 4000),
    refetchInterval: query => query.state.status === 'error' ? false : query.state.data?.some(operationActive) ? 2000 : false,
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: true,
    staleTime: 0,
  })
  const jobs = operations.data ?? NO_OPERATIONS
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])
  useEffect(() => {
    const terminal = jobs.filter(job => !operationActive(job) && !settled.current.has(job.id))
    if (terminal.length && !operations.isError) {
      terminal.forEach(job => settled.current.add(job.id))
      void queryClient.invalidateQueries({ queryKey: ['storage-files', storage.id] })
      void queryClient.invalidateQueries({ queryKey: ['storage-size', storage.id] })
    }
  }, [jobs, operations.isError, queryClient, storage.id])

  const refreshOperations = async () => {
    if (!mounted.current || !ownsSession(owner)) return
    // Invalidation alone reuses an initial in-flight read with no cached data.
    await queryClient.cancelQueries({ queryKey: operationsKey })
    if (mounted.current && ownsSession(owner)) await queryClient.invalidateQueries({ queryKey: operationsKey })
  }
  const command = useMutation({
    mutationFn: async (request: { kind: 'create'; data: CreateStorageOperation } | { kind: 'retry'; id: string; request_id: string } | { kind: 'cancel'; id: string }) => {
      if (request.kind === 'create') return operationsApi.create(request.data)
      if (request.kind === 'retry') return operationsApi.retry(request.id, request.request_id)
      return operationsApi.cancel(request.id)
    },
    onSuccess: (response, request) => {
      if (!mounted.current || !ownsSession(owner)) return
      if (request.kind !== 'cancel' && 'operation_id' in response.data) onViewChange?.('operation', response.data.operation_id)
      if (request.kind === 'create') {
        unfinishedRequest.current = null
        setDeleteTarget(null)
        setBulkDeleteTarget(null)
        setSelectedFiles(new Set())
      }

      void refreshOperations()
    },
    onError: (error, request) => {
      if (!mounted.current || !ownsSession(owner)) return
      // These API rejections precede admission; transport/timeouts/5xx remain ambiguous.
      if (request.kind === 'create' && [400, 401, 403, 404, 409, 422, 429].includes(responseStatus(error) ?? 0)) unfinishedRequest.current = null
      setDeleteTarget(null)
      setBulkDeleteTarget(null)
      void refreshOperations()
    },
    onSettled: () => { submitting.current = false },
  })
  const startDelete = (paths: string[]) => {
    if (submitting.current || !ownsSession(owner)) return
    submitting.current = true
    unfinishedRequest.current ??= { action: 'storage_delete', storage_id: storage.id, path: currentPath, paths, request_id: crypto.randomUUID() }
    command.mutate({ kind: 'create', data: unfinishedRequest.current })
  }
  const conflicts = (path: string) => jobs.some(job => operationActive(job) && job.items.some(item =>
    (item.state === 'pending' || item.state === 'running') && (item.path === path || item.path.startsWith(`${path}/`) || path.startsWith(`${item.path}/`))))


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
    const url = isLocal
      ? `/api/v1/storage/local/files/download?path=${encodeURIComponent(filePath)}`
      : `/api/v1/storage/${storage.id}/files/download?path=${encodeURIComponent(filePath)}`
    setDownloadError('')
    setDownloadRequest(request => request + 1)
    setDownloadUrl(url)
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
    setIsSelecting(true)
    setSelectedFiles(new Set(sortedFiles.map((f) => f.name)))
  }

  const handleBulkDownload = async () => {
    const downloadOwner = sessionGeneration()
    const paths = Array.from(selectedFiles).map(buildFilePath)
    try {
      const response = isLocal
        ? await storageApi.bulkDownloadLocalFiles(paths)
        : await storageApi.bulkDownloadFiles(storage.id as number, paths)
      if (!mounted.current || !ownsSession(downloadOwner)) return
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
      if (mounted.current && ownsSession(downloadOwner)) { setDownloadError('Selected ZIP download failed. Selection and folder are unchanged.'); toast.error('Bulk download failed') }
    }
  }

  const files = data?.files || []
  const sortedFiles = files.filter(file => file.name.toLocaleLowerCase().includes(filter.toLocaleLowerCase()) && (kind === 'all' || (file.is_symlink ? kind === 'link' : file.is_dir ? kind === 'directory' : kind === 'file'))).sort((a, b) => {
    if (sort === 'name' && a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1
    const value = (file: StorageFile) => sort === 'size' ? file.size : sort === 'modified' ? (typeof file.modified === 'number' ? file.modified : Date.parse(file.modified)) : file.name
    const av = value(a), bv = value(b)
    if (av == null || bv == null) return av == null && bv == null ? a.name.localeCompare(b.name) : av == null ? 1 : -1
    const compared = typeof av === 'string' && typeof bv === 'string' ? av.localeCompare(bv) : Number(av) - Number(bv)
    return (direction === 'desc' ? -compared : compared) || a.name.localeCompare(b.name)
  })

  const breadcrumbs = currentPath ? currentPath.split('/') : []

  return (
    <BrowserFrame inline={inline} title={`Browse ${storage.name}`} onClose={onClose}>
      <div
        className={clsx('bg-dark-800 border border-dark-700 shadow-2xl w-full flex flex-col', !inline && 'max-h-[90dvh]')}
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
              aria-pressed={isSelecting}
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
              aria-label="Close storage browser"
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

        {onViewChange && <div className="flex flex-wrap gap-3 p-3">
          <label>Filter loaded directory<input className="input" aria-label="Filter loaded directory" value={filter} onChange={e => onViewChange('q', e.target.value)} /></label>
          <label>Entry kind<select className="input" aria-label="Entry kind" value={kind} onChange={e => onViewChange('kind', e.target.value)}>{['all','file','directory','link'].map(value => <option key={value}>{value}</option>)}</select></label>
          <label>Sort entries<select className="input" aria-label="Sort entries" value={sort} onChange={e => onViewChange('sort', e.target.value)}>{['name','size','modified'].map(value => <option key={value}>{value}</option>)}</select></label>
          <label>Sort direction<select className="input" aria-label="Sort direction" value={direction} onChange={e => onViewChange('direction', e.target.value)}>{['asc','desc'].map(value => <option key={value}>{value}</option>)}</select></label>
          <p>{sortedFiles.length} visible entries in this loaded directory; no recursive search.</p>
        </div>}
        {selectionNotice && <p role="status" aria-label="Selection notice">{selectionNotice}</p>}
        {downloadError && <p role="alert">{downloadError}</p>}
        {downloadUrl && <><p role="status">Download requested for {new URL(downloadUrl, window.location.origin).searchParams.get('path')}. Browser saving is not confirmed.</p><iframe key={downloadRequest} title="Storage download response" className="hidden" src={downloadUrl} onError={() => setDownloadError('File download response unavailable.')} onLoad={e => {
          try { if (e.currentTarget.contentDocument?.body?.textContent?.trim()) setDownloadError('File download failed. Check access and refresh this folder before retrying.') } catch { setDownloadError('File download response unavailable.') }
        }} /></>}
        <div className="flex flex-wrap gap-3 items-center p-3 border-b border-dark-700">
          <input type="checkbox" aria-label="Select all entries" checked={sortedFiles.length > 0 && selectedFiles.size === sortedFiles.length}
            ref={element => { if (element) element.indeterminate = selectedFiles.size > 0 && selectedFiles.size < sortedFiles.length }}
            onChange={event => { if (event.target.checked) selectAll(); else setSelectedFiles(new Set()) }} />
          <button type="button" className="btn-secondary" disabled={!sortedFiles.length} onClick={selectAll}>Select All</button>
          <button type="button" className="btn-secondary" disabled={!selectedFiles.size} onClick={() => setSelectedFiles(new Set())}>Clear Selection</button>
          <span className="text-sm" aria-live="polite">{selectedFiles.size} selected</span>
        </div>
        {/* Content */}
        <div className={clsx('overflow-y-auto', inline ? 'min-h-40 max-h-[60dvh]' : 'flex-1')}>
          {isLoading ? (
            <div className="flex flex-col items-center justify-center py-16">
              <Loader2 className="w-8 h-8 text-primary-500 animate-spin mb-3" />
              <p className="text-sm text-dark-400">Loading files...</p>
            </div>
          ) : error ? (
            <div className="text-center py-16">
              <AlertTriangle className="w-10 h-10 text-red-400 mx-auto mb-3" />
              <p role="alert" className="text-red-400 font-medium">Failed to list files</p>
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
            <EntityTable label={`Files in ${storage.name}`}>
              <thead className="text-xs text-dark-500 uppercase bg-dark-900/50 sticky top-0">
                <tr>
                  <th scope="col" className="text-left px-4 py-2 font-medium">Name</th>
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
                    tabIndex={isSelecting ? undefined : 0}
                    onKeyDown={(event) => { if (!isSelecting && event.target === event.currentTarget && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); handleBack() } }}
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
                      tabIndex={file.is_dir || isSelecting ? 0 : undefined}
                      aria-selected={isSelecting ? isSelected : undefined}
                      onKeyDown={(event) => { if (event.target === event.currentTarget && (event.key === 'Enter' || event.key === ' ')) { if (isSelecting || file.is_dir) event.preventDefault(); if (isSelecting) toggleSelection(file.name); else if (file.is_dir) handleNavigate(file.name) } }}
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
                          {file.is_symlink && <span className="text-xs text-dark-400">Symbolic link</span>}
                          {file.is_dir && !isSelecting && (
                            <ChevronRight className="w-3 h-3 text-dark-600 ml-auto shrink-0" />
                          )}
                        </div>
                      </td>
                      <td className="px-4 py-2.5 text-right">
                        <span className="text-xs text-dark-500">
                          {file.is_dir ? <DirectorySize storage={storage} path={buildFilePath(file.name)} revision={dataUpdatedAt} /> : formatSize(file.size)}
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
                                disabled={command.isPending || conflicts(buildFilePath(file.name)) || !!unfinishedRequest.current}
                                className="p-1.5 rounded transition-colors text-dark-400 hover:text-red-400 hover:bg-dark-600"
                                title="Delete"
                              >
                                {command.isPending ? (
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
            </EntityTable>
          )}
        </div>

        {/* Selection action bar */}
        {selectedFiles.size > 0 && (
          <div className="border-t border-dark-700 px-4 py-3 flex items-center justify-between bg-dark-800">
            <div className="flex flex-wrap gap-2">
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
                disabled={command.isPending || !!unfinishedRequest.current || Array.from(selectedFiles).some(name => conflicts(buildFilePath(name)))}
                className="px-3 py-1.5 bg-red-600/10 hover:bg-red-600/20 text-red-400 rounded-lg text-sm flex items-center gap-1 transition-colors"
              >
                <Trash2 className="w-3.5 h-3.5" /> Delete
              </button>
            </div>
          </div>
        )}

        {command.isError && unfinishedRequest.current && <button type="button" className="btn-secondary" disabled={command.isPending} onClick={() => startDelete(unfinishedRequest.current!.paths)}>Retry request with same ID</button>}
        <StorageOperationResults operations={jobs} loading={operations.isPending} stale={operations.isError} pending={command.isPending} error={command.isError}
          onRefresh={() => { void refreshOperations() }}
          onFolder={path => { setCurrentPath(path); setSelectedFiles(new Set()); setDeleteTarget(null); setBulkDeleteTarget(null) }}
          onCancel={id => { if (!submitting.current) { submitting.current = true; command.mutate({ kind: 'cancel', id }) } }}
          onRetry={id => { if (!submitting.current) { submitting.current = true; const request_id = retryRequests.current.get(id) ?? crypto.randomUUID(); retryRequests.current.set(id, request_id); command.mutate({ kind: 'retry', id, request_id }) } }} />
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
          if (deleteTarget) startDelete([deleteTarget])
        }}
        title="Delete File"
        message={`Delete "${deleteTarget?.split('/').pop()}"? This cannot be undone.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={command.isPending}
      />

      {/* Bulk delete confirm */}
      <ConfirmDialog
        isOpen={bulkDeleteTarget !== null}
        onClose={() => setBulkDeleteTarget(null)}
        onConfirm={() => {
          if (bulkDeleteTarget) startDelete(bulkDeleteTarget)
        }}
        title="Delete Files"
        message={`Delete ${bulkDeleteTarget?.length} selected entries (files, folders or links) from ${currentPath || '/'}? This cannot be undone.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={command.isPending}
      />
    </BrowserFrame>
  )
}
import { Modal } from './ui/dialog'
import { EntityTable } from './ui/workspace'

function BrowserFrame({ inline, title, onClose, children }: { inline: boolean; title: string; onClose: () => void; children: React.ReactNode }) {
  return inline ? <section aria-label={title}>{children}</section> : <Modal title={title} onClose={onClose}>{children}</Modal>
}
