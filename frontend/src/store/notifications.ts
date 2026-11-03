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

// Notifications are private to this page session. Remove legacy unscoped data.
try { localStorage.removeItem('dockervault_notifications') } catch { /* Storage unavailable. */ }

export const useNotificationStore = create<NotificationStore>((set, get) => ({
  notifications: [],

  addNotification: (notification) => {
    const newNotification: Notification = {
      ...notification,
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      timestamp: Date.now(),
      read: false,
    }
    set((state) => {
      const updated = [newNotification, ...state.notifications].slice(0, MAX_NOTIFICATIONS)
      return { notifications: updated }
    })
  },

  markRead: (id) => {
    set((state) => {
      const updated = state.notifications.map((n) =>
        n.id === id ? { ...n, read: true } : n
      )
      return { notifications: updated }
    })
  },

  markAllRead: () => {
    set((state) => {
      const updated = state.notifications.map((n) => ({ ...n, read: true }))
      return { notifications: updated }
    })
  },

  removeNotification: (id) => {
    set((state) => {
      const updated = state.notifications.filter((n) => n.id !== id)
      return { notifications: updated }
    })
  },

  clearAll: () => {

    set({ notifications: [] })
  },

  unreadCount: () => {
    return get().notifications.filter((n) => !n.read).length
  },
}))
