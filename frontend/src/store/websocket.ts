import { create } from 'zustand'
import { Backup } from '../api'


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
let socketGeneration = 0
let expireSession: () => void = () => undefined
export function onSocketSessionExpired(handler: () => void) { expireSession = handler }
let queryHint: (type: string) => void = () => undefined
export function onQueryHint(handler: (type: string) => void) {
  queryHint = handler
  return () => { if (queryHint === handler) queryHint = () => undefined }
}

export const useWebSocketStore = create<WebSocketStore>((set, get) => ({
  connected: false,
  backupProgress: new Map(),
  recentBackups: [],

  connect: () => {
    if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return
    if (typeof window === 'undefined' || !window.location) return

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = window.location.host

    // The session_token cookie is httpOnly, so it cannot be read from JS.
    // Instead, the browser automatically sends cookies on same-origin
    // WebSocket connections and the backend reads the cookie directly.
    ws = new WebSocket(`${protocol}//${host}/ws/updates`)
    const socket = ws
    const generation = ++socketGeneration
    const current = () => generation === socketGeneration && ws === socket

    ws.onopen = () => {
      if (!current()) return
      console.log('WebSocket connected')
      set({ connected: true })
      queryHint('reconnect')
    }

    ws.onclose = (event) => {
      if (!current()) return
      ws = null
      console.log('WebSocket disconnected')
      set({ connected: false })

      // 4001 = authentication failure — do not reconnect
      if (event?.code === 4001) {
        console.warn('WebSocket auth failed, not reconnecting')
        expireSession()
        return
      }
      
      // Reconnect after 5 seconds
      reconnectTimeout = setTimeout(() => {
        get().connect()
      }, 5000)
    }

    ws.onerror = (error) => {
      if (!current()) return
      console.error('WebSocket error:', error)
    }

    ws.onmessage = (event) => {
      if (!current()) return
      try {
        const data = JSON.parse(event.data)
        handleMessage(data, set, get)
      } catch (error) {
        console.error('Failed to parse WebSocket message:', error)
      }
    }
  },

  disconnect: () => {
    ++socketGeneration
    if (reconnectTimeout) {
      clearTimeout(reconnectTimeout)
    }
    if (ws) {
      ws.close()
      ws = null
    }
    set({ connected: false, backupProgress: new Map(), recentBackups: [] })
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
    case 'operation_update':
      if (typeof data.operation_id === 'string') queryHint('operation_update')
      break
    case 'connected':
      console.log('WebSocket handshake complete')
      break

    case 'backup_progress':
      if (!Number.isSafeInteger(data.backup_id) || typeof data.progress !== 'number' || !Number.isFinite(data.progress)) break
      get().updateProgress({
        backup_id: data.backup_id as number,
        progress: data.progress as number,
        message: 'Work reported by live channel; awaiting backend confirmation',
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

        // A socket event is only a hint. The authorized HTTP result owns completion.
        queryHint(data.type)
      }
      break

    default:
      console.log('Unknown WebSocket message type:', data.type)
  }
}

export function cleanupWebSocket() {
  useWebSocketStore.getState().disconnect()
}
