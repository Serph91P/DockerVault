import { useQuery } from '@tanstack/react-query'
import { queryKeys } from '../api/queryKeys'
import { QueryFeedback } from './QueryFeedback'
import {
  X,
  CheckCircle,

  XCircle,
  Info,

  FileText,
  StopCircle,
  Archive,
  Lock,
  Upload,
  Play,
  Zap,
  Terminal,
} from 'lucide-react'
import { backupsApi } from '../api'

const STEP_META: Record<string, { label: string; icon: React.ElementType }> = {
  start: { label: 'Start', icon: Play },
  validation: { label: 'Validation', icon: FileText },
  stop_containers: { label: 'Stop Containers', icon: StopCircle },
  pre_hook: { label: 'Pre-Hook', icon: Terminal },
  archive: { label: 'Archive', icon: Archive },
  encrypt: { label: 'Encrypt', icon: Lock },
  post_hook: { label: 'Post-Hook', icon: Terminal },
  restart: { label: 'Restart Containers', icon: Play },
  upload: { label: 'Remote Sync', icon: Upload },
  complete: { label: 'Complete', icon: CheckCircle },
  error: { label: 'Error', icon: XCircle },
  cleanup: { label: 'Cleanup', icon: Zap },
}

const LEVEL_STYLES: Record<string, { dot: string; text: string }> = {
  debug: { dot: 'bg-dark-500', text: 'text-dark-400' },
  info: { dot: 'bg-primary-400', text: 'text-dark-200' },
  warning: { dot: 'bg-amber-400', text: 'text-amber-300' },
  error: { dot: 'bg-red-400', text: 'text-red-300' },
}

interface Props {
  backupId: number
  onClose: () => void
  inline?: boolean
}

export default function BackupLogViewer({ backupId, onClose, inline = false }: Props) {
  const query = useQuery({ queryKey: queryKeys.backups.logs(backupId), queryFn: () => backupsApi.getLogs(backupId).then(response => response.data), retry: false })
  const { data: logs = [] } = query

  const formatTime = (iso: string) => {
    const d = new Date(iso)
    return d.toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit', second: '2-digit' })
  }

  const content = (
      <div className="bg-dark-800 border border-dark-600 rounded-xl shadow-2xl w-full max-w-2xl max-h-[80vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-dark-600">
          <h2 className="text-lg font-semibold text-white flex items-center gap-2">
            <FileText size={20} className="text-primary-400" />
            Backup #{backupId} — Job Log
          </h2>
          <button
            onClick={onClose}
            aria-label="Close job log"
            className="p-1.5 rounded-lg hover:bg-dark-600 text-dark-400 hover:text-white transition-colors"
          >
            <X size={18} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4">
          <QueryFeedback name="Run logs" query={query} />
          {query.isSuccess && logs.length === 0 && (
            <div className="flex items-center justify-center py-12 text-dark-500">
              <Info size={20} className="mr-2" />
              No log entries for this backup.
            </div>
          )}

          {logs.length > 0 && (
            <div className="relative">
              {/* Timeline line */}
              <div className="absolute left-[15px] top-2 bottom-2 w-px bg-dark-600" />

              <div className="space-y-1">
                {logs.map((log) => {
                  const meta = STEP_META[log.step] || { label: log.step, icon: Info }
                  const Icon = meta.icon
                  const style = LEVEL_STYLES[log.level] || LEVEL_STYLES.info

                  return (
                    <div key={log.id} className="relative flex items-start gap-3 pl-1 group">
                      {/* Timeline dot */}
                      <div className={`relative z-10 mt-1.5 w-[10px] h-[10px] rounded-full ring-2 ring-dark-800 ${style.dot} flex-shrink-0`} />

                      <div className="flex-1 min-w-0 py-1">
                        <div className="flex items-center gap-2 flex-wrap">
                          <Icon size={14} className="text-dark-400 flex-shrink-0" />
                          <span className="text-xs font-medium text-dark-400 uppercase tracking-wide">
                            {meta.label}
                          </span>
                          <span className="text-xs text-dark-500 ml-auto flex-shrink-0">
                            {formatTime(log.created_at)}
                          </span>
                        </div>
                        <p className={`text-sm mt-0.5 ${style.text}`}>
                          {log.message}
                        </p>
                        {log.details && (
                          <pre className="mt-1 text-xs text-dark-500 bg-dark-900/50 rounded px-2 py-1 overflow-x-auto">
                            {JSON.stringify(log.details, null, 2)}
                          </pre>
                        )}
                      </div>
                    </div>
                  )
                })}
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-dark-600 flex justify-end">
          <button
            onClick={onClose}
            className="px-4 py-2 text-sm rounded-lg bg-dark-700 hover:bg-dark-600 text-dark-300 hover:text-white transition-colors"
          >
            Close
          </button>
        </div>
      </div>
  )
  return inline ? content : <Modal title={`Backup ${backupId} — Job Log`} onClose={onClose}>{content}</Modal>
}
import { Modal } from './ui/dialog'
