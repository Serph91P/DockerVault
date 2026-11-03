import { afterEach, expect, it, vi } from 'vitest'
import { useWebSocketStore, cleanupWebSocket } from '../websocket'
import { useNotificationStore } from '../notifications'
import { clearSession, queryClient } from '../session'
import { useAuthStore } from '../auth'
import api from '../../api'
import { server } from '../../test/mocks/server'
import { http, HttpResponse } from 'msw'

class Socket {
  static OPEN = 1
  static CONNECTING = 0
  readyState = 0
  onopen: (() => void) | null = null
  onclose: ((event: { code: number }) => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  close() { this.onclose?.({ code: 1000 }) }
}

afterEach(() => {
  cleanupWebSocket()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  clearSession()
})

it('invalidates even retained socket callbacks and cancels intentional-close reconnects', () => {
  vi.useFakeTimers()
  const sockets: Socket[] = []
  vi.stubGlobal('WebSocket', class extends Socket { constructor() { super(); sockets.push(this) } })
  useWebSocketStore.getState().connect()
  const old = sockets[0]
  const open = old.onopen
  const message = old.onmessage
  clearSession()
  open?.()
  message?.({ data: JSON.stringify({ type: 'backup_progress', backup_id: 1, progress: 50, message: 'old' }) })
  message?.({ data: JSON.stringify({ type: 'backup_completed', backup_id: 1 }) })
  vi.advanceTimersByTime(10000)
  expect(sockets).toHaveLength(1)
  expect(useWebSocketStore.getState().connected).toBe(false)
  expect(useWebSocketStore.getState().backupProgress.size).toBe(0)
  expect(useNotificationStore.getState().notifications).toHaveLength(0)
})

it('does not connect merely because the module was imported', () => {
  vi.useFakeTimers()
  let connections = 0
  vi.stubGlobal('WebSocket', class extends Socket { constructor() { super(); connections++ } })
  vi.advanceTimersByTime(2000)
  expect(connections).toBe(0)
})

it('clears query and mutation caches and old browser presentation on a boundary', async () => {
  queryClient.setQueryData(['protected'], { prior: true })
  await queryClient.getMutationCache().build(queryClient, { mutationFn: async () => ({ prior: true }) }).execute(undefined)
  localStorage.setItem('dockervault-wizard-draft', JSON.stringify({ data: { preCommand: 'sensitive hook' } }))
  localStorage.setItem('storage-test-results', '{}')
  clearSession()
  expect(queryClient.getQueryCache().getAll()).toHaveLength(0)
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
  expect(localStorage.getItem('dockervault-wizard-draft')).toBeNull()
  expect(localStorage.getItem('storage-test-results')).toBeNull()
})

it('does not run old mutation presentation callbacks after a boundary', async () => {
  let release!: () => void
  const gate = new Promise<void>((resolve) => { release = resolve })
  const mutation = queryClient.getMutationCache().build(queryClient, {
    mutationFn: async () => { await gate; throw new Error('Old operation failed') },
    onError: () => useNotificationStore.getState().addNotification({ type: 'error', title: 'Prior operation', message: 'Old failure' }),
  })
  const result = mutation.execute(undefined).catch(() => undefined)
  await Promise.resolve()
  clearSession()
  release()
  await result
  expect(useNotificationStore.getState().notifications).toHaveLength(0)
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
})

it('does not repopulate or overwrite the next session from a late protected query', async () => {
  let release!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  const old = queryClient.fetchQuery({ queryKey: ['protected'], queryFn: async () => { await gate; return { owner: 'prior' } } }).catch(() => undefined)
  clearSession()
  queryClient.setQueryData(['protected'], { owner: 'next' })
  release()
  await old
  expect(queryClient.getQueryData(['protected'])).toEqual({ owner: 'next' })
})

it('preserves only same-operator nonsensitive drafts on reload, never after logout', () => {
  localStorage.setItem('dockervault-wizard-draft', JSON.stringify({ ownerId: 701, data: { targetName: 'Useful draft' } }))
  clearSession(701)
  expect(localStorage.getItem('dockervault-wizard-draft')).not.toBeNull()
  clearSession(702)
  expect(localStorage.getItem('dockervault-wizard-draft')).toBeNull()
  localStorage.setItem('dockervault-wizard-draft', '{broken')
  localStorage.setItem('dockervault_notifications', 'old')
  clearSession()
  expect(localStorage.getItem('dockervault-wizard-draft')).toBeNull()
  expect(localStorage.getItem('dockervault_notifications')).toBeNull()
})

it('cancels a request started in the same tick as the session boundary', async () => {
  server.use(http.get('/api/v1/queued-probe', () => HttpResponse.json({ protected: true })))
  const request = api.get('/queued-probe').then(() => 'completed', () => 'cancelled')
  clearSession()
  expect(await request).toBe('cancelled')
})

it('does not start an old queued mutation after the session boundary', async () => {
  let sent = false
  const mutation = queryClient.getMutationCache().build(queryClient, { mutationFn: async () => { sent = true; return 'old' } })
  const result = mutation.execute(undefined).catch(() => undefined)
  clearSession()
  await result
  expect(sent).toBe(false)
})

it('expires protected state when the server revokes the socket session', () => {
  const sockets: Socket[] = []
  vi.stubGlobal('WebSocket', class extends Socket { constructor() { super(); sockets.push(this) } })
  useAuthStore.setState({ isAuthenticated: true })
  queryClient.setQueryData(['prior'], 'protected')
  useWebSocketStore.getState().connect()
  sockets[0].onclose?.({ code: 4001 })
  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(queryClient.getQueryCache().getAll()).toHaveLength(0)
})

it('keeps notifications in this session only, not browser persistence', () => {
  useNotificationStore.getState().addNotification({ type: 'info', title: 'Session', message: 'Protected details' })
  expect(useNotificationStore.getState().notifications).toHaveLength(1)
  expect(localStorage.getItem('dockervault_notifications')).toBeNull()
})
