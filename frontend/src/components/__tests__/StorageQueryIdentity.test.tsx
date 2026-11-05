import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, expect, it, vi } from 'vitest'
import { http, HttpResponse } from 'msw'
import BackupEditDialog from '../BackupEditDialog'
import BackupWizard from '../BackupWizard'
import { server } from '../../test/mocks/server'
import type { BackupTarget } from '../../api'

const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()) })
const target = { id: 917, name: 'Synthetic plan', target_type: 'path', host_path: '/source', enabled: true, dependencies: [], selected_volumes: [], include_paths: [], exclude_paths: [], per_volume_rules: {}, remote_storage_ids: [] } as unknown as BackupTarget
it('the private plan editor keeps input and associates only safe validation after a failed save', async () => {
  server.use(http.get('/api/v1/targets/917', () => HttpResponse.json(target)), http.put('/api/v1/targets/917', () => HttpResponse.json({ detail: [{ loc: ['body', 'name'], type: 'value_error', msg: 'PRIVATE COMMAND', input: 'PRIVATE COMMAND' }] }, { status: 422 })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(<QueryClientProvider client={client}><BackupEditDialog isOpen target={target} onClose={vi.fn()} /></QueryClientProvider>)
  const input = screen.getByRole('textbox', { name: 'Name' })
  await userEvent.clear(input); await userEvent.type(input, 'Retained plan input')
  await userEvent.click(screen.getByRole('button', { name: 'Save Changes' }))
  await waitFor(() => expect(input).toHaveAttribute('aria-invalid', 'true'))
  expect(input).toHaveAccessibleDescription(/check this value/i)
  expect(input).toHaveValue('Retained plan input')
  expect(screen.queryByText('PRIVATE COMMAND')).not.toBeInTheDocument()
})
it.each([false, true])('failed storage discovery is not silent empty in an open editor (wizard=%s)', async wizard => {
  let failed = true
  server.use(http.get('/api/v1/storage', () => failed ? new HttpResponse(null, { status: 503 }) : HttpResponse.json([])), http.get('/api/v1/targets/917', () => HttpResponse.json(target)))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(<QueryClientProvider client={client}>{wizard ? <BackupWizard isOpen editTarget={target} onClose={vi.fn()} /> : <BackupEditDialog isOpen target={target} onClose={vi.fn()} />}</QueryClientProvider>)
  expect(await screen.findByRole('alert', { name: 'Storage choices status' })).toHaveTextContent(/unavailable/i)
  failed = false
  await userEvent.click(screen.getByRole('button', { name: 'Retry Storage choices' }))
  await waitFor(() => expect(screen.queryByRole('alert', { name: 'Storage choices status' })).not.toBeInTheDocument())
})
it.each([false, true])('storage mutation refreshes the mounted editor (wizard=%s)', async wizard => {
  let revision = 1
  server.use(http.get('/api/v1/storage', () => HttpResponse.json([{ id: revision, name: `Storage ${revision}`, storage_type: 'local', enabled: true, base_path: '/synthetic' } ])), http.get('/api/v1/targets/917', () => HttpResponse.json(target)))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  clients.push(client)
  render(<QueryClientProvider client={client}>{wizard ? <BackupWizard isOpen editTarget={target} onClose={vi.fn()} /> : <BackupEditDialog isOpen target={target} onClose={vi.fn()} />}</QueryClientProvider>)
  const storageData = () => client.getQueryCache().getAll().map(q => q.state.data).find(data => Array.isArray(data) && data.some(item => item?.name?.startsWith('Storage '))) as { id: number }[] | undefined
  await waitFor(() => expect(storageData()?.[0].id).toBe(1))
  revision = 2
  await client.invalidateQueries({ queryKey: ['storages'] })
  await waitFor(() => expect(storageData()?.[0].id).toBe(2))
})
