import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import { QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import SetupWizard from '../SetupWizard'
import Settings from '../Settings'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../../store/auth'
import { clearSession, queryClient } from '../../store/session'

const user = { id: 1, username: 'operator', is_admin: true, created_at: '', last_login: null }
const counts = { backup_targets: 2, schedules: 1, retention_policies: 1, backups: 3, remote_storages: 1, backup_storage_syncs: 1, backup_schedules: 1, encryption_config: 1, app_settings: 2 }
const preview = { counts, exported_at: '2026-11-09T12:00:00+00:00', storages: [{ id: 1, type: 'ssh', state: 'credentials_required' }], archives_included: false, archive_private_keys_included: false }
const result = { counts, storages: [{ id: 1, type: 'ssh', state: 'credentials_required' }], komodo: { state: 'disabled' }, catalog: [{ id: 1, local: 'unavailable', copies: [{ storage_id: 1, state: 'offline' }] }], archives_modified: false }
function mount(element: React.ReactNode, entry = '/') { return render(<QueryClientProvider client={queryClient}><MemoryRouter initialEntries={[entry]}>{element}</MemoryRouter></QueryClientProvider>) }
beforeEach(() => { clearSession(); useAuthStore.setState({ user, isAuthenticated: true, isLoading: false, setupRequired: false }); server.use(http.get('/api/v1/recovery/status', () => HttpResponse.json(null))) })
afterEach(() => { cleanup(); clearSession(); vi.restoreAllMocks() })
async function chooseRestore() {
  mount(<SetupWizard />)
  await userEvent.click(screen.getByRole('button', { name: 'Restore system backup' }))
  await userEvent.upload(screen.getByLabelText('System backup file'), new File(['{"encrypted":"synthetic-envelope"}'], 'fixture.dockervault', { type: 'application/octet-stream' }))
  await waitFor(() => expect(screen.getByLabelText('System-backup password')).toBeEnabled())
  await userEvent.type(screen.getByLabelText('System-backup password'), 'synthetic-bundle-password')
}
it('requires an explicit Create New versus Restore choice before administrator creation', async () => {
  mount(<SetupWizard />)
  expect(screen.queryByLabelText('Username')).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Restore system backup' })).toBeEnabled()
  await userEvent.click(screen.getByRole('button', { name: 'Create new installation' }))
  expect(screen.getByLabelText('Username')).toBeInTheDocument()
  expect(screen.getByRole('heading', { name: 'Welcome to DockerVault' })).toHaveFocus()
})
it('validates a redacted preview before a single restore and shows real non-success connection states', async () => {
  let restores = 0, accountRequests = 0
  server.use(
    http.post('/api/v1/auth/setup', () => { accountRequests++; return HttpResponse.json({ user }) }),
    http.post('/api/v1/recovery/preview', async ({ request }) => { expect(await request.json()).toMatchObject({ password: 'synthetic-bundle-password' }); return HttpResponse.json(preview) }),
    http.post('/api/v1/recovery/restore', async ({ request }) => { restores++; expect(await request.json()).toMatchObject({ username: 'replacement', admin_password: 'synthetic-admin-password', confirm_admin_password: 'synthetic-admin-password' }); return HttpResponse.json({ user, result }) }),
  )
  await chooseRestore()
  expect(screen.queryByLabelText('New administrator username')).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('button', { name: 'Validate and preview' }))
  await screen.findByRole('heading', { name: 'Validated preview — not restored' })
  expect(screen.getByText(/Credentials required/)).toBeInTheDocument()
  await userEvent.type(screen.getByLabelText('New administrator username'), 'replacement')
  await userEvent.type(screen.getByLabelText('New account password'), 'synthetic-admin-password')
  await userEvent.type(screen.getByLabelText('Confirm new account password'), 'synthetic-admin-password')
  await userEvent.dblClick(screen.getByRole('button', { name: 'Restore configuration and create administrator' }))
  await screen.findByRole('heading', { name: 'Configuration restored' })
  expect(screen.getByText(/Unavailable/)).toBeInTheDocument()
  expect(screen.getByText(/Offline/)).toBeInTheDocument()
  expect(accountRequests).toBe(0)
  expect(restores).toBe(1)
  expect(screen.queryByLabelText('System-backup password')).not.toBeInTheDocument()
  expect(JSON.stringify(queryClient.getQueryCache().getAll())).not.toContain('synthetic-bundle-password')
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
  expect(localStorage.length).toBe(0)
  expect(sessionStorage.length).toBe(0)
})
it('clears an incorrect bundle password and never presents a failed activation as restored', async () => {
  server.use(http.post('/api/v1/recovery/preview', () => HttpResponse.json({ detail: 'synthetic-must-not-echo' }, { status: 400 })))
  await chooseRestore()
  await userEvent.click(screen.getByRole('button', { name: 'Validate and preview' }))
  await screen.findByRole('alert')
  expect(screen.getByLabelText('System-backup password')).toHaveValue('')
  expect(document.body.textContent).not.toContain('synthetic-must-not-echo')
  expect(screen.queryByRole('heading', { name: 'Configuration restored' })).not.toBeInTheDocument()
})
it('exports only after password confirmation, clears secrets and never caches mutation variables', async () => {
  let requests = 0
  server.use(http.post('/api/v1/recovery/export', async ({ request }) => { requests++; expect(await request.json()).toEqual({ password: 'synthetic-export-password', confirm_password: 'synthetic-export-password' }); return new HttpResponse('synthetic-encrypted-bundle', { headers: { 'Content-Type': 'application/octet-stream', 'Cache-Control': 'no-store' } }) }))
  const objectURL = vi.fn(() => 'blob:synthetic-download'), revoke = vi.fn()
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: objectURL, revokeObjectURL: revoke }))
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined)
  mount(<Settings />, '/settings#system-recovery')
  await userEvent.type(screen.getByLabelText('System-backup password'), 'synthetic-export-password')
  await userEvent.type(screen.getByLabelText('Confirm system-backup password'), 'synthetic-export-password')
  await userEvent.click(screen.getByRole('button', { name: 'Download encrypted system backup' }))
  await waitFor(() => expect(objectURL).toHaveBeenCalledTimes(1))
  expect(screen.getByLabelText('System-backup password')).toHaveValue('')
  expect(screen.getByLabelText('Confirm system-backup password')).toHaveValue('')
  expect(requests).toBe(1)
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
  expect(revoke).toHaveBeenCalledWith('blob:synthetic-download')
})
it('shows imported SSH public references without pretending private key custody or offering installation', async () => {
  server.use(http.get('/api/v1/recovery/status', () => HttpResponse.json({ ...result, ssh_public_keys: [{ name: 'original', public_key: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIC7G8BfNXfsH4jQXzdy06W3u7i3BPlMrZ1E9QXuTLfPb' }] })))
  mount(<Settings />, '/settings#system-recovery')
  expect(await screen.findByRole('heading', { name: 'SSH public reconnect references' })).toBeInTheDocument()
  expect(screen.getByLabelText('SSH public reference original')).toHaveAttribute('tabindex', '0')
  expect(screen.queryByRole('button', { name: /install/i })).not.toBeInTheDocument()
})

it.each([400, 403, 409, 500])('never confirms activation on HTTP %s and clears all submitted passwords', async status => {
  server.use(http.post('/api/v1/recovery/preview', () => HttpResponse.json(preview)), http.post('/api/v1/recovery/restore', () => HttpResponse.json({ detail: 'must-not-render-server-detail' }, { status })))
  await chooseRestore()
  await userEvent.click(screen.getByRole('button', { name: 'Validate and preview' }))
  await screen.findByRole('heading', { name: 'Validated preview — not restored' })
  await userEvent.type(screen.getByLabelText('New administrator username'), 'replacement')
  await userEvent.type(screen.getByLabelText('New account password'), 'synthetic-admin-password')
  await userEvent.type(screen.getByLabelText('Confirm new account password'), 'synthetic-admin-password')
  await userEvent.click(screen.getByRole('button', { name: 'Restore configuration and create administrator' }))
  await screen.findByRole('alert')
  expect(screen.getByLabelText('System-backup password')).toHaveValue('')
  expect(screen.queryByLabelText('New account password')).not.toBeInTheDocument()
  expect(screen.queryByRole('heading', { name: 'Configuration restored' })).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Check setup status' })).toBeEnabled()
  expect(document.body.textContent).not.toContain('must-not-render-server-detail')
  expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
})

it('does not offer export to a non-administrator', () => {
  useAuthStore.setState({ user: { ...user, is_admin: false } })
  mount(<Settings />, '/settings#system-recovery')
  expect(screen.queryByRole('button', { name: 'Download encrypted system backup' })).not.toBeInTheDocument()
  expect(screen.getByText(/Only an authenticated administrator/)).toBeVisible()
})

it('does not download a late export response after session invalidation', async () => {
  let release!: () => void, started!: () => void
  const gate = new Promise<void>(resolve => { release = resolve }), ready = new Promise<void>(resolve => { started = resolve })
  const download = vi.fn(() => 'blob:late')
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: download, revokeObjectURL: vi.fn() }))
  server.use(http.post('/api/v1/recovery/export', async () => { started(); await gate; return new HttpResponse('encrypted') }))
  mount(<Settings />, '/settings#system-recovery')
  await userEvent.type(screen.getByLabelText('System-backup password'), 'synthetic-export-password')
  await userEvent.type(screen.getByLabelText('Confirm system-backup password'), 'synthetic-export-password')
  await userEvent.click(screen.getByRole('button', { name: 'Download encrypted system backup' }))
  await ready
  act(() => clearSession())
  release()
  await act(async () => { await gate })
  expect(download).not.toHaveBeenCalled()
})
