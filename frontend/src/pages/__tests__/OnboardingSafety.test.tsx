import { afterEach, expect, it } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import App from '../../App'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../../store/auth'
import { clearSession, queryClient } from '../../store/session'

const operator = { id: 1, username: 'operator', is_admin: true, created_at: '', last_login: null }
afterEach(() => { cleanup(); clearSession() })
function mount() {
  useAuthStore.setState({ user: null, isAuthenticated: false, setupRequired: false, isLoading: true, statusError: null })
  render(<QueryClientProvider client={queryClient}><MemoryRouter><App /></MemoryRouter></QueryClientProvider>)
}

it('offers status retry without setup or protected content on HTTP 503', async () => {
  server.use(http.get('/api/v1/auth/status', () => HttpResponse.json({}, { status: 503 })))
  mount()
  await screen.findByRole('button', { name: /retry/i })
  expect(screen.queryByRole('button', { name: 'Create account' })).not.toBeInTheDocument()
  expect(screen.queryByRole('navigation')).not.toBeInTheDocument()
})

it('does not render a one-time key arriving after logout during generation', async () => {
  let started!: () => void
  let release!: () => void
  const ready = new Promise<void>(resolve => { started = resolve })
  const gate = new Promise<void>(resolve => { release = resolve })
  const marker = 'synthetic-late-private-material'
  server.use(
    http.get('/api/v1/auth/status', () => HttpResponse.json({ authenticated: true, setup_complete: true, user: operator })),
    http.get('/api/v1/encryption/status', () => HttpResponse.json({ setup_completed: false, encryption_enabled: false, public_key: null, onboarding_deferred: false })),
    http.post('/api/v1/encryption/setup', async () => { started(); await gate; return HttpResponse.json({ private_key: marker, public_key: 'public', recovery_instructions: 'instructions' }) }),
    http.post('/api/v1/auth/logout', () => HttpResponse.json({ message: 'Logged out' })),
  )
  mount()
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Set Up Encryption' }))
  await ready
  await act(() => useAuthStore.getState().logout())
  release()
  await screen.findByRole('button', { name: 'Sign In' })
  expect(document.body.textContent?.includes(marker)).toBe(false)
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
  expect(queryClient.getQueryCache().getAll()).toHaveLength(0)
})

it('keeps account success recoverable through encryption failure and reload, without replaying account setup', async () => {
  let created = false
  let accounts = 0
  server.use(
    http.get('/api/v1/auth/status', () => HttpResponse.json({ authenticated: created, setup_complete: created, user: created ? operator : null })),
    http.post('/api/v1/auth/setup', () => { created = true; accounts++; return HttpResponse.json({ user: operator }) }),
    http.get('/api/v1/encryption/status', () => HttpResponse.json({ setup_completed: false, encryption_enabled: false, public_key: null, onboarding_deferred: false })),
    http.post('/api/v1/encryption/setup', () => HttpResponse.json({}, { status: 500 })),
  )
  mount()
  const user = userEvent.setup()
  await user.type(await screen.findByLabelText('Username'), 'operator')
  await user.type(screen.getByLabelText('Account password'), 'synthetic-password')
  await user.type(screen.getByLabelText('Confirm account password'), 'synthetic-password')
  await user.dblClick(screen.getByRole('button', { name: 'Create account' }))
  await user.click(await screen.findByRole('button', { name: 'Set Up Encryption' }))
  await screen.findByRole('alert')
  expect(screen.queryByRole('navigation')).not.toBeInTheDocument()
  cleanup()
  clearSession()
  mount()
  await screen.findByRole('button', { name: 'Set Up Encryption' })
  expect(screen.queryByLabelText('Account password')).not.toBeInTheDocument()
  expect(accounts).toBe(1)
})
