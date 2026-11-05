import { render, screen, within, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import StorageBrowser from '../StorageBrowser'
import { clearSession } from '../../store/session'

const storage = { id: 'local' as const, name: 'Synthetic', storage_type: 'local', base_path: '/backups' }
const files = [{ name: 'folder', is_dir: true, size: null, modified: 1 }, { name: 'file', is_dir: false, size: 3, modified: 1 }]
const operation = (changes = {}) => ({ id: 'durable-one', action: 'storage_delete', storage_id: 'local', path: '', state: 'running', stage: 'deleting', total: 2, completed: 0, succeeded: 0, failed: 0, skipped: 0, unknown: 0, cancel_requested: false, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', finished_at: null, parent_id: null, items: [{ path: 'folder', state: 'running', error: null }, { path: 'file', state: 'pending', error: null }], ...changes })
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const onClose = vi.fn()
  return { client, onClose, ...render(<QueryClientProvider client={client}><StorageBrowser storage={storage} onClose={onClose} /></QueryClientProvider>) }
}
function listings(jobs: () => unknown[] = () => []) {
  server.use(
    http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files, path: '' })),
    http.get('/api/v1/storage/local/files/size', () => HttpResponse.json({ state: 'completed', bytes: 5 })),
    http.get('/api/v1/operations', () => HttpResponse.json(jobs())),
  )
}

