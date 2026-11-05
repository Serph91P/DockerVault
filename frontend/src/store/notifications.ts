import { create } from 'zustand'

export type NotificationType = 'success' | 'error' | 'warning' | 'info'
export type NotificationTarget = { kind: 'plan' | 'run' | 'archive'; id: number } | { kind: 'operation'; id: string; storageId: 'local' | number }
export const notificationResults = {
  'plan-saved': { type: 'info', title: 'Protection plan saved', message: 'Backend confirmed the saved plan. This is not a run or archive result.' },
  'run-completed': { type: 'success', title: 'Run completed', message: 'Backend confirmed completion. Open the run for copy outcomes.' },
  'run-partial': { type: 'warning', title: 'Run partially completed', message: 'Some copy outcomes need attention. Open the run.' },
  'run-failed': { type: 'error', title: 'Run failed', message: 'Backend confirmed failure. Open the run logs.' },
  'run-cancelled': { type: 'warning', title: 'Run cancelled', message: 'Backend confirmed cancellation. Open the run.' },
  'operation-completed': { type: 'success', title: 'Storage operation completed', message: 'Backend confirmed the final result. Open the operation for item outcomes.' },
  'operation-partial': { type: 'warning', title: 'Storage operation needs attention', message: 'Open the operation for failed, skipped or uncertain item outcomes.' },
  'operation-failed': { type: 'error', title: 'Storage operation failed', message: 'Backend confirmed failure. Open the operation.' },
} as const
export type NotificationCode = keyof typeof notificationResults
export interface Notification {
  id: string; type: NotificationType; title: string; message: string; timestamp: number; read: boolean
  code?: NotificationCode; target?: NotificationTarget; dedupeKey?: string
  revision?: number; counts?: { succeeded: number; failed: number; total: number }
  phase?: 'accepted' | 'running' | 'completed' | 'partial' | 'failed' | 'cancelled' | 'unknown'; startedAt?: number
  dismissed?: boolean
}
interface NotificationStore {
  notifications: Notification[]
  addNotification: (notification: Omit<Notification, 'id' | 'timestamp' | 'read'>) => void
  markRead: (id: string) => void; markAllRead: () => void; removeNotification: (id: string) => void
  clearAll: () => void; unreadCount: () => number
}
const KEY = 'dockervault-notifications-v1'
const TTL = 7 * 86400000
let owner: number | null = null
let dismissed: Notification[] = []
const positiveId = (id: unknown): id is number => typeof id === 'number' && Number.isSafeInteger(id) && id > 0
const uuid = (id: unknown): id is string => typeof id === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)
export function notificationHref(target: NotificationTarget): string | null {
  if (!target || typeof target !== 'object') return null
  if (target.kind === 'operation') return uuid(target.id) && (target.storageId === 'local' || positiveId(target.storageId)) ? `/storage?operation=${target.id}&storage=${target.storageId}` : null
  if (!positiveId(target.id)) return null
  return target.kind === 'plan' ? `/protection?plan=${target.id}` : target.kind === 'run' ? `/history?run=${target.id}` : target.kind === 'archive' ? `/history?archive=${target.id}` : null
}
function safeResult(value: unknown): Notification | null {
  if (!value || typeof value !== 'object') return null
  const n = value as Notification
  if (!n.code || !Object.prototype.hasOwnProperty.call(notificationResults, n.code) || !n.target || !notificationHref(n.target)) return null
  if (!Number.isFinite(n.timestamp) || n.timestamp < Date.now() - TTL || n.timestamp > Date.now() || typeof n.read !== 'boolean') return null
  const target: NotificationTarget = n.target.kind === 'operation' ? { kind: 'operation', id: n.target.id, storageId: n.target.storageId } : { kind: n.target.kind, id: n.target.id }
  const revision = typeof n.revision === 'number' && Number.isSafeInteger(n.revision) && n.revision >= 0 ? n.revision : 0
  const dedupeKey = `${target.kind}:${target.id}:${target.kind === 'operation' ? target.storageId : ''}:${n.code}:${revision}`
  const counts = n.counts && [n.counts.succeeded, n.counts.failed, n.counts.total].every(v => Number.isSafeInteger(v) && v >= 0) && n.counts.succeeded + n.counts.failed <= n.counts.total ? { succeeded: n.counts.succeeded, failed: n.counts.failed, total: n.counts.total } : undefined
  return { ...notificationResults[n.code], code: n.code, target, revision, counts, dedupeKey, id: dedupeKey, timestamp: n.timestamp, read: n.read, dismissed: n.dismissed === true }
}
function removeRetention() { try { sessionStorage.removeItem(KEY) } catch { /* Storage unavailable. */ } }
function persist() {
  if (owner === null) return
  const items = [...useNotificationStore.getState().notifications, ...dismissed].map(safeResult).filter((n): n is Notification => n !== null).sort((a, b) => b.timestamp - a.timestamp).slice(0, 50)
  if (items.length === 0) { removeRetention(); return }
  try { sessionStorage.setItem(KEY, JSON.stringify({ version: 1, owner, items })) } catch { /* Quota does not break in-memory controls. */ }
}
// Called only after the authenticated HTTP user has been verified.
export function bindNotificationOwner(id: number) {
  if (!positiveId(id)) { resetNotificationOwner(); return }
  if (owner === id) return
  let notifications: Notification[] = []
  try {
    const saved = JSON.parse(sessionStorage.getItem(KEY) ?? 'null')
    if (saved?.version === 1 && saved.owner === id && Array.isArray(saved.items)) notifications = saved.items.slice(0, 50).map(safeResult).filter((n: Notification | null): n is Notification => n !== null)
    else removeRetention()
  } catch { removeRetention() }
  owner = id
  const unique = [...new Map(notifications.map(n => [n.id, n])).values()]
  dismissed = unique.filter(n => n.dismissed)
  useNotificationStore.setState({ notifications: unique.filter(n => !n.dismissed) })
  persist()
}
export function resetNotificationOwner(preserveFor?: number) {
  owner = null
  dismissed = []
  useNotificationStore.setState({ notifications: [] })
  if (!positiveId(preserveFor)) removeRetention()
  else {
    try { if (JSON.parse(sessionStorage.getItem(KEY) ?? 'null')?.owner !== preserveFor) removeRetention() } catch { removeRetention() }
  }
}
try { localStorage.removeItem('dockervault_notifications') } catch { /* Legacy data is never read. */ }
export const useNotificationStore = create<NotificationStore>((set, get) => ({
  notifications: [],
  addNotification: input => {
    const raw = { ...input, id: crypto.randomUUID(), timestamp: Date.now(), read: false }
    const notification = input.code ? safeResult(raw) : raw
    if (!notification || (notification.dedupeKey && [...get().notifications, ...dismissed].some(n => n.dedupeKey === notification.dedupeKey))) return
    set({ notifications: [notification, ...get().notifications].slice(0, 50) }); persist()
  },
  markRead: id => { set({ notifications: get().notifications.map(n => n.id === id ? { ...n, read: true } : n) }); persist() },
  markAllRead: () => { set({ notifications: get().notifications.map(n => ({ ...n, read: true })) }); persist() },
  removeNotification: id => { dismissed = [...get().notifications.filter(n => n.id === id && n.code).map(n => ({ ...n, dismissed: true })), ...dismissed].slice(0, 50); set({ notifications: get().notifications.filter(n => n.id !== id) }); persist() },
  clearAll: () => { dismissed = [...get().notifications.filter(n => n.code).map(n => ({ ...n, dismissed: true })), ...dismissed].slice(0, 50); set({ notifications: [] }); persist() },
  unreadCount: () => get().notifications.filter(n => !n.read).length,
}))
