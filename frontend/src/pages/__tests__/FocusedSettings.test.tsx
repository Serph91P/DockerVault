import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryRouter, RouterProvider } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Settings from '../Settings'
import { clearSession, onSessionExpired } from '../../store/session'
import { queryKeys } from '../../api/queryKeys'

const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()); onSessionExpired(() => undefined) })
const config = { enabled: true, api_url: 'https://saved.example', has_api_key: true, has_api_secret: true, connected: false }
function mount(hash = 'connection') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  clients.push(client)
  const router = createMemoryRouter([{ path: '/settings', element: <Settings /> }, { path: '/storage', element: <h1>Storage destination</h1> }], { initialEntries: [`/settings#${hash}`] })
  render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>)
  return { client, router }
}
function komodo(read = () => HttpResponse.json(config)) {
  server.use(http.get('/api/v1/settings/komodo', read))
}
async function edit() { await userEvent.click(await screen.findByRole('button', { name: 'Edit Settings' })); return screen.getByRole('textbox', { name: 'Komodo API URL' }) }

it('offers five durable contexts and only shows the selected context', async () => {
  mount('system-recovery')
  const nav = screen.getByRole('navigation', { name: 'Settings sections' })
  expect(nav.querySelectorAll('a')).toHaveLength(5)
  expect(screen.getByRole('region', { name: 'System recovery' })).toHaveFocus()
  expect(screen.getByText(/#143/)).toBeVisible()
  expect(screen.queryByRole('button', { name: /import|export/i })).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('link', { name: 'Connections', exact: true }))
  expect(screen.getByRole('region', { name: 'Docker connection settings' })).toHaveFocus()
})

it('tests saved settings only, never writes a draft, and Cancel clears private inputs', async () => {
  let writes = 0, tests = 0
  komodo()
  server.use(http.put('/api/v1/settings/komodo', () => { writes++; return HttpResponse.json(config) }), http.post('/api/v1/settings/komodo/test', () => { tests++; return HttpResponse.json({ success: false, message: 'failed' }) }))
  const { client } = mount()
  const url = await edit()
  await userEvent.clear(url); await userEvent.type(url, 'https://draft.example')
  await userEvent.type(screen.getByLabelText('Komodo API key', { exact: true }), 'private-fixture-marker')
  expect(screen.getByRole('button', { name: 'Test saved connection' })).toBeDisabled()
  expect(JSON.stringify(client.getQueryCache().getAll().map(q => q.state))).not.toContain('private-fixture-marker')
  expect(JSON.stringify(client.getMutationCache().getAll())).not.toContain('private-fixture-marker')
  expect(JSON.stringify(localStorage)).not.toContain('private-fixture-marker')
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
  expect(screen.queryByDisplayValue('private-fixture-marker')).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('button', { name: 'Test saved connection' }))
  await screen.findByText(/Komodo test: failed/)
  expect(writes).toBe(0); expect(tests).toBe(1)
})

it.each([503, 422])('failed save %s retains input and allows correction without success', async status => {
  komodo()
  server.use(http.put('/api/v1/settings/komodo', () => HttpResponse.json(status === 422 ? { detail: [{ loc: ['body', 'api_url'], msg: 'invalid', type: 'value_error' }] } : {}, { status })))
  mount(); const url = await edit(); await userEvent.clear(url); await userEvent.type(url, 'https://draft.example')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText('Request failed. Review the highlighted fields or retry.')
  expect(url).toHaveValue('https://draft.example')
  expect(screen.queryByText(/Komodo settings saved/)).not.toBeInTheDocument()
  if (status === 422) expect(url).toHaveAttribute('aria-invalid', 'true')
})

it('confirms only canonical readback, not PUT data or the local draft', async () => {
  let saved = false
  komodo(() => HttpResponse.json(saved ? { ...config, api_url: 'https://canonical.example' } : config))
  server.use(http.put('/api/v1/settings/komodo', () => { saved = true; return HttpResponse.json({ ...config, api_url: 'https://put.example' }) }))
  mount(); const url = await edit(); await userEvent.clear(url); await userEvent.type(url, 'https://draft.example')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText('Komodo settings saved; canonical values confirmed.')
  expect(screen.getByText('https://canonical.example')).toBeVisible()
  expect(screen.queryByText('https://put.example')).not.toBeInTheDocument()
})

