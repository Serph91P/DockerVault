import { render, screen, within } from '@testing-library/react'
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
  it('accepts the complete summary with unknown optional reference values', () => {
    expect(() => validateRead('/overview/summary', summary)).not.toThrow()
  })
})
