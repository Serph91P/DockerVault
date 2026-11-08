import { afterEach, expect, it, vi } from 'vitest'
import { QueryObserver } from '@tanstack/react-query'
import { queryClient, clearSession } from '../session'
import { useWebSocketStore } from '../websocket'
import { startQueryFreshness } from '../freshness'
import { queryKeys } from '../../api/queryKeys'

afterEach(() => { clearSession(); vi.useRealTimers(); vi.unstubAllGlobals() })
it('reconnect invalidates real active HTTP readers without requiring another mutation', async () => {
  let revision = 1
  const observer = new QueryObserver(queryClient, { queryKey: queryKeys.backups.list({ limit: 10 }), queryFn: async () => revision, staleTime: Infinity })
  const unsubscribe = observer.subscribe(() => {})
  await observer.refetch()
  const stop = startQueryFreshness()
  revision = 2
  useWebSocketStore.setState({ connected: true })
  await vi.waitFor(() => expect(observer.getCurrentResult().data).toBe(2))
  stop(); unsubscribe()
})
it('fallback is bounded, skips active2s storage jobs, stops hidden and revokes on logout', async () => {
  vi.useFakeTimers()
  useWebSocketStore.setState({ connected: false })
  let reads = 0, jobReads = 0
  const observer = new QueryObserver(queryClient, { queryKey: queryKeys.backups.list(), queryFn: async () => ++reads, staleTime: Infinity })
  const job = new QueryObserver(queryClient, { queryKey: queryKeys.storageOperations('local', 1), queryFn: async () => { jobReads++; return [{ state: 'running' }] }, refetchInterval: 2000 })
  const unsubscribe = observer.subscribe(() => {}), unjob = job.subscribe(() => {})
  await observer.refetch(); await job.refetch()
  const stop = startQueryFreshness()
  const initial = reads, initialJobs = jobReads
  await vi.advanceTimersByTimeAsync(9999)
  expect(reads).toBe(initial)
  await vi.advanceTimersByTimeAsync(1)
  expect(reads).toBe(initial + 1)
  expect(jobReads).toBeLessThanOrEqual(initialJobs + 5)
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' })
  document.dispatchEvent(new Event('visibilitychange'))
  await vi.advanceTimersByTimeAsync(30000)
  expect(reads).toBe(initial + 1)
  clearSession()
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' })
  document.dispatchEvent(new Event('visibilitychange'))
  await vi.advanceTimersByTimeAsync(30000)
  expect(reads).toBe(initial + 1)
  stop(); unsubscribe(); unjob()
})

it.each(['backup_completed', 'operation_update'])('overview summary refreshes after %s without a socket business-result claim', async kind => {
  vi.useFakeTimers()
  const sockets: { onmessage: ((event: { data: string }) => void) | null }[] = []
  vi.stubGlobal('WebSocket', class {
    static OPEN = 1
    static CONNECTING = 0
    readyState = 0
    onmessage = null
    constructor() { sockets.push(this) }
    close() {}
  })
  let revision = 1
  const observer = new QueryObserver(queryClient, { queryKey: queryKeys.overview.summary(5, 1), queryFn: async () => revision, staleTime: Infinity })
  const unsubscribe = observer.subscribe(() => {})
  await observer.refetch()
  const stop = startQueryFreshness()
  useWebSocketStore.getState().connect()
  revision = 2
  sockets[0]?.onmessage?.({ data: JSON.stringify({ type: kind, backup_id: 1, operation_id: 'synthetic' }) })
  await vi.advanceTimersByTimeAsync(150)
  expect(observer.getCurrentResult().data).toBe(2)
  stop(); unsubscribe()
})

it('fallback does not invalidate an active detail that already owns the two-second poll', async () => {
  vi.useFakeTimers()
  useWebSocketStore.setState({ connected: false })
  const key = ['storage-operations', 'detail', 'synthetic', 'local', 1]
  const observer = new QueryObserver(queryClient, { queryKey: key, queryFn: async () => ({ state: 'running' }), refetchInterval: 2000 })
  const unsubscribe = observer.subscribe(() => {})
  await observer.refetch()
  const invalidations: unknown[] = []
  const uncache = queryClient.getQueryCache().subscribe(event => { if (event.type === 'updated' && event.action.type === 'invalidate' && event.query.queryKey === key) invalidations.push(event) })
  const stop = startQueryFreshness()
  await vi.advanceTimersByTimeAsync(10000)
  expect(invalidations).toHaveLength(0)
  stop(); uncache(); unsubscribe()
})

it('coalesced backup and operation hints both reach authoritative HTTP readers', async () => {
  vi.useFakeTimers()
  const sockets: { onmessage: ((event: { data: string }) => void) | null }[] = []
  vi.stubGlobal('WebSocket', class {
    static OPEN = 1
    static CONNECTING = 0
    readyState = 0
    onmessage = null
    constructor() { sockets.push(this) }
    close() {}
  })
  let revision = 1
  const runs = new QueryObserver(queryClient, { queryKey: queryKeys.backups.list(), queryFn: async () => revision, staleTime: Infinity })
  const jobs = new QueryObserver(queryClient, { queryKey: queryKeys.storageOperations('local', 1), queryFn: async () => revision, staleTime: Infinity })
  const unruns = runs.subscribe(() => {}), unjobs = jobs.subscribe(() => {})
  await runs.refetch(); await jobs.refetch()
  const stop = startQueryFreshness()
  useWebSocketStore.getState().connect()
  let release!: (value: number) => void
  let reads = 0
  const raced = new QueryObserver(queryClient, { queryKey: queryKeys.targets.list(), queryFn: () => ++reads === 1 ? new Promise<number>(resolve => { release = resolve }) : Promise.resolve(2), staleTime: Infinity })
  const unraced = raced.subscribe(() => {})
  revision = 2
  for (const type of ['backup_completed', 'operation_update', 'backup_failed']) sockets[0]?.onmessage?.({ data: JSON.stringify({ type, backup_id: 1, operation_id: 'synthetic' }) })
  await vi.advanceTimersByTimeAsync(150)
  expect(runs.getCurrentResult().data).toBe(2)
  expect(jobs.getCurrentResult().data).toBe(2)
  release(1)
  await vi.advanceTimersByTimeAsync(0)
  expect(raced.getCurrentResult().data).toBe(2)
  stop(); unruns(); unjobs(); unraced()
})
