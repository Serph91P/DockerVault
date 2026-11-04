import { render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it } from 'vitest'
import App from '../../App'
import { useAuthStore } from '../../store/auth'
import { server } from '../../test/mocks/server'
import { http, HttpResponse } from 'msw'

function Location() { return <output data-testid="location">{useLocation().pathname}</output> }
function renderPath(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[path]}><App /><Location /></MemoryRouter></QueryClientProvider>)
}
beforeEach(() => {
  useAuthStore.setState({ isAuthenticated: true, isLoading: false, setupRequired: false, user: { id: 1, username: 'operator', created_at: '', is_admin: true, last_login: null } })
  server.use(http.get('/api/v1/auth/status', () => HttpResponse.json({ setup_complete: true, authenticated: true, user: { id: 1, username: 'operator' } })))
  server.use(http.get('/api/v1/encryption/status', () => HttpResponse.json({ setup_completed: true, encryption_enabled: true, onboarding_deferred: false })))
})
describe('workspace routes', () => {
  it.each([['/protection', 'Protection'], ['/history', 'History & Restore'], ['/protection/schedules', 'Schedules'], ['/protection/retention', 'Retention Policies']])('renders the working destination at %s', async (path, title) => {
    renderPath(path)
    expect(await screen.findByRole('heading', { level: 1, name: title })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent(path)
  })
  it.each([['/backups', '/protection'], ['/schedules', '/protection/schedules'], ['/retention', '/protection/retention']])('redirects legacy %s deliberately', async (path, destination) => {
    renderPath(path)
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent(destination))
  })
  it.each(['/missing', '/protection/99999', '/storage/99999', '/history/99999'])('does not misrepresent missing %s as overview', async (path) => {
    renderPath(path)
    expect(await screen.findByRole('heading', { name: 'Page not found' })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent(path)
  })
})
