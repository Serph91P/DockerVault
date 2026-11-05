import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Storage from '../Storage'
import { queryKeys } from '../../api/queryKeys'

afterEach(() => { cleanup(); localStorage.removeItem('storage-test-results') })
it('a new backend configuration revision discards a previous successful provider measurement', async () => {
  const storage = { id: 982, name: 'Synthetic storage', storage_type: 'local', enabled: true, base_path: '/synthetic', updated_at: '2026-01-01T00:00:00Z' }
  server.use(http.get('/api/v1/storage', () => HttpResponse.json([storage])), http.post('/api/v1/storage/982/test', () => HttpResponse.json({ success: true })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const view = render(<QueryClientProvider client={client}><MemoryRouter><Storage /></MemoryRouter></QueryClientProvider>)
  await screen.findByText('Synthetic storage')
  await userEvent.click(screen.getByRole('button', { name: 'Test Connection' }))
  await screen.findByText('Connected', { exact: true })
  act(() => client.setQueryData(queryKeys.storages.list(), [{ ...storage, updated_at: '2026-01-02T00:00:00Z' }]))
  await waitFor(() => expect(screen.queryByText('Connected', { exact: true })).not.toBeInTheDocument())
  view.unmount(); client.clear()
})
it.each([[{}, 'Unknown'], [{ success: true, warnings: ['private provider diagnostic'] }, 'Partial']])('provider result %j cannot default to successful connection', async (body, label) => {
  localStorage.setItem('storage-test-results', JSON.stringify({ 982: { success: true, message: 'OLD OWNER', testedAt: new Date().toISOString() } }))
  server.use(http.get('/api/v1/storage', () => HttpResponse.json([{ id: 982, name: 'Synthetic storage', storage_type: 'local', enabled: true, base_path: '/synthetic' }])), http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([])), http.post('/api/v1/storage/982/test', () => HttpResponse.json(body)))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const view = render(<QueryClientProvider client={client}><MemoryRouter><Storage /></MemoryRouter></QueryClientProvider>)
  await screen.findByText('Synthetic storage')
  expect(screen.queryByText('OLD OWNER')).not.toBeInTheDocument()
  expect(screen.queryByText('Connected')).not.toBeInTheDocument()
  await userEvent.click(screen.getByRole('button', { name: 'Test Connection' }))
  expect(await screen.findByText(label, { exact: true })).toBeInTheDocument()
  expect(screen.queryByText('Connected')).not.toBeInTheDocument()
  expect(localStorage.getItem('storage-test-results')).toBeNull()
  view.unmount(); client.clear()
})