it.each(['unavailable', 'malformed'])('readback %s never confirms save and preserves correction', async failure => {
  let saved = false
  komodo(() => saved ? failure === 'unavailable' ? new HttpResponse(null, { status: 503 }) : HttpResponse.json({}) : HttpResponse.json(config))
  server.use(http.put('/api/v1/settings/komodo', () => { saved = true; return HttpResponse.json(config) }))
  mount(); const url = await edit(); await userEvent.clear(url); await userEvent.type(url, 'https://draft.example')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText(/Write accepted, but canonical readback is unconfirmed/)
  expect(url).toHaveValue('https://draft.example')
  expect(screen.queryByText('Komodo settings saved; canonical values confirmed.')).not.toBeInTheDocument()
})

it('a later draft revision is not overwritten or declared saved by a delayed readback', async () => {
  let resolve!: () => void, saved = false
  const held = new Promise<void>(r => { resolve = r })
  komodo(async () => { if (saved) await held; return HttpResponse.json(config) })
  server.use(http.put('/api/v1/settings/komodo', () => { saved = true; return HttpResponse.json(config) }))
  mount(); const url = await edit(); await userEvent.clear(url); await userEvent.type(url, 'https://submitted.example')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await waitFor(() => expect(saved).toBe(true))
  await userEvent.clear(url); await userEvent.type(url, 'https://newer.example')
  await act(async () => { resolve() })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save', exact: true })).toBeEnabled())
  expect(url).toHaveValue('https://newer.example')
  expect(screen.queryByText('Komodo settings saved; canonical values confirmed.')).not.toBeInTheDocument()
})

it('session revocation clears credentials and ignores a delayed test', async () => {
  let resolve!: () => void
  const held = new Promise<void>(r => { resolve = r })
  komodo(); server.use(http.post('/api/v1/settings/komodo/test', async () => { await held; return HttpResponse.json({ success: true }) }))
  mount(); await userEvent.click(await screen.findByRole('button', { name: 'Test saved connection' }))
  await edit(); await userEvent.type(screen.getByLabelText('Komodo API secret', { exact: true }), 'private-fixture-marker')
  await act(async () => { clearSession(); resolve() })
  expect(screen.queryByDisplayValue('private-fixture-marker')).not.toBeInTheDocument()
  expect(screen.queryByText(/Komodo test: completed/)).not.toBeInTheDocument()
})

