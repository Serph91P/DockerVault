import { afterEach, beforeEach, expect, it } from 'vitest'
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
