import { create } from 'zustand'
import { Backup } from '../api'
import { useNotificationStore } from './notifications'

interface BackupProgress {
  backup_id: number
  progress: number
  message: string
}

interface WebSocketStore {
  connected: boolean
  backupProgress: Map<number, BackupProgress>
  recentBackups: Backup[]
  connect: () => void
  disconnect: () => void
  updateProgress: (progress: BackupProgress) => void
  addRecentBackup: (backup: Backup) => void
}

let ws: WebSocket | null = null
let reconnectTimeout: ReturnType<typeof setTimeout> | null = null

export const useWebSocketStore = create<WebSocketStore>((set, get) => ({
  connected: false,
  backupProgress: new Map(),
  recentBackups: [],

  connect: () => {
    if (ws?.readyState === WebSocket.OPEN) return

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = window.location.host

    // The session_token cookie is httpOnly, so it cannot be read from JS.
    // Instead, the browser automatically sends cookies on same-origin
    // WebSocket connections and the backend reads the cookie directly.
    ws = new WebSocket(`${protocol}//${host}/ws/updates`)

    ws.onopen = () => {
      console.log('WebSocket connected')
      set({ connected: true })
    }

    ws.onclose = (event) => {
      console.log('WebSocket disconnected')
      set({ connected: false })

      // 4001 = authentication failure — do not reconnect
      if (event?.code === 4001) {
        console.warn('WebSocket auth failed, not reconnecting')
        return
      }
      
      // Reconnect after 5 seconds
      reconnectTimeout = setTimeout(() => {
        get().connect()
      }, 5000)
    }

    ws.onerror = (error) => {
      console.error('WebSocket error:', error)
    }

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        handleMessage(data, set, get)
      } catch (error) {
        console.error('Failed to parse WebSocket message:', error)
      }
    }
  },

  disconnect: () => {
    if (reconnectTimeout) {
      clearTimeout(reconnectTimeout)
    }
    if (ws) {
      ws.close()
      ws = null
    }
    set({ connected: false })
  },

  updateProgress: (progress) => {
    set((state) => {
      const newProgress = new Map(state.backupProgress)
      newProgress.set(progress.backup_id, progress)
      return { backupProgress: newProgress }
    })
  },

  addRecentBackup: (backup) => {
    set((state) => ({
      recentBackups: [backup, ...state.recentBackups.slice(0, 9)],
    }))
  },
}))

function handleMessage(
  data: { type: string; [key: string]: unknown },
  set: (state: Partial<WebSocketStore>) => void,
  get: () => WebSocketStore
) {
  switch (data.type) {
    case 'connected':
      console.log('WebSocket handshake complete')
      break

    case 'backup_progress':
      get().updateProgress({
        backup_id: data.backup_id as number,
        progress: data.progress as number,
        message: data.message as string,
      })
      break

    case 'backup_completed':
    case 'backup_failed':
      // Remove from progress tracking
      {
        const currentState = get()
        const newProgress = new Map(currentState.backupProgress)
        newProgress.delete(data.backup_id as number)
        set({ backupProgress: newProgress })

        // Add persistent notification
        const addNotification = useNotificationStore.getState().addNotification
        if (data.type === 'backup_completed') {
          addNotification({
            type: 'success',
            title: 'Backup completed',
            message: `Backup #${data.backup_id} finished successfully`,
          })
        } else {
          addNotification({
            type: 'error',
            title: 'Backup failed',
            message: `Backup #${data.backup_id} failed. Check logs for details.`,
          })
        }
      }
      break

    default:
      console.log('Unknown WebSocket message type:', data.type)
  }
}

// Auto-connect on import
if (typeof window !== 'undefined') {
  setTimeout(() => {
    useWebSocketStore.getState().connect()
  }, 1000)
}
