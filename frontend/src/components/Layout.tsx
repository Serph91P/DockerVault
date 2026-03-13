import { Link, Outlet, useLocation } from 'react-router-dom'
import { clsx } from 'clsx'
import {
  LayoutDashboard,
  Archive,
  Clock,
  Trash2,
  Cloud,
  Settings,
  Wifi,
  WifiOff,
  LogOut,
  User,
  Keyboard,
} from 'lucide-react'
import { useState } from 'react'
import { useWebSocketStore } from '../store/websocket'
import { useAuthStore } from '../store/auth'
import { useKeyboardShortcuts } from './KeyboardShortcuts'
import { NotificationBadge, NotificationCenter } from './NotificationCenter'
import toast from 'react-hot-toast'

const navigation = [
  { name: 'Dashboard', href: '/', icon: LayoutDashboard },
  { name: 'Backups', href: '/backups', icon: Archive },
  { name: 'Schedules', href: '/schedules', icon: Clock },
  { name: 'Retention', href: '/retention', icon: Trash2 },
  { name: 'Remote Storage', href: '/storage', icon: Cloud },
  { name: 'Settings', href: '/settings', icon: Settings },
]

export default function Layout() {
  const location = useLocation()
  const connected = useWebSocketStore((state) => state.connected)
  const { user, logout } = useAuthStore()
  const { showHelp, setShowHelp, ShortcutHelp } = useKeyboardShortcuts()
  const [showNotifications, setShowNotifications] = useState(false)
  
  const handleLogout = async () => {
    await logout()
    toast.success('Logged out successfully')
  }

  return (
    <div className="flex h-screen bg-dark-950">
      {/* Sidebar */}
      <div className="w-64 bg-dark-900 border-r border-dark-700 flex flex-col">
        {/* Logo */}
        <div className="h-16 flex items-center px-6 border-b border-dark-700">
          <div className="flex items-center gap-3">
            <div className="w-8 h-8 bg-primary-500 rounded-lg flex items-center justify-center">
              <Archive className="w-5 h-5 text-white" />
            </div>
            <span className="text-lg font-semibold text-dark-100">
              DockerVault
            </span>
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 px-3 py-4 space-y-1 overflow-y-auto">
          {navigation.map((item) => {
            const isActive = location.pathname === item.href
            return (
              <Link
                key={item.name}
                to={item.href}
                className={clsx(
                  'flex items-center gap-3 px-3 py-2 rounded-lg text-sm font-medium transition-colors',
                  isActive
                    ? 'bg-primary-500/10 text-primary-400'
                    : 'text-dark-400 hover:bg-dark-800 hover:text-dark-100'
                )}
              >
                <item.icon className="w-5 h-5" />
                {item.name}
              </Link>
            )
          })}
        </nav>

        {/* Connection Status & User */}
        <div className="px-4 py-3 border-t border-dark-700 space-y-3">
          {/* User Info */}
          {user && (
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2 text-sm">
                <User className="w-4 h-4 text-dark-400" />
                <span className="text-dark-300">{user.username}</span>
              </div>
              <button
                onClick={handleLogout}
                className="p-1.5 text-dark-400 hover:text-red-400 hover:bg-dark-800 rounded-lg transition-colors"
                title="Logout"
              >
                <LogOut className="w-4 h-4" />
              </button>
            </div>
          )}
          
          {/* Connection Status */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm">
              {connected ? (
                <>
                  <Wifi className="w-4 h-4 text-green-500" />
                  <span className="text-dark-400">Connected</span>
                </>
              ) : (
                <>
                  <WifiOff className="w-4 h-4 text-red-500" />
                  <span className="text-dark-400">Disconnected</span>
                </>
              )}
            </div>
            <div className="relative">
              <NotificationBadge onClick={() => setShowNotifications(!showNotifications)} />
              <NotificationCenter
                isOpen={showNotifications}
                onClose={() => setShowNotifications(false)}
              />
            </div>
          </div>

          {/* Keyboard Shortcuts Hint */}
          <button
            onClick={() => setShowHelp(true)}
            className="flex items-center gap-2 text-xs text-dark-500 hover:text-dark-300 transition-colors"
            title="Keyboard Shortcuts"
          >
            <Keyboard className="w-3.5 h-3.5" />
            <span>Press <kbd className="px-1 py-0.5 bg-dark-700 border border-dark-600 rounded text-[10px] font-mono">?</kbd> for shortcuts</span>
          </button>
        </div>
      </div>

      {/* Main Content */}
      <div className="flex-1 flex flex-col overflow-hidden">
        <main className="flex-1 overflow-y-auto p-6">
          <Outlet />
        </main>
      </div>

      {/* Keyboard Shortcut Help Dialog */}
      <ShortcutHelp isOpen={showHelp} onClose={() => setShowHelp(false)} />
    </div>
  )
}
