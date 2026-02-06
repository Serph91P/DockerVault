import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  X,
  Folder,
  File,
  Download,
  ChevronRight,
  ChevronDown,
  Archive,
  Loader2,
  FileText,
  FileCode,
  FileImage,
  FileArchive,
} from 'lucide-react'
import { Backup, backupsApi } from '../api'
import { clsx } from 'clsx'

interface BackupFile {
  name: string
  path: string
  size: number
  size_human: string
  is_dir: boolean
  mode: string
  mtime: string
}

interface BackupBrowserProps {
  backup: Backup
  onClose: () => void
}

// Get icon based on file extension
function getFileIcon(fileName: string, isDir: boolean) {
  if (isDir) return Folder
  
  const ext = fileName.split('.').pop()?.toLowerCase() || ''
  
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'ico'].includes(ext)) {
    return FileImage
  }
  if (['zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar'].includes(ext)) {
    return FileArchive
  }
  if (['js', 'ts', 'tsx', 'jsx', 'py', 'json', 'yml', 'yaml', 'xml', 'html', 'css', 'sh', 'bash'].includes(ext)) {
    return FileCode
  }
  if (['txt', 'md', 'log', 'conf', 'cfg', 'ini', 'env'].includes(ext)) {
    return FileText
  }
  
  return File
}

// Format bytes to human-readable size
function formatSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(1024))
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

// File Row Component
function FileRow({
  file,
  depth,
  backupId,
  expanded,
  onToggle,
  children,
}: {
  file: BackupFile
  depth: number
  backupId: number
  expanded: boolean
  onToggle: () => void
  children?: React.ReactNode
}) {
  const [downloading, setDownloading] = useState(false)
  const Icon = getFileIcon(file.name, file.is_dir)

  const handleDownload = async () => {
    if (file.is_dir) return
    
    setDownloading(true)
    try {
      // Create download link
      const response = await fetch(`/api/backups/${backupId}/files/${encodeURIComponent(file.path)}`)
      if (!response.ok) throw new Error('Download failed')
      
      const blob = await response.blob()
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = file.name
      document.body.appendChild(a)
      a.click()
      document.body.removeChild(a)
      URL.revokeObjectURL(url)
    } catch {
      console.error('Failed to download file')
    } finally {
      setDownloading(false)
    }
  }

  return (
    <>
      <div
        className={clsx(
          'flex items-center gap-2 py-2 px-3 hover:bg-dark-700 rounded-lg transition-colors',
          file.is_dir && 'cursor-pointer'
        )}
        style={{ paddingLeft: `${depth * 20 + 12}px` }}
        onClick={file.is_dir ? onToggle : undefined}
      >
        {/* Expand/Collapse Arrow for Directories */}
        {file.is_dir ? (
          expanded ? (
            <ChevronDown className="w-4 h-4 text-dark-400 shrink-0" />
          ) : (
            <ChevronRight className="w-4 h-4 text-dark-400 shrink-0" />
          )
        ) : (
          <span className="w-4 shrink-0" />
        )}

        {/* Icon */}
        <Icon
          className={clsx(
            'w-4 h-4 shrink-0',
            file.is_dir ? 'text-yellow-500' : 'text-dark-400'
          )}
        />

        {/* Name */}
        <span className="flex-1 text-sm text-dark-200 truncate">{file.name}</span>

        {/* Size */}
        {!file.is_dir && (
          <span className="text-xs text-dark-500 shrink-0">{file.size_human || formatSize(file.size)}</span>
        )}

        {/* Download Button */}
        {!file.is_dir && (
          <button
            onClick={(e) => {
              e.stopPropagation()
              handleDownload()
            }}
            disabled={downloading}
            className="p-1.5 text-dark-400 hover:text-primary-400 hover:bg-dark-600 rounded transition-colors"
            title="Download file"
          >
            {downloading ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Download className="w-4 h-4" />
            )}
          </button>
        )}
      </div>
      {expanded && children}
    </>
  )
}

// Recursive File Tree Component
function FileTree({
  files,
  depth,
  backupId,
}: {
  files: BackupFile[]
  depth: number
  backupId: number
}) {
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set())

  const toggleDir = (path: string) => {
    setExpandedDirs((prev) => {
      const next = new Set(prev)
      if (next.has(path)) {
        next.delete(path)
      } else {
        next.add(path)
      }
      return next
    })
  }

  // Group files by first path segment
  const groupedFiles = files.reduce(
    (acc, file) => {
      const parts = file.path.split('/').filter(Boolean)
      const firstPart = parts[0] || file.name

      if (!acc[firstPart]) {
        acc[firstPart] = {
          name: firstPart,
          path: firstPart,
          is_dir: parts.length > 1 || file.is_dir,
          size: file.size,
          size_human: file.size_human,
          mode: file.mode,
          mtime: file.mtime,
          children: [],
        }
      }

      if (parts.length > 1) {
        acc[firstPart].children.push({
          ...file,
          path: parts.slice(1).join('/'),
          name: parts[parts.length - 1],
        })
      }

      return acc
    },
    {} as Record<string, BackupFile & { children: BackupFile[] }>
  )

  // Sort: directories first, then alphabetically
  const sortedFiles = Object.values(groupedFiles).sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1
    return a.name.localeCompare(b.name)
  })

  return (
    <>
      {sortedFiles.map((file) => (
        <FileRow
          key={file.path}
          file={file}
          depth={depth}
          backupId={backupId}
          expanded={expandedDirs.has(file.path)}
          onToggle={() => toggleDir(file.path)}
        >
          {file.children.length > 0 && (
            <FileTree files={file.children} depth={depth + 1} backupId={backupId} />
          )}
        </FileRow>
      ))}
    </>
  )
}

export default function BackupBrowser({ backup, onClose }: BackupBrowserProps) {
  const { data: files, isLoading, error } = useQuery({
    queryKey: ['backup-files', backup.id],
    queryFn: async () => {
      const response = await backupsApi.listFiles(backup.id)
      return response.data as BackupFile[]
    },
  })

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="bg-dark-800 rounded-2xl border border-dark-700 shadow-2xl w-full max-w-3xl max-h-[80vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-dark-700">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 bg-primary-500/10 rounded-lg flex items-center justify-center">
              <Archive className="w-5 h-5 text-primary-500" />
            </div>
            <div>
              <h2 className="text-lg font-semibold text-dark-100">Browse Backup</h2>
              <p className="text-sm text-dark-400">
                {backup.file_path?.split('/').pop() || `Backup #${backup.id}`}
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-2 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-4">
          {isLoading ? (
            <div className="flex items-center justify-center py-12">
              <Loader2 className="w-8 h-8 text-primary-500 animate-spin" />
            </div>
          ) : error ? (
            <div className="text-center py-12">
              <p className="text-red-400">Failed to load backup contents</p>
              <p className="text-sm text-dark-500 mt-2">
                The backup file may be corrupted or inaccessible
              </p>
            </div>
          ) : files && files.length > 0 ? (
            <div className="space-y-0.5">
              <FileTree files={files} depth={0} backupId={backup.id} />
            </div>
          ) : (
            <div className="text-center py-12">
              <Archive className="w-12 h-12 text-dark-500 mx-auto mb-4" />
              <p className="text-dark-400">This backup is empty</p>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between p-4 border-t border-dark-700">
          <div className="text-sm text-dark-400">
            {files?.length || 0} items • {backup.file_size_human || '-'}
          </div>
          <button
            onClick={onClose}
            className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  )
}
