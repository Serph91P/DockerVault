import { create } from 'zustand'

export type NotificationType = 'success' | 'error' | 'warning' | 'info'

export interface Notification {
  id: string
  type: NotificationType
  title: string
  message: string
  timestamp: number
  read: boolean
}

interface NotificationStore {
  notifications: Notification[]
  addNotification: (notification: Omit<Notification, 'id' | 'timestamp' | 'read'>) => void
  markRead: (id: string) => void
  markAllRead: () => void
  removeNotification: (id: string) => void
  clearAll: () => void
  unreadCount: () => number
}

const MAX_NOTIFICATIONS = 50

function loadNotifications(): Notification[] {
  try {
    const stored = localStorage.getItem('dockervault_notifications')
    if (stored) {
      const parsed = JSON.parse(stored) as Notification[]
      // Keep only last MAX_NOTIFICATIONS and items from last 7 days
      const cutoff = Date.now() - 7 * 24 * 60 * 60 * 1000
      return parsed.filter((n) => n.timestamp > cutoff).slice(0, MAX_NOTIFICATIONS)
    }
  } catch {
    // Ignore parse errors
  }
  return []
}

function saveNotifications(notifications: Notification[]) {
  try {
    localStorage.setItem('dockervault_notifications', JSON.stringify(notifications))
  } catch {
    // Ignore storage errors
  }
}

export const useNotificationStore = create<NotificationStore>((set, get) => ({
  notifications: loadNotifications(),

  addNotification: (notification) => {
    const newNotification: Notification = {
      ...notification,
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      timestamp: Date.now(),
      read: false,
    }
    set((state) => {
      const updated = [newNotification, ...state.notifications].slice(0, MAX_NOTIFICATIONS)
      saveNotifications(updated)
      return { notifications: updated }
    })
  },

  markRead: (id) => {
    set((state) => {
      const updated = state.notifications.map((n) =>
        n.id === id ? { ...n, read: true } : n
      )
      saveNotifications(updated)
      return { notifications: updated }
    })
  },

  markAllRead: () => {
    set((state) => {
      const updated = state.notifications.map((n) => ({ ...n, read: true }))
      saveNotifications(updated)
      return { notifications: updated }
    })
  },

  removeNotification: (id) => {
    set((state) => {
      const updated = state.notifications.filter((n) => n.id !== id)
      saveNotifications(updated)
      return { notifications: updated }
    })
  },

  clearAll: () => {
    saveNotifications([])
    set({ notifications: [] })
  },

  unreadCount: () => {
    return get().notifications.filter((n) => !n.read).length
  },
}))
