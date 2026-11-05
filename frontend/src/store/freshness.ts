import { queryClient, ownsSession, sessionGeneration, sessionSignal } from './session'
import { useWebSocketStore, onQueryHint } from './websocket'
import { recordRunResults, recordStorageResults } from './resultNotifications'
import type { Backup } from '../api'
import type { StorageOperation } from '../api/operations'
import type { Query } from '@tanstack/react-query'

const families = new Set(['targets', 'backups', 'storages', 'schedules', 'retention-policies', 'containers', 'volumes', 'stacks', 'system-info', 'settings', 'docker-health', 'encryption-status', 'ssh-keys', 'storage-files', 'storage-size', 'storage-operations'])
let activeStop: (() => void) | undefined

// One lifecycle owner for authorized readers; websocket never supplies business results.
export function startQueryFreshness() {
  activeStop?.()
  const owner = sessionGeneration(), signal = sessionSignal()
  let stopped = false, timer: ReturnType<typeof setTimeout> | undefined, eventTimer: ReturnType<typeof setTimeout> | undefined
  let failures = 0
  const pendingHints = new Set<string>()
  const visible = () => document.visibilityState !== 'hidden'
  const refresh = async (kind?: string) => {
    if (stopped || !ownsSession(owner) || !visible()) return
    const matches = (query: Query) => {
      const family = String(query.queryKey[0])
      if (!families.has(family) || (!kind && query.state.fetchStatus === 'fetching')) return false
      if (kind === 'operation_update') return ['storage-operations', 'storage-files', 'storage-size'].includes(family)
      if (kind?.startsWith('backup_')) return ['backups', 'targets', 'system-info', 'schedules'].includes(family)
      // StorageBrowser already polls active jobs every two seconds.
      const jobs = Array.isArray(query.state.data) ? query.state.data : [query.state.data]
      if (!kind && family === 'storage-operations' && jobs.some(job => job && ['queued', 'running'].includes(job.state))) return false
      return true
    }
    if (kind) await queryClient.cancelQueries({ predicate: query => query.isActive() && query.state.fetchStatus === 'fetching' && matches(query) })
    if (stopped || !ownsSession(owner) || !visible()) return
    await queryClient.invalidateQueries({ refetchType: 'active', predicate: matches })
    const failed = queryClient.getQueryCache().getAll().some(q => q.isActive() && q.state.status === 'error')
    failures = failed ? Math.min(failures + 1, 2) : 0
  }
  const schedule = () => {
    clearTimeout(timer)
    if (stopped || !ownsSession(owner) || !visible() || useWebSocketStore.getState().connected) return
    timer = setTimeout(async () => { await refresh(); schedule() }, Math.min(10000 * (failures + 1), 30000))
  }
  const unsubscribe = useWebSocketStore.subscribe((state, previous) => {
    if (state.connected === previous.connected) return
    clearTimeout(timer)
    if (state.connected) { failures = 0; void refresh('reconnect') }
    else schedule()
  })
  const unhint = onQueryHint(kind => {
    if (kind === 'reconnect' || stopped || !ownsSession(owner)) return
    pendingHints.add(kind.startsWith('backup_') ? 'backup_result' : kind)
    if (eventTimer) return
    eventTimer = setTimeout(() => { eventTimer = undefined; const hints = [...pendingHints]; pendingHints.clear(); for (const hint of hints) void refresh(hint) }, 150)
  })
  const uncache = queryClient.getQueryCache().subscribe(event => {
    if (stopped || !ownsSession(owner) || event.type !== 'updated' || event.action.type !== 'success') return
    const data = event.query.state.data
    if (event.query.queryKey[0] === 'backups') recordRunResults((Array.isArray(data) ? data : [data]).filter((item): item is Backup => Boolean(item && Number.isSafeInteger(item.id) && typeof item.status === 'string')))
    if (event.query.queryKey[0] === 'storage-operations') recordStorageResults((Array.isArray(data) ? data : [data]).filter((item): item is StorageOperation => Boolean(item && typeof item.id === 'string' && typeof item.state === 'string')))
  })
  const visibility = () => { if (visible()) void refresh('visible'); schedule() }
  const stop = () => {
    stopped = true; clearTimeout(timer); clearTimeout(eventTimer); pendingHints.clear(); unsubscribe(); unhint(); uncache()
    document.removeEventListener('visibilitychange', visibility); signal.removeEventListener('abort', stop)
    window.removeEventListener('focus', visibility)
    if (activeStop === stop) activeStop = undefined
  }
  activeStop = stop
  document.addEventListener('visibilitychange', visibility)
  window.addEventListener('focus', visibility)
  signal.addEventListener('abort', stop, { once: true })
  schedule()
  return stop
}
