import { describe, it, expect } from 'vitest'
import { render, screen, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'
import Dashboard from '../Dashboard'
import { server } from '../../test/mocks/server'
import { http, HttpResponse } from 'msw'
import { mockHistoryRecord, mockOverview } from '../../test/mocks/overview'

function mount() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  render(<QueryClientProvider client={queryClient}><BrowserRouter><Dashboard /></BrowserRouter></QueryClientProvider>)
}
function history(rows: Record<string, unknown>[]) {
  server.use(http.get('/api/v1/backups/history', ({ request }) => {
    const p = new URL(request.url).searchParams
    const filtered = rows.filter(row => !p.get('status') || row.status === p.get('status'))
    return HttpResponse.json({ total: filtered.length, items: filtered.slice(0, Number(p.get('limit'))).map(mockHistoryRecord), limit: Number(p.get('limit')), offset: 0, snapshot: 9, dataset: 'retained_catalog' })
  }))
}
describe('Dashboard Page', () => {
  it('should display dashboard title and description', () => {
    mount()
    expect(screen.getByRole('heading', { level: 1, name: 'Overview' })).toBeInTheDocument()
    expect(screen.getByText('Overview of your Docker backups')).toBeInTheDocument()
  })
  it('should display resource labels and explicit configured-plan scopes', async () => {
    mount()
    for (const label of ['Containers', 'Volumes', 'Stacks', 'Plans (not removed)']) expect(await screen.findByText(label)).toBeInTheDocument()
    expect(await screen.findByText(/2 total · 2 enabled · 0 disabled · 2 manual/)).toBeInTheDocument()
  })
  it('should display recent archives section', () => {
    mount(); expect(screen.getByRole('heading', { name: 'Recent archives' })).toBeInTheDocument()
  })
  it('should display registered next-executions section', () => {
    mount(); expect(screen.getByRole('heading', { name: 'Next registered executions' })).toBeInTheDocument()
  })
  it('should display labeled context rather than sampled success metrics', () => {
    mount()
    expect(screen.getByRole('heading', { name: 'Resource and plan context' })).toBeInTheDocument()
    expect(screen.getByText('Completed retained runs')).toBeInTheDocument()
    expect(screen.getByText('Retained runs')).toBeInTheDocument()
    expect(screen.getByText(/Inventory and configured scope, not backed-up coverage/)).toBeInTheDocument()
  })
  it('should display container count from API without claiming protection', async () => {
    server.use(http.get('/api/v1/docker/containers', () => HttpResponse.json([{ id: '1', name: 'one', status: 'running' }, { id: '2', name: 'two', status: 'running' }])))
    mount(); expect(await screen.findByText('2/2 running / discovered')).toBeInTheDocument()
  })
  it('should display volume count from API', async () => {
    server.use(http.get('/api/v1/docker/volumes', () => HttpResponse.json([{ name: 'one' }, { name: 'two' }, { name: 'three' }])))
    mount()
    const label = await screen.findByText('Volumes')
    expect(await within(label.parentElement!).findByText('3')).toBeInTheDocument()
  })
  it('should display configured backup targets with intentional detail destinations', async () => {
    server.use(http.get('/api/v1/targets', () => HttpResponse.json([{ id: 1, name: 'target-1', target_type: 'volume', enabled: true }, { id: 2, name: 'target-2', target_type: 'container', enabled: true }])))
    mount()
    expect(await screen.findByRole('link', { name: 'target-1' })).toHaveAttribute('href', '/protection/1')
    expect(screen.getByRole('link', { name: 'target-2' })).toHaveAttribute('href', '/protection/2')
  })
  it('should display archive list, size and duration from History API', async () => {
    history([{ id: 1, target_id: 1, target_name: 'my-database-backup', status: 'completed', created_at: '2024-01-01T10:00:00Z', file_size_human: '1 KB', duration_seconds: 60 }])
    mount()
    expect(await screen.findByRole('link', { name: 'my-database-backup' })).toHaveAttribute('href', '/history/archives/1')
    expect(screen.getByText(/1 KB · 60s/)).toBeInTheDocument()
  })
  it('should show empty archive state only after confirmed History response', async () => {
    history([]); mount()
    expect(await screen.findByText(/No recorded archives yet/)).toBeInTheDocument()
  })
  it('should show empty registered-execution state', async () => {
    server.use(http.get('/api/v1/schedules', () => HttpResponse.json([])), http.get('/api/v1/schedules/jobs', () => HttpResponse.json([])))
    mount(); expect(await screen.findByText('No registered next executions.')).toBeInTheDocument()
    expect(await screen.findByText('0 enabled of 0 schedule definitions')).toBeInTheDocument()
  })
  it('should display schedule definitions and actual registered times separately', async () => {
    server.use(
      http.get('/api/v1/schedules', () => HttpResponse.json([{ id: 1, name: 'Daily Backup Schedule', cron_expression: '0 3 * * *', enabled: true }])),
      http.get('/api/v1/schedules/jobs', () => HttpResponse.json([{ id: 'backup_target_2', name: 'Actual registered plan', next_run: '2027-01-02T03:00:00Z' }])),
    )
    mount()
    expect(await screen.findByRole('link', { name: 'Daily Backup Schedule' })).toHaveAttribute('href', '/protection/schedules/1')
    expect(screen.getByText(/0 3 \* \* \*/)).toBeInTheDocument()
    expect(await screen.findByRole('link', { name: 'Actual registered plan' })).toHaveAttribute('href', '/protection/2')
    expect(screen.getByText('2027-01-02T03:00:00Z')).toBeInTheDocument()
  })
  it('should count running containers correctly', async () => {
    server.use(http.get('/api/v1/docker/containers', () => HttpResponse.json([{ id: '1', name: 'running', status: 'running' }, { id: '2', name: 'stopped', status: 'exited' }])))
    mount(); expect(await screen.findByText('1/2 running / discovered')).toBeInTheDocument()
  })
  it('should use authoritative completed and failed totals in their distinct scopes', async () => {
    history([{ id: 1, target_id: 1, target_name: 'one', status: 'completed', created_at: '2026-11-08T10:00:00Z' }, { id: 2, target_id: 1, target_name: 'two', status: 'completed', created_at: '2026-11-08T11:00:00Z' }, { id: 3, target_id: 1, target_name: 'three', status: 'failed', created_at: '2026-11-08T11:00:00Z' }])
    mount()
    expect(await screen.findByRole('heading', { name: '1 failed runs created in the UTC 24-hour window' })).toBeInTheDocument()
    expect(await screen.findByText('2 (catalog status, not restore verification)')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Failed run #3: three' })).toHaveAttribute('href', expect.stringContaining('/history/runs/3?view=runs&status=failed'))
  })
  it('should be accessible with proper headings and first-plan action', () => {
    mount()
    for (const name of ['Overview', 'Needs attention', 'Current work', 'Recent archives', 'Next registered executions', 'Resource and plan context', 'Local capacity']) expect(screen.getByRole('heading', { name })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'New backup plan' })).toHaveAttribute('href', '/protection/new')
  })
  it('should distinguish an empty installation from plans without retained runs', async () => {
    server.use(http.get('/api/v1/overview/summary', () => HttpResponse.json({ ...mockOverview, plans: { ...mockOverview.plans, total: 0, enabled: 0, manual: 0 } })))
    history([]); mount()
    expect(await screen.findByRole('region', { name: 'First backup readiness' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Create first backup plan' })).toHaveAttribute('href', '/protection/new')
  })
})