describe('durable storage browser', () => {
  it('offers visible select all, native mixed state, clear and directory-scoped count', async () => {
    listings()
    mount()
    const user = userEvent.setup()
    await screen.findByRole('row', { name: /folder/ })
    await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
    expect(screen.getByText('2 selected')).toBeVisible()
    expect(screen.getByRole('checkbox', { name: 'Select all entries' })).toBeChecked()
    await user.click(screen.getByRole('row', { name: /file/ }))
    expect(screen.getByText('1 selected')).toBeVisible()
    expect(screen.getByRole('checkbox', { name: 'Select all entries' })).toBePartiallyChecked()
    await user.click(screen.getByRole('button', { name: 'Clear Selection' }))
    expect(screen.getByText('0 selected')).toBeVisible()
    await user.click(screen.getByTitle('Select files'))
    await user.click(screen.getByRole('row', { name: /folder/ }))
    expect(screen.getByText('0 selected')).toBeVisible()
  })

  it('starts once, preserves panel/path/mode and displays authoritative mixed results', async () => {
    let jobs: unknown[] = []
    const requests: Record<string, unknown>[] = []
    listings(() => jobs)
    server.use(http.post('/api/v1/operations', async ({ request }) => {
      requests.push(await request.json() as Record<string, unknown>)
      jobs = [operation({ state: 'failed', completed: 2, succeeded: 1, failed: 1, items: [{ path: 'folder', state: 'succeeded', error: null }, { path: 'file', state: 'failed', error: 'Permission denied' }] })]
      return HttpResponse.json({ operation_id: 'durable-one' }, { status: 202 })
    }))
    const { onClose } = mount()
    const user = userEvent.setup()
    await screen.findByRole('row', { name: /folder/ })
    await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
    await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
    const confirmation = screen.getByRole('dialog', { name: 'Delete Files' })
    await user.dblClick(within(confirmation).getByRole('button', { name: 'Delete', exact: true }))
    expect(await screen.findByText('Permission denied')).toBeVisible()
    expect(screen.getByText(/1 succeeded.*1 failed/)).toBeVisible()
    expect(screen.getByRole('button', { name: 'Retry failed items' })).toBeEnabled()
    expect(screen.getByRole('dialog', { name: 'Browse Synthetic' })).toBeVisible()
    expect(screen.getByText('0 selected')).toBeVisible()
    expect(requests).toHaveLength(1)
    expect(requests[0]).toMatchObject({ action: 'storage_delete', storage_id: 'local', path: '', paths: ['folder', 'file'] })
    expect(requests[0].request_id).toMatch(/^[\da-f-]{36}$/)
    expect(onClose).not.toHaveBeenCalled()
  })

  it('rediscovers a running job on reopen without resubmission and requests cancel', async () => {
    let job = operation()
    let posts = 0
    listings(() => [job])
    server.use(
      http.post('/api/v1/operations', () => { posts++; return HttpResponse.json({}, { status: 500 }) }),
      http.post('/api/v1/operations/durable-one/cancel', () => { job = operation({ cancel_requested: true }); return HttpResponse.json(job) }),
    )
    const { unmount } = mount()
    expect(await screen.findByText('durable-one')).toBeVisible()
    expect(screen.getByRole('progressbar', { name: 'Completed entries' })).toHaveAttribute('value', '0')
    expect(screen.getByText(/Recursive work.*indeterminate/)).toBeVisible()
    expect(screen.getByRole('progressbar', { name: 'Current deletion (indeterminate)' })).not.toHaveAttribute('value')
    unmount()
    mount()
    await userEvent.click(await screen.findByRole('button', { name: 'Cancel operation' }))
    expect(await screen.findByText(/Cancellation requested.*not rollback/)).toBeVisible()
    expect(posts).toBe(0)
  })

  it('does not present an old terminal result as current when refresh fails', async () => {
    listings(() => [operation({ state: 'completed', completed: 2, succeeded: 2, items: [{ path: 'folder', state: 'succeeded', error: null }, { path: 'file', state: 'succeeded', error: null }] })])
    mount()
    await screen.findByText('durable-one')
    server.use(http.get('/api/v1/operations', () => HttpResponse.json({}, { status: 503 })))
    await userEvent.click(screen.getByRole('button', { name: 'Refresh operations' }))
    expect(await screen.findByRole('alert', {}, { timeout: 6000 })).toHaveTextContent(/Operation status unavailable.*stale/)
  })

  it('reuses the failed-only retry request ID after a lost acceptance response', async () => {
    listings(() => [operation({ state: 'failed', completed: 2, succeeded: 1, failed: 1, items: [{ path: 'folder', state: 'succeeded', error: null }, { path: 'file', state: 'failed', error: 'Permission denied' }] })])
    const requests: { request_id: string }[] = []
    server.use(http.post('/api/v1/operations/durable-one/retry', async ({ request }) => {
      requests.push(await request.json() as { request_id: string })
      return HttpResponse.error()
    }))
    mount()
    const user = userEvent.setup()
    await user.click(await screen.findByRole('button', { name: 'Retry failed items' }))
    await screen.findByText(/Operation request failed/)
    await user.click(screen.getByRole('button', { name: 'Retry failed items' }))
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1].request_id).toBe(requests[0].request_id)
  })

  it('does not create a second retry intent after an accepted fast response', async () => {
    listings(() => [operation({ state: 'failed', completed: 2, succeeded: 1, failed: 1, items: [{ path: 'folder', state: 'succeeded', error: null }, { path: 'file', state: 'failed', error: 'Synthetic failure' }] })])
    const requests: { request_id: string }[] = []
    server.use(http.post('/api/v1/operations/durable-one/retry', async ({ request }) => {
      requests.push(await request.json() as { request_id: string })
      return HttpResponse.json({ operation_id: 'same-child' }, { status: 202 })
    }))
    mount()
    const user = userEvent.setup()
    await user.click(await screen.findByRole('button', { name: 'Retry failed items' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Retry failed items' })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: 'Retry failed items' }))
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1].request_id).toBe(requests[0].request_id)
  })

  it('shows the existing child on reopen rather than offering the original failed scope again', async () => {
    listings(() => [operation({ state: 'failed', completed: 2, succeeded: 1, failed: 1, items: [{ path: 'folder', state: 'succeeded', error: null }, { path: 'file', state: 'failed', error: 'Synthetic failure' }] }), operation({ id: 'finished-child', parent_id: 'durable-one', state: 'completed', total: 1, completed: 1, succeeded: 1, items: [{ path: 'file', state: 'succeeded', error: null }] })])
    mount()
    await screen.findByText('finished-child')
    expect(within(screen.getByRole('article', { name: 'Operation durable-one' })).queryByRole('button', { name: 'Retry failed items' })).not.toBeInTheDocument()
  })

  it.each([422, 409, 429])('allows a corrected new intent after definitive HTTP %s rejection', async status => {
    listings()
    const requests: { request_id: string; paths: string[] }[] = []
    server.use(http.post('/api/v1/operations', async ({ request }) => {
      requests.push(await request.json() as typeof requests[number])
      return requests.length === 1
        ? HttpResponse.json({ detail: 'Request not admitted' }, { status })
        : HttpResponse.json({ operation_id: 'corrected' }, { status: 202 })
    }))
    const { onClose } = mount()
    const user = userEvent.setup()
    await screen.findByRole('row', { name: /folder/ })
    await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
    await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
    await user.click(within(screen.getByRole('dialog', { name: 'Delete Files' })).getByRole('button', { name: 'Delete', exact: true }))
    await screen.findByText(/Operation request failed/)
    await user.click(screen.getByRole('row', { name: /file/ }))
    expect(screen.getByText('1 selected')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Delete', exact: true })).toBeEnabled()
    expect(screen.queryByRole('button', { name: 'Retry request with same ID' })).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
    await user.click(within(screen.getByRole('dialog', { name: 'Delete Files' })).getByRole('button', { name: 'Delete', exact: true }))
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1].paths).toEqual(['folder'])
    expect(requests[1].request_id).not.toBe(requests[0].request_id)
    expect(onClose).not.toHaveBeenCalled()
  })

  it.each(['network', 500, 408])('preserves the exact original intent after ambiguous %s acceptance', async status => {
    listings()
    const requests: unknown[] = []
    server.use(http.post('/api/v1/operations', async ({ request }) => {
      requests.push(await request.json())
      return status === 'network' ? HttpResponse.error() : HttpResponse.json({}, { status: status as number })
    }))
    mount()
    const user = userEvent.setup()
    await screen.findByRole('row', { name: /folder/ })
    await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
    await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
    await user.click(within(screen.getByRole('dialog', { name: 'Delete Files' })).getByRole('button', { name: 'Delete', exact: true }))
    await screen.findByText(/Operation request failed/)
    await user.click(screen.getByRole('row', { name: /file/ }))
    expect(screen.getByRole('button', { name: 'Delete', exact: true })).toBeDisabled()
    await user.click(screen.getByRole('button', { name: 'Retry request with same ID' }))
    await waitFor(() => expect(requests).toHaveLength(2))
    expect(requests[1]).toEqual(requests[0])
  })

  it('does not report an initial pending list as confirmed empty', async () => {
    let release!: () => void
    const held = new Promise<void>(resolve => { release = resolve })
    listings()
    server.use(http.get('/api/v1/operations', async () => { await held; return HttpResponse.json([]) }))
    const { unmount } = mount()
    try {
      await screen.findByRole('row', { name: /folder/ })
      expect(screen.getByText('Loading storage operations...')).toBeVisible()
      expect(screen.queryByText('No recent storage operations')).not.toBeInTheDocument()
      release()
      expect(await screen.findByText('No recent storage operations')).toBeVisible()
    } finally { release(); unmount() }
  })

  it('ignores accepted late responses after logout and clears private operation UI', async () => {
    let release!: () => void
    const held = new Promise<void>(resolve => { release = resolve })
    listings()
    server.use(http.post('/api/v1/operations', async () => { await held; return HttpResponse.json({ operation_id: 'late-user-one' }, { status: 202 }) }))
    mount()
    const user = userEvent.setup()
    await screen.findByRole('row', { name: /folder/ })
    await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
    await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
    await user.click(within(screen.getByRole('dialog', { name: 'Delete Files' })).getByRole('button', { name: 'Delete', exact: true }))
    clearSession()
    release()
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.queryByText('late-user-one')).not.toBeInTheDocument()
  })
})
