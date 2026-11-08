import { act, render, screen, within, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useAuthStore } from '../../store/auth'
import { clearSession, queryClient as sessionClient } from '../../store/session'
import api from '../../api'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import { describe, it, expect } from 'vitest'
import { server } from '../../test/mocks/server'
import { validateRead } from '../../api/responses'
import Dashboard from '../Dashboard'

export const summary = {
  observed_at: '2026-11-08T12:00:00Z', dataset: 'retained_catalog_and_current_process',
  window: { after: '2026-11-07T12:00:00Z', before: '2026-11-08T12:00:00Z', time_field: 'created_at' },
  plans: { total: 1, enabled: 1, disabled: 0, manual: 1, automatic_enabled: 0, automatic_paused: 0, without_recorded_run: { total: 0, enabled: 0, items: [] }, without_recorded_completed_run: { total: 0, enabled: 0, items: [] } },
  backup_activity: { recorded_pending: 0, recorded_running: 0, live: 0, total: 0, items: [] as Record<string, unknown>[] },
  storage_activity: { total: 0, items: [] as Record<string, unknown>[] },
  recorded_copy_failures: { backups: 0, total: 0, items: [] as Record<string, unknown>[] },
}
const historyRow = { id: 7, target_id: 1, target_name: 'Fixture archive', status: 'completed', created_at: '2026-11-08T11:00:00Z', source_type: 'path', source_identity: '/synthetic', encrypted: false, local_available: false, remote_synced: true, plan_removed: false, retention_context: 'retained', archive_state: 'recorded', local_state: 'missing', can_restore: true, can_sync: false, remote_sync_details: [] }
function setup(body = summary) {
  server.use(
    http.get('/api/v1/overview/summary', () => HttpResponse.json(body)),
    http.get('/api/v1/backups/history', ({ request }) => {
      const params = new URL(request.url).searchParams
      const failed = params.get('status') === 'failed'
      return HttpResponse.json({ total: failed ? 0 : 507, items: failed ? [] : [historyRow], limit: 5, offset: 0, snapshot: 7, dataset: 'retained_catalog' })
    }),
  )
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(<QueryClientProvider client={client}><MemoryRouter><Dashboard /></MemoryRouter></QueryClientProvider>)
  return client
}
describe('Overview contract', () => {
  it('orders attention before real work and archive context, using server totals and exact archive links', async () => {
    setup()
    expect(await screen.findByRole('heading', { name: 'Needs attention' })).toBeInTheDocument()
    expect(await screen.findByText('Currently shown 1 of 507 retained archives')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Fixture archive' })).toHaveAttribute('href', '/history/archives/7')
    expect(screen.getByText(/Local: missing/)).toBeInTheDocument()
    const headings = screen.getAllByRole('heading', { level: 2 }).map(h => h.textContent)
    expect(headings.indexOf('Needs attention')).toBeLessThan(headings.indexOf('Current work'))
    expect(headings.indexOf('Current work')).toBeLessThan(headings.indexOf('Recent archives'))
  })
  it('keeps early-completed uploads active and separates unobserved catalog records', async () => {
    const body = structuredClone(summary)
    body.backup_activity = { recorded_pending: 0, recorded_running: 1, live: 1, total: 2, items: [
      { backup_id: 8, plan_id: 1, name: 'Uploading fixture', catalog_status: 'completed', stage: 'upload', live_worker: true },
      { backup_id: 9, plan_id: null, name: null, catalog_status: 'running', stage: null, live_worker: false },
    ] }
    setup(body)
    expect(await screen.findByText(/Live worker · upload/)).toBeInTheDocument()
    expect(screen.getByText(/No live worker observed/)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Uploading fixture' })).toHaveAttribute('href', '/history/runs/8')
  })
  it('opens recorded copy failures in both the archive/run and exact location, retaining missing mappings', async () => {
    const body = structuredClone(summary)
    body.recorded_copy_failures = { total: 508, backups: 507, items: [{ backup_id: 7, storage_id: 43, storage_name: null, status: 'failed' }] }
    setup(body)
    const region = await screen.findByRole('region', { name: 'Recorded copy failures' })
    expect(within(region).getByText(/508 failed copy records across 507 retained runs/)).toBeInTheDocument()
    expect(within(region).getByRole('link', { name: 'Run #7' })).toHaveAttribute('href', '/history/runs/7')
    expect(within(region).getByRole('link', { name: /Unknown or unavailable location/ })).toHaveAttribute('href', '/storage?storage=43')
  })
  it('keeps independent archive data when the overview response is malformed', async () => {
    setup({ ...summary, backup_activity: { ...summary.backup_activity, total: -1 } })
    expect(await screen.findByText('Overview summary: unavailable')).toBeInTheDocument()
    expect(await screen.findByRole('link', { name: 'Fixture archive' })).toBeInTheDocument()
    expect(screen.queryByText('No attention items recorded')).not.toBeInTheDocument()
  })
  it.each([
    { ...summary, observed_at: 7 },
    { ...summary, window: { ...summary.window, after: summary.window.before } },
    { ...summary, plans: { ...summary.plans, total: 500 } },
    { ...summary, storage_activity: { total: 1, items: [{ operation_id: '../bad', storage_id: 'local', state: 'running', stage: 'deleting' }] } },
    { ...summary, recorded_copy_failures: { total: 0, backups: 0, items: [{ backup_id: 1, storage_id: 2, storage_name: null, status: 'failed' }] } },
  ])('rejects malformed nested summary before it can become cached truth %#', value => {
    expect(() => validateRead('/overview/summary', value)).toThrow()
  })
  it('keeps incomplete encryption readiness in attention instead of claiming no issues', async () => {
    server.use(
      http.get('/api/v1/docker/health', () => HttpResponse.json({ status: 'healthy' })),
      http.get('/api/v1/settings/system-info', () => HttpResponse.json({ disk_status: 'available', disk_total: 8 * 1024 ** 3, disk_used: 2 * 1024 ** 3, disk_free: 6 * 1024 ** 3, backup_count: 1, target_count: 1, uptime_seconds: 1, app_version: 'fixture' })),
      http.get('/api/v1/encryption/status', () => HttpResponse.json({ setup_completed: false, encryption_enabled: true, onboarding_deferred: false })),
    )
    setup()
    expect(await screen.findByText('Encryption decision incomplete — confirm custody or explicitly defer.')).toBeInTheDocument()
    expect(screen.queryByText(/No attention items recorded in these scopes/)).not.toBeInTheDocument()
  })
  it('retains the last confirmed failure window through a newer-window error, then replaces it on retry', async () => {
    const user = userEvent.setup()
    setup()
    let current = summary
    let fail = false
    const requested: string[] = []
    server.use(
      http.get('/api/v1/overview/summary', () => HttpResponse.json(current)),
      http.get('/api/v1/backups/history', ({ request }) => {
        const params = new URL(request.url).searchParams
        const failed = params.get('status') === 'failed'
        if (failed) requested.push(params.get('before')!)
        if (failed && fail) return new HttpResponse(null, { status: 503 })
        return HttpResponse.json({ total: 1, items: [{ ...historyRow, status: failed ? 'failed' : 'completed' }], limit: 5, offset: 0, snapshot: 7, dataset: 'retained_catalog' })
      }),
    )
    const failure = await screen.findByRole('link', { name: 'Failed run #7: Fixture archive' })
    const originalHref = failure.getAttribute('href')
    fail = true
    current = { ...summary, observed_at: '2026-11-08T13:00:00Z', window: { ...summary.window, after: '2026-11-07T13:00:00Z', before: '2026-11-08T13:00:00Z' } }
    await user.click(screen.getByRole('button', { name: 'Refresh Overview summary', exact: true }))
    await waitFor(() => expect(requested).toContain(current.window.before))
    expect(await screen.findByText('Failed runs in window: stale data')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Failed run #7: Fixture archive' })).toHaveAttribute('href', originalHref)
    expect(screen.getByText(`${summary.window.after} through ${summary.window.before}; created_at, not the time failure occurred.`)).toBeInTheDocument()
    expect(screen.getByText('Currently shown 1 of 1 retained archives')).toBeInTheDocument()
    fail = false
    await user.click(screen.getByRole('button', { name: 'Retry Failed runs in window', exact: true }))
    await waitFor(() => expect(screen.getByRole('link', { name: 'Failed run #7: Fixture archive' })).not.toHaveAttribute('href', originalHref))
    expect(new URL(screen.getByRole('link', { name: 'Failed run #7: Fixture archive' }).getAttribute('href')!, 'http://localhost').searchParams.get('before')).toBe(current.window.before)
    expect(screen.queryByText('Failed runs in window: stale data')).not.toBeInTheDocument()
  })
  it('does not retain a failure snapshot across an owner/session boundary', async () => {
    const client = setup()
    server.use(http.get('/api/v1/backups/history', ({ request }) => HttpResponse.json({ total: 1, items: [{ ...historyRow, status: new URL(request.url).searchParams.get('status') === 'failed' ? 'failed' : 'completed' }], limit: 5, offset: 0, snapshot: 7, dataset: 'retained_catalog' })))
    expect(await screen.findByRole('link', { name: 'Failed run #7: Fixture archive' })).toBeInTheDocument()
    server.use(http.get('/api/v1/backups/history', () => new HttpResponse(null, { status: 503 })))
    await act(async () => {
      clearSession()
      client.clear()
      useAuthStore.setState({ user: { id: 188, username: 'next', is_admin: false, created_at: '', last_login: null } })
    })
    expect(await screen.findByText('Failed runs in window: unavailable')).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Failed run #7: Fixture archive' })).not.toBeInTheDocument()
  })
  it.each(['logout', 'expiry'] as const)('discards confirmed failures after %s and a same-owner session remount', async (reason) => {
    const identity = { id: 188, username: 'same-operator', is_admin: false, created_at: '', last_login: null }
    useAuthStore.setState({ user: identity, isAuthenticated: true })
    server.use(http.get('/api/v1/overview/summary', () => HttpResponse.json(summary)))
    let fail = false
    server.use(http.get('/api/v1/backups/history', ({ request }) => {
      const failed = new URL(request.url).searchParams.get('status') === 'failed'
      if (failed && fail) return new HttpResponse(null, { status: 503 })
      return HttpResponse.json({ total: 1, items: [{ ...historyRow, status: failed ? 'failed' : 'completed' }], limit: 5, offset: 0, snapshot: 7, dataset: 'retained_catalog' })
    }))
    const mount = () => render(<QueryClientProvider client={sessionClient}><MemoryRouter><Dashboard /></MemoryRouter></QueryClientProvider>)
    const first = mount()
    expect(await screen.findByRole('link', { name: 'Failed run #7: Fixture archive' })).toBeInTheDocument()
    first.unmount()
    fail = true
    const sameSession = mount()
    await userEvent.setup().click(await screen.findByRole('button', { name: 'Refresh Failed runs in window', exact: true }))
    expect(await screen.findByText('Failed runs in window: stale data')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Failed run #7: Fixture archive' })).toBeInTheDocument()
    sameSession.unmount()
    server.use(
      http.post('/api/v1/auth/logout', () => HttpResponse.json({})),
      http.get('/api/v1/targets', () => new HttpResponse(null, { status: 401 })),
    )
    await act(async () => {
      if (reason === 'logout') await useAuthStore.getState().logout()
      else await api.get('/targets').catch(() => undefined)
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    server.use(http.get('/api/v1/targets', () => HttpResponse.json([])))
    useAuthStore.setState({ user: identity, isAuthenticated: true })
    mount()
    expect(await screen.findByText('Failed runs in window: unavailable')).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Failed run #7: Fixture archive' })).not.toBeInTheDocument()
    expect(await screen.findByRole('link', { name: 'Fixture archive' })).toBeInTheDocument()
    fail = false
    await userEvent.setup().click(screen.getByRole('button', { name: 'Retry Failed runs in window', exact: true }))
    expect(await screen.findByRole('link', { name: 'Failed run #7: Fixture archive' })).toBeInTheDocument()
  })
  it('accepts the complete summary with unknown optional reference values', () => {
    expect(() => validateRead('/overview/summary', summary)).not.toThrow()
  })
})
