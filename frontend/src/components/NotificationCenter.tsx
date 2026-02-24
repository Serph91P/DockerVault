import { useRef, useEffect } from 'react'
import {
  Bell,
  X,
  CheckCheck,
  Trash2,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  Info,
} from 'lucide-react'
import { clsx } from 'clsx'
import { useNotificationStore, type NotificationType } from '../store/notifications'
import { formatDistanceToNowStrict } from 'date-fns'

const typeConfig: Record<
  NotificationType,
  { icon: typeof CheckCircle2; color: string; bg: string }
> = {
  success: { icon: CheckCircle2, color: 'text-green-400', bg: 'bg-green-500/10' },
  error: { icon: XCircle, color: 'text-red-400', bg: 'bg-red-500/10' },
  warning: { icon: AlertTriangle, color: 'text-yellow-400', bg: 'bg-yellow-500/10' },
  info: { icon: Info, color: 'text-blue-400', bg: 'bg-blue-500/10' },
}

interface NotificationCenterProps {
  isOpen: boolean
  onClose: () => void
}

export function NotificationBadge({ onClick }: { onClick: () => void }) {
  const unreadCount = useNotificationStore((s) => s.unreadCount())

  return (
    <button
      onClick={onClick}
      className="relative p-1.5 text-dark-400 hover:text-dark-200 hover:bg-dark-800 rounded-lg transition-colors"
      title="Notifications"
    >
      <Bell className="w-4 h-4" />
      {unreadCount > 0 && (
        <span className="absolute -top-0.5 -right-0.5 w-4 h-4 bg-red-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
          {unreadCount > 9 ? '9+' : unreadCount}
        </span>
      )}
    </button>
  )
}

export function NotificationCenter({ isOpen, onClose }: NotificationCenterProps) {
  const { notifications, markRead, markAllRead, removeNotification, clearAll } =
    useNotificationStore()
  const panelRef = useRef<HTMLDivElement>(null)

  // Close on outside click
  useEffect(() => {
    if (!isOpen) return
    const handleClick = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        onClose()
      }
    }
    // Delay to prevent the opening click from immediately closing
    const timer = setTimeout(() => {
      document.addEventListener('mousedown', handleClick)
    }, 0)
    return () => {
      clearTimeout(timer)
      document.removeEventListener('mousedown', handleClick)
    }
  }, [isOpen, onClose])

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', handleKey)
    return () => document.removeEventListener('keydown', handleKey)
  }, [isOpen, onClose])

  if (!isOpen) return null

  const unreadCount = notifications.filter((n) => !n.read).length

  return (
    <div
      ref={panelRef}
      className="absolute bottom-12 left-0 w-80 bg-dark-800 border border-dark-600 rounded-xl shadow-2xl z-50 flex flex-col max-h-[70vh] animate-fade-in"
    >
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-dark-700">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-semibold text-dark-100">Notifications</h3>
          {unreadCount > 0 && (
            <span className="px-1.5 py-0.5 bg-red-500/20 text-red-400 text-xs rounded-full font-medium">
              {unreadCount}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">
          {unreadCount > 0 && (
            <button
              onClick={markAllRead}
              className="p-1.5 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
              title="Mark all as read"
            >
              <CheckCheck className="w-3.5 h-3.5" />
            </button>
          )}
          {notifications.length > 0 && (
            <button
              onClick={clearAll}
              className="p-1.5 text-dark-400 hover:text-red-400 hover:bg-dark-700 rounded-lg transition-colors"
              title="Clear all"
            >
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          )}
          <button
            onClick={onClose}
            className="p-1.5 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Notification List */}
      <div className="flex-1 overflow-y-auto">
        {notifications.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-10 text-dark-500">
            <Bell className="w-8 h-8 mb-2 opacity-40" />
            <p className="text-sm">No notifications</p>
          </div>
        ) : (
          <div className="divide-y divide-dark-700/50">
            {notifications.map((notification) => {
              const config = typeConfig[notification.type]
              const Icon = config.icon
              return (
                <div
                  key={notification.id}
                  className={clsx(
                    'flex gap-3 px-4 py-3 hover:bg-dark-750 transition-colors cursor-pointer group',
                    !notification.read && 'bg-dark-750/50'
                  )}
                  onClick={() => markRead(notification.id)}
                >
                  <div
                    className={clsx(
                      'w-7 h-7 rounded-lg flex items-center justify-center flex-shrink-0 mt-0.5',
                      config.bg
                    )}
                  >
                    <Icon className={clsx('w-3.5 h-3.5', config.color)} />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-start justify-between gap-2">
                      <p
                        className={clsx(
                          'text-sm truncate',
                          notification.read ? 'text-dark-300' : 'text-dark-100 font-medium'
                        )}
                      >
                        {notification.title}
                      </p>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          removeNotification(notification.id)
                        }}
                        className="p-0.5 text-dark-600 hover:text-dark-300 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0"
                      >
                        <X className="w-3 h-3" />
                      </button>
                    </div>
                    <p className="text-xs text-dark-500 mt-0.5 line-clamp-2">
                      {notification.message}
                    </p>
                    <p className="text-[10px] text-dark-600 mt-1">
                      {formatDistanceToNowStrict(notification.timestamp, { addSuffix: true })}
                    </p>
                  </div>
                  {!notification.read && (
                    <div className="w-2 h-2 bg-primary-500 rounded-full flex-shrink-0 mt-2" />
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
