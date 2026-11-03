import { afterEach, beforeEach, expect, it } from 'vitest'
import api from '../../api'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../auth'
import { useNotificationStore } from '../notifications'

const operator = {
  id: 701,
  username: 'synthetic-operator',
  is_admin: true,
  created_at: '2026-01-01T00:00:00Z',
  last_login: null,
}

beforeEach(() => {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    isLoading: true,
    setupRequired: false,
  })
  useNotificationStore.getState().clearAll()
  server.use(http.post('/api/v1/auth/logout', () => HttpResponse.json({ message: 'Logged out' })))
})

afterEach(() => {
  useNotificationStore.getState().clearAll()
  localStorage.clear()
})

it('does not treat an unavailable auth-status endpoint as a fresh installation', async () => {
  server.use(http.get('/api/v1/auth/status', () => HttpResponse.json(
    { detail: 'Temporarily unavailable' }, { status: 503 },
  )))

  await useAuthStore.getState().checkAuthStatus()

  expect(useAuthStore.getState().setupRequired).toBe(false)
  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(useAuthStore.getState().isLoading).toBe(false)
})

it('clears the previous operator notifications when logging out', async () => {
  useAuthStore.setState({ user: operator, isAuthenticated: true, isLoading: false })
  useNotificationStore.getState().addNotification({
    type: 'info', title: 'Synthetic prior session', message: 'A prior operator notification',
  })
  expect(useNotificationStore.getState().notifications).toHaveLength(1)

  await useAuthStore.getState().logout()

  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(useNotificationStore.getState().notifications).toHaveLength(0)
})

it('does not restore an old authenticated session from a response arriving after logout', async () => {
  let signalStarted!: () => void
  let releaseStatus!: () => void
  const started = new Promise<void>((resolve) => { signalStarted = resolve })
  const released = new Promise<void>((resolve) => { releaseStatus = resolve })
  server.use(http.get('/api/v1/auth/status', async () => {
    signalStarted()
    await released
    return HttpResponse.json({ authenticated: true, setup_complete: true, user: operator })
  }))
  useAuthStore.setState({ user: operator, isAuthenticated: true, isLoading: false })

  const oldStatusRequest = useAuthStore.getState().checkAuthStatus()
  await started
  try {
    await useAuthStore.getState().logout()
  } finally {
    releaseStatus()
    await oldStatusRequest
  }

  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(useAuthStore.getState().user).toBeNull()
})

it.each(['login', 'setup'] as const)('rejects late %s ownership after logout and another login', async action => {
  let started!: () => void
  let release!: () => void
  const ready = new Promise<void>(resolve => { started = resolve })
  const gate = new Promise<void>(resolve => { release = resolve })
  server.use(http.post(`/api/v1/auth/${action}`, async () => { started(); await gate; return HttpResponse.json({ user: operator }) }))
  const old = useAuthStore.getState()[action]('prior', 'synthetic-password').catch(() => undefined)
  await ready
  await useAuthStore.getState().logout()
  const next = { ...operator, id: 702, username: 'next-operator' }
  server.use(http.post('/api/v1/auth/login', () => HttpResponse.json({ user: next })))
  await useAuthStore.getState().login('next', 'synthetic-password')
  release()
  await old
  expect(useAuthStore.getState().user?.id).toBe(702)
  expect(useAuthStore.getState().isAuthenticated).toBe(true)
})

it('hides the previous operator while a different login is pending', async () => {
  let release!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  server.use(http.post('/api/v1/auth/login', async () => { await gate; return HttpResponse.json({ user: { ...operator, id: 702 } }) }))
  useAuthStore.setState({ user: operator, isAuthenticated: true, isLoading: false })
  const login = useAuthStore.getState().login('next', 'synthetic-password')
  try {
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    expect(useAuthStore.getState().user).toBeNull()
  } finally { release(); await login }
})

it('reports local isolation without falsely confirming failed server revocation', async () => {
  server.use(http.post('/api/v1/auth/logout', () => HttpResponse.json({}, { status: 503 })))
  useAuthStore.setState({ user: operator, isAuthenticated: true, isLoading: false })
  await useAuthStore.getState().logout()
  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(useAuthStore.getState().logoutError).toMatch(/could not be confirmed/)
})

it('expires the local session on a protected HTTP 401', async () => {
  useAuthStore.setState({ user: operator, isAuthenticated: true, isLoading: false })
  useNotificationStore.getState().addNotification({ type: 'info', title: 'Old', message: 'Prior operator' })
  server.use(http.get('/api/v1/protected-probe', () => HttpResponse.json({}, { status: 401 })))
  await api.get('/protected-probe').catch(() => undefined)
  expect(useAuthStore.getState().isAuthenticated).toBe(false)
  expect(useNotificationStore.getState().notifications).toHaveLength(0)
})

it('aborts protected requests before waiting for server logout', async () => {
  let started!: () => void
  let release!: () => void

  const ready = new Promise<void>((r) => { started = r })
  const gate = new Promise<void>((r) => { release = r })
  server.use(http.get('/api/v1/protected-probe', async () => {
    started()
    await gate
    return HttpResponse.json({ prior: true })
  }))
  const pending = api.get('/protected-probe').then(() => 'completed', () => 'cancelled')
  await ready
  try {
    await useAuthStore.getState().logout()
    expect(await pending).toBe('cancelled')
  } finally {
    release()
    await pending
  }
})
