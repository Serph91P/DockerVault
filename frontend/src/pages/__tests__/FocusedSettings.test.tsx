import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryRouter, RouterProvider } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Settings from '../Settings'
import { queryKeys } from '../../api/queryKeys'

const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()) })
function mount(hash = 'connection') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  clients.push(client)
  const router = createMemoryRouter([{ path: '/settings', element: <Settings /> }, { path: '/storage', element: <h1>Storage destination</h1> }], { initialEntries: [`/settings#${hash}`] })
  render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>)
  return { client, router }
}
it('offers five durable contexts and only shows the selected context', async () => {
  mount('system-recovery')
  const nav = screen.getByRole('navigation', { name: 'Settings sections' })
  expect(nav.querySelectorAll('a')).toHaveLength(5)
  expect(screen.getByRole('region', { name: 'System recovery' })).toHaveFocus()
  expect(screen.getByText(/Archives and volume data are NOT included/)).toBeVisible()
  expect(screen.queryByRole('button', { name: 'Restore system backup' })).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('link', { name: 'Connections', exact: true }))
  expect(screen.getByRole('region', { name: 'Docker connection settings' })).toHaveFocus()
})

it('dirty navigation can stay for correction or discard and go Back', async () => {
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json({ check_quota_before_upload: false })))
  const { router } = mount('storage-safeguards')
  const quota = await screen.findByRole('switch', { name: 'Check quota before upload' })
  await userEvent.click(quota)
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  await screen.findByRole('dialog', { name: 'Discard unsaved settings?' })
  await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel', exact: true }))
  expect(quota).toHaveAttribute('aria-checked', 'true')
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  await userEvent.click(await screen.findByRole('button', { name: 'Discard and leave' }))
  expect(await screen.findByRole('region', { name: 'System settings' })).toBeVisible()
  await act(async () => { await router.navigate(-1) })
  expect(screen.getByRole('region', { name: 'Storage safeguards' })).toHaveFocus()
  expect(await screen.findByRole('switch')).toHaveAttribute('aria-checked', 'false')
})

it.each([{}, { check_quota_before_upload: 'false' }])('unknown or malformed quota cannot be submitted as false (%j)', async data => {
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json(data)))
  mount('storage-safeguards')
  await screen.findByRole('alert', { name: 'Storage settings status' })
  expect(screen.queryByRole('switch', { name: 'Check quota before upload' })).not.toBeInTheDocument()
})

it.each([true, false])('quota %s is a private draft until Save and canonical readback', async initial => {
  let current = initial, writes = 0
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json({ check_quota_before_upload: current })), http.put('/api/v1/settings/storage', async ({ request }) => { current = (await request.json() as { check_quota_before_upload: boolean }).check_quota_before_upload; writes++; return HttpResponse.json({ check_quota_before_upload: current }) }))
  mount('storage-safeguards')
  await userEvent.click(await screen.findByRole('switch', { name: 'Check quota before upload' }))
  expect(writes).toBe(0)
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
  expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', String(initial))
  await userEvent.click(screen.getByRole('switch'))
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText('Storage settings saved; canonical values confirmed.')
  expect(current).toBe(!initial); expect(writes).toBe(1)
})

it('independent loads retain quota and disk fallback is not usage', async () => {
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json({ check_quota_before_upload: true })), http.get('/api/v1/settings/system-info', () => HttpResponse.json({ disk_status: 'unavailable', disk_total: 0, disk_used: 0, disk_free: 0, backup_count: 0, target_count: 0, uptime_seconds: 0, app_version: 'fixture' })))
  mount('storage-safeguards')
  expect(await screen.findByRole('switch')).toHaveAttribute('aria-checked', 'true')
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  expect(await screen.findByText('Disk capacity unavailable; fallback numbers are not measured usage.')).toBeVisible()
  expect(screen.queryByRole('meter')).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('link', { name: 'Storage safeguards', exact: true }))
  expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
})

it('a delayed pre-save refresh cannot replace canonical Storage readback', async () => {
  let release!: () => void, refreshStarted!: () => void
  const held = new Promise<void>(r => { release = r }), started = new Promise<void>(r => { refreshStarted = r })
  let reads = 0, saved = false
  const before = { check_quota_before_upload: false }
  const after = { check_quota_before_upload: true }
  server.use(http.get('/api/v1/settings/storage', async () => { reads++; if (reads === 2) { refreshStarted(); await held; return HttpResponse.json(before) } return HttpResponse.json(saved ? after : before) }), http.put('/api/v1/settings/storage', () => { saved = true; return HttpResponse.json(after) }))
  const { client } = mount('storage-safeguards')
  await userEvent.click(await screen.findByRole('switch', { name: 'Check quota before upload' }))
  const key = queryKeys.settings.storage
  const refresh = client.refetchQueries({ queryKey: key })
  await started
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText('Storage settings saved; canonical values confirmed.')
  await act(async () => { release(); await refresh })
  await waitFor(() => expect(client.getQueryData(key)).toEqual(after))
})

it('reverting to the original storage value during Save remains an editable newer intent', async () => {
  let release!: () => void, started!: () => void, saved = false
  const held = new Promise<void>(r => { release = r }), admitted = new Promise<void>(r => { started = r })
  const before = { check_quota_before_upload: false }
  const after = { check_quota_before_upload: true }
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json(saved ? after : before)), http.put('/api/v1/settings/storage', async () => { saved = true; started(); await held; return HttpResponse.json(after) }))
  mount('storage-safeguards')
  await userEvent.click(await screen.findByRole('switch', { name: 'Check quota before upload' }))
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true })); await admitted
  await userEvent.click(screen.getByRole('switch', { name: 'Check quota before upload' }))
  await act(async () => { release() })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save', exact: true })).toBeEnabled())
  expect(screen.getByText('Unsaved storage draft')).toBeVisible()
  expect(screen.queryByText(/settings saved; canonical values confirmed/)).not.toBeInTheDocument()
})