it('dirty navigation can stay for correction or discard and go Back', async () => {
  komodo(); const { router } = mount(); const url = await edit()
  await userEvent.type(url, '/draft')
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  await screen.findByRole('dialog', { name: 'Discard unsaved settings?' })
  await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel', exact: true }))
  expect(url).toHaveValue(config.api_url + '/draft')
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  await userEvent.click(await screen.findByRole('button', { name: 'Discard and leave' }))
  expect(await screen.findByRole('region', { name: 'System settings' })).toBeVisible()
  await act(async () => { await router.navigate(-1) })
  expect(screen.getByRole('region', { name: 'Docker connection settings' })).toHaveFocus()
  expect(await screen.findByRole('button', { name: 'Edit Settings' })).toBeVisible()
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

it('a changed or stale saved configuration invalidates a prior test observation', async () => {
  komodo(); server.use(http.post('/api/v1/settings/komodo/test', () => HttpResponse.json({ success: true })))
  const { client } = mount()
  await userEvent.click(await screen.findByRole('button', { name: 'Test saved connection' }))
  await screen.findByText(/Komodo test: completed/)
  await act(async () => { client.setQueryData(queryKeys.settings.komodo, { ...config, api_url: 'https://changed.example' }) })
  await waitFor(() => expect(screen.queryByText(/Komodo test: completed/)).not.toBeInTheDocument())
})

it('refreshing even unchanged redacted flags invalidates a prior secret-configuration test', async () => {
  komodo(); server.use(http.post('/api/v1/settings/komodo/test', () => HttpResponse.json({ success: true })))
  const { client } = mount()
  await userEvent.click(await screen.findByRole('button', { name: 'Test saved connection' }))
  await screen.findByText(/Komodo test: completed/)
  await client.refetchQueries({ queryKey: queryKeys.settings.komodo })
  await waitFor(() => expect(screen.queryByText(/Komodo test: completed/)).not.toBeInTheDocument())
})

it('401 expires the private editor instead of retaining credentials for another operator', async () => {
  komodo(); onSessionExpired(() => clearSession())
  server.use(http.put('/api/v1/settings/komodo', () => new HttpResponse(null, { status: 401 })))
  mount(); await edit(); await userEvent.type(screen.getByLabelText('Komodo API secret', { exact: true }), 'private-fixture-marker')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await waitFor(() => expect(screen.queryByDisplayValue('private-fixture-marker')).not.toBeInTheDocument())
  expect(screen.queryByText('Komodo settings saved; canonical values confirmed.')).not.toBeInTheDocument()
})

it('cancel of a pending save revokes the old editor result without promising rollback', async () => {
  let resolve!: () => void, submitted = false
  const held = new Promise<void>(r => { resolve = r })
  komodo(); server.use(http.put('/api/v1/settings/komodo', async () => { submitted = true; await held; return HttpResponse.json(config) }))
  mount(); const url = await edit(); await userEvent.type(url, '/draft')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await waitFor(() => expect(submitted).toBe(true))
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
  await act(async () => { resolve() })
  expect(screen.queryByText('Komodo settings saved; canonical values confirmed.')).not.toBeInTheDocument()
  expect(await screen.findByRole('button', { name: 'Edit Settings' })).toBeVisible()
})

it('independent loads retain quota when Komodo is unavailable and disk fallback is not usage', async () => {
  komodo(() => new HttpResponse(null, { status: 503 }))
  server.use(http.get('/api/v1/settings/storage', () => HttpResponse.json({ check_quota_before_upload: true })), http.get('/api/v1/settings/system-info', () => HttpResponse.json({ disk_status: 'unavailable', disk_total: 0, disk_used: 0, disk_free: 0, backup_count: 0, target_count: 0, uptime_seconds: 0, app_version: 'fixture' })))
  mount('storage-safeguards')
  expect(await screen.findByRole('switch')).toHaveAttribute('aria-checked', 'true')
  await userEvent.click(screen.getByRole('link', { name: 'System', exact: true }))
  expect(await screen.findByText('Disk capacity unavailable; fallback numbers are not measured usage.')).toBeVisible()
  expect(screen.queryByRole('meter')).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('link', { name: 'Storage safeguards', exact: true }))
  expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
})

it.each(['connection', 'storage-safeguards'])('a delayed pre-save refresh cannot replace canonical %s readback', async section => {
  let release!: () => void, refreshStarted!: () => void
  const held = new Promise<void>(r => { release = r }), started = new Promise<void>(r => { refreshStarted = r })
  let reads = 0, saved = false
  const komodoMode = section === 'connection'
  const endpoint = komodoMode ? 'komodo' : 'storage'
  const before = komodoMode ? config : { check_quota_before_upload: false }
  const after = komodoMode ? { ...config, api_url: 'https://canonical.example' } : { check_quota_before_upload: true }
  server.use(http.get('/api/v1/settings/' + endpoint, async () => { reads++; if (reads === 2) { refreshStarted(); await held; return HttpResponse.json(before) } return HttpResponse.json(saved ? after : before) }), http.put('/api/v1/settings/' + endpoint, () => { saved = true; return HttpResponse.json(after) }))
  const { client } = mount(section)
  if (komodoMode) { const url = await edit(); await userEvent.clear(url); await userEvent.type(url, 'https://draft.example') }
  else await userEvent.click(await screen.findByRole('switch', { name: 'Check quota before upload' }))
  const key = komodoMode ? queryKeys.settings.komodo : queryKeys.settings.storage
  const refresh = client.refetchQueries({ queryKey: key })
  await started
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText(`${komodoMode ? 'Komodo' : 'Storage'} settings saved; canonical values confirmed.`)
  await act(async () => { release(); await refresh })
  await waitFor(() => expect(client.getQueryData(key)).toEqual(after))
})

it('malformed canonical endpoint cannot be reported as saved', async () => {
  let saved = false
  komodo(() => HttpResponse.json(saved ? { ...config, api_url: 7 } : config))
  server.use(http.put('/api/v1/settings/komodo', () => { saved = true; return HttpResponse.json(config) }))
  mount(); const url = await edit(); await userEvent.type(url, '/draft')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText(/Write accepted, but canonical readback is unconfirmed/)
  expect(url).toHaveValue(config.api_url + '/draft')
})

it('explicit clears send empty strings while blank masked inputs keep credentials', async () => {
  let payload: unknown
  komodo(); server.use(http.put('/api/v1/settings/komodo', async ({ request }) => { payload = await request.json(); return HttpResponse.json(config) }))
  mount(); await edit()
  await userEvent.click(screen.getByRole('switch', { name: 'Enable Komodo integration' }))
  await userEvent.click(screen.getByRole('checkbox', { name: 'Clear API key' }))
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  await screen.findByText('Komodo settings saved; canonical values confirmed.')
  expect(payload).toEqual({ enabled: false, api_key: '' })
})
