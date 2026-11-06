import { render, screen, waitFor, within } from '@testing-library/react'
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
  it.each([['/protection/schedules/new', 'New schedule', 'Schedules'], ['/protection/schedules/8/edit', 'Edit schedule', 'Schedules'], ['/protection/retention/7', 'Retention policy detail', 'Retention']])('names the policy subroute and links its breadcrumb at %s', async (path, title, section) => {
    renderPath(path)
    await waitFor(() => expect(document.title).toBe(`${title} · DockerVault`))
    expect(within(screen.getByRole('navigation', { name: 'Breadcrumb' })).getByRole('link', { name: section })).toHaveAttribute('href', path.startsWith('/protection/schedules') ? '/protection/schedules' : '/protection/retention')
  })
  it.each(['/protection/schedules/new', '/protection/retention/new', '/protection/schedules/8/edit', '/protection/retention/7/edit', '/protection/schedules/edit', '/protection/retention/edit'])('never interprets a static policy segment as a plan at %s', async path => {
    let planReads = 0
    server.use(http.get('/api/v1/targets/:id', () => { planReads++; return new HttpResponse(null, { status: 404 }) }))
    renderPath(path)
    const heading = await screen.findByRole('heading', { level: 1 })
    expect(heading).not.toHaveTextContent('Invalid protection plan address')
    expect(planReads).toBe(0)
  })
  it.each([['/protection', 'Protection'], ['/history', 'History & Restore'], ['/protection/schedules', 'Schedules'], ['/protection/retention', 'Retention Policies']])('renders the working destination at %s', async (path, title) => {
    renderPath(path)
    expect(await screen.findByRole('heading', { level: 1, name: title })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent(path)
  })
  it.each([['/backups', '/protection'], ['/schedules', '/protection/schedules'], ['/retention', '/protection/retention']])('redirects legacy %s deliberately', async (path, destination) => {
    renderPath(path)
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent(destination))
  })
  it('opens a real plan detail and immutable-source edit route, preserving safe return context', async () => {
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ id: 917, name: 'Route fixture', target_type: 'path', host_path: '/fixture', enabled: true, dependencies: [], selected_volumes: [], include_paths: [], exclude_paths: [], per_volume_rules: {}, remote_storage_ids: [], compression_enabled: true, stop_container: false })))
    renderPath('/protection/917/edit?returnTo=%2Fprotection%3Fq%3Dfixture%26type%3Dpath')
    expect(await screen.findByRole('heading', { name: 'Edit protection: Route fixture' })).toBeInTheDocument()
    expect(screen.getByText(/Source \(read-only\): path/)).toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Return to plan without saving' })).toHaveAttribute('href', '/protection/917?returnTo=%2Fprotection%3Fq%3Dfixture%26type%3Dpath')
  })
  it('consumes the existing notification plan query without losing valid filters', async () => {
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ id: 917, name: 'Route fixture', target_type: 'path', host_path: '/fixture', enabled: true, dependencies: [], selected_volumes: [], include_paths: [], exclude_paths: [], per_volume_rules: {}, remote_storage_ids: [], compression_enabled: true, stop_container: false })))
    renderPath('/protection?plan=917&q=fixture&type=path')
    expect(await screen.findByRole('heading', { level: 1, name: 'Route fixture' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Return to protection list' })).toHaveAttribute('href', '/protection?q=fixture&type=path')
  })
  it('keeps an unavailable plan address and retained-history recovery distinct from Overview', async () => {
    server.use(http.get('/api/v1/targets/99999', () => new HttpResponse(null, { status: 404 })))
    renderPath('/protection/99999')
    expect(await screen.findByRole('heading', { name: 'Protection plan unavailable' })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent('/protection/99999')
    expect(screen.getByRole('link', { name: 'Open retained history' })).toHaveAttribute('href', '/history')
  })
  it.each(['/missing', '/storage/99999', '/history/99999'])('does not misrepresent missing %s as overview', async (path) => {
    renderPath(path)
    expect(await screen.findByRole('heading', { name: 'Page not found' })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent(path)
  })
})
