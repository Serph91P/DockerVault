import { render, screen, cleanup, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import { NotificationCenter } from '../NotificationCenter'
import { bindNotificationOwner, resetNotificationOwner } from '../../store/notifications'

afterEach(() => { cleanup(); resetNotificationOwner() })
it('global operations reads canonical jobs and exposes final per-item outcomes with a revisitable link', async () => {
  bindNotificationOwner(1)
  const id = 'b658c36b-4872-4364-aafc-72b227142815'
  server.use(http.get('/api/v1/operations', () => HttpResponse.json([{ id, action: 'storage_delete', storage_id: 'local', path: '', state: 'completed', stage: 'completed', total: 2, completed: 2, succeeded: 1, failed: 1, skipped: 0, unknown: 0, cancel_requested: false, created_at: '2026-11-01T00:00:00Z', updated_at: '2026-11-01T01:00:00Z', finished_at: '2026-11-01T01:00:00Z', parent_id: null, items: [{ path: 'synthetic-one', state: 'succeeded', error: null }, { path: 'synthetic-two', state: 'failed', error: 'synthetic failure' }] }])))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const view = render(<QueryClientProvider client={client}><MemoryRouter><NotificationCenter isOpen onClose={() => undefined} /></MemoryRouter></QueryClientProvider>)
  await waitFor(() => expect(screen.getByText(/1 succeeded, 1 failed/)).toBeInTheDocument())
  expect(screen.getByRole('link', { name: `Open operation #${id}` })).toHaveAttribute('href', `/storage?operation=${id}&storage=local`)
  expect(screen.getByText('Storage operation needs attention')).toBeInTheDocument()
  expect(screen.queryByRole('button', { name: /resume/i })).not.toBeInTheDocument()
  view.unmount(); client.clear()
})
