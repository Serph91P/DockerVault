import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Dashboard from '../Dashboard'
import Backups from '../Backups'
import Schedules from '../Schedules'
import Retention from '../Retention'
import Storage from '../Storage'
import Settings from '../Settings'

const routes = [
  ['Overview', '/', '/targets', 'Plans', <Dashboard />],
  ['Protection', '/protection', '/targets', 'Plans', <Backups workspace="protection" />],
  ['History', '/history', '/backups', 'Archives', <Backups workspace="history" />],
  ['Schedules', '/protection/schedules', '/schedules', 'Schedules', <Schedules />],
  ['Retention', '/protection/retention', '/retention', 'Retention policies', <Retention />],
  ['Storage', '/storage', '/storage', 'Storage locations', <Storage />],
  ['Settings', '/settings#storage-safeguards', '/settings/storage', 'Storage settings', <Settings />],
] as const
it('malformed compatibility items keep independent schedules visible with a scoped Retry', async () => {
  let malformed = true
  let scheduleReads = 0
  server.use(
    http.get('/api/v1/schedules', () => { scheduleReads++; return HttpResponse.json([{ id: 1, name: 'Independent valid schedule', cron_expression: '0 2 * * *', enabled: true, target_count: 0 }]) }),
    http.get('/api/v1/schedules/compatibility/inventory', () => HttpResponse.json({ contract: 'synthetic contract', timezone: 'UTC', items: [{ kind: 'schedule', id: 1, name: 'Independent valid schedule', expression: '0 2 * * *', warnings: malformed ? null : ['Review intent'] }] })),
  )
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(<QueryClientProvider client={client}><MemoryRouter><Schedules /></MemoryRouter></QueryClientProvider>)
  expect(await screen.findByRole('alert', { name: 'Schedule compatibility status' })).toHaveTextContent(/unavailable/i)
  expect(screen.getByText('Independent valid schedule')).toBeVisible()
  malformed = false
  await userEvent.click(screen.getByRole('button', { name: 'Retry Schedule compatibility' }))
  expect(await screen.findByText('Review intent')).toBeVisible()
  expect(screen.getByRole('button', { name: 'Refresh Schedule compatibility' })).toHaveFocus()
  expect(scheduleReads).toBe(1)
})
const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()) })
for (const [name, route, endpoint, label, page] of routes) {
  it(`${name}: preserves confirmed data as stale after refresh failure, then Retry updates it`, async () => {
    let failed = false
    const valid = endpoint === '/settings/storage' ? { check_quota_before_upload: true } : []
    server.use(http.get(`/api/v1${endpoint}`, () => failed ? new HttpResponse(null, { status: 503 }) : HttpResponse.json(valid)))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    clients.push(client)
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{page}</MemoryRouter></QueryClientProvider>)
    await waitFor(() => expect(screen.getByLabelText(`${label} status`)).toHaveTextContent(/updated/i))
    failed = true
    await client.invalidateQueries()
    expect(await screen.findByRole('alert', { name: `${label} status` })).toHaveTextContent(/stale data/i)
    expect(screen.getByRole('alert', { name: `${label} status` })).toHaveTextContent(/last successful refresh/i)
    failed = false
    await userEvent.click(screen.getByRole('button', { name: `Retry ${label}` }))
    await waitFor(() => expect(screen.queryByRole('alert', { name: `${label} status` })).not.toBeInTheDocument())
  })
  it.each([403, 503, 'network', 'malformed'])(`${name}: %s is unavailable, never verified empty, and Retry is actionable`, async fault => {
    let failed = true
    server.use(http.get(`/api/v1${endpoint}`, () => !failed ? HttpResponse.json(endpoint === '/settings/storage' ? { check_quota_before_upload: true } : []) : fault === 'network' ? HttpResponse.error() : fault === 'malformed' ? HttpResponse.json({ unexpected: true }) : new HttpResponse(null, { status: fault as number })))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    clients.push(client)
    render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{page}</MemoryRouter></QueryClientProvider>)
    const alert = await screen.findByRole('alert', { name: `${label} status` })
    expect(alert).toHaveTextContent(fault === 403 ? /access denied/i : /unavailable/i)
    if (endpoint === '/targets') expect(screen.queryByText('No backups configured')).not.toBeInTheDocument()
    failed = false
    await userEvent.click(screen.getByRole('button', { name: `Retry ${label}` }))
    await waitFor(() => expect(screen.queryByRole('alert', { name: `${label} status` })).not.toBeInTheDocument())
    expect(screen.getByRole('button', { name: `Refresh ${label}` })).toHaveFocus()
  })
}
