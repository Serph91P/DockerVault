import { render, screen, within, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, it, expect, vi } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import StorageBrowser from '../StorageBrowser'
import { clearSession } from '../../store/session'

const storage = { id: 'local' as const, name: 'Synthetic', storage_type: 'local', base_path: '/backups' }
const directory = (name: string) => ({ name, size: null, is_dir: true, modified: 1 })

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const onClose = vi.fn()
  const view = render(<QueryClientProvider client={client}><StorageBrowser storage={storage} onClose={onClose} /></QueryClientProvider>)
  return { client, onClose, ...view }
}

describe('separate directory sizes', () => {
  beforeEach(() => server.use(http.get('/api/v1/operations', () => HttpResponse.json([]))))
  it('lists before size completion, formats bytes and does not treat unavailable as zero', async () => {
    let finish!: () => void
    const gate = new Promise<void>(resolve => { finish = resolve })
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: [directory('backup'), directory('locked'), directory('empty')], path: '' })),
      http.get('/api/v1/storage/local/files/size', async ({ request }) => {
        const path = new URL(request.url).searchParams.get('path')
        if (path === 'backup') await gate
        return HttpResponse.json({ path, state: path === 'locked' ? 'unavailable' : 'completed', bytes: path === 'locked' ? null : path === 'empty' ? 0 : 1536 })
      }),
    )
    mount()
    const row = await screen.findByRole('row', { name: /backup/ })
    expect(within(row).getByText('Loading size...')).toBeInTheDocument()
    expect(await screen.findByText('Unavailable')).toBeInTheDocument()
    expect(await screen.findByText('0 B')).toBeInTheDocument()
    finish()
    expect(await screen.findByText('1.5 KB')).toBeInTheDocument()
  })

  it('cancels old navigation sizes and never overwrites a same-name new row', async () => {
    let finish!: () => void
    const gate = new Promise<void>(resolve => { finish = resolve })
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: [directory('backup')], path: '' })),
      http.get('/api/v1/storage/local/files/size', async ({ request }) => {
        const path = new URL(request.url).searchParams.get('path')
        if (path === 'backup') await gate
        return HttpResponse.json({ path, state: 'completed', bytes: path === 'backup' ? 999 : 10 })
      }),
    )
    const { client } = mount()
    await userEvent.click(await screen.findByRole('row', { name: /backup/ }))
    expect(await screen.findByText('10.0 B')).toBeInTheDocument()
    finish()
    await waitFor(() => expect(client.getQueryCache().findAll({ queryKey: ['storage-size', 'local', '/backups', 'backup'] })).toHaveLength(0))
    expect(screen.queryByText('999.0 B')).not.toBeInTheDocument()
    expect(screen.getByText('10.0 B')).toBeInTheDocument()
  })

  it('refreshes directory sizes after deletion and leaves the browser open', async () => {
    let deleted = false
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: deleted ? [] : [directory('backup')], path: '' })),
      http.get('/api/v1/storage/local/files/size', () => HttpResponse.json({ path: 'backup', state: 'completed', bytes: 20 })),
      http.post('/api/v1/operations', () => { deleted = true; return HttpResponse.json({ operation_id: 'completed-size-refresh' }, { status: 202 }) }),
      http.get('/api/v1/operations', () => HttpResponse.json(deleted ? [{ id: 'completed-size-refresh', action: 'storage_delete', storage_id: 'local', path: '', state: 'completed', stage: 'finished', total: 1, completed: 1, succeeded: 1, failed: 0, skipped: 0, unknown: 0, cancel_requested: false, parent_id: null, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', finished_at: '2026-01-01T00:00:00Z', items: [{ path: 'backup', state: 'succeeded', error: null }] }] : [])),
    )
    const { onClose } = mount()
    const user = userEvent.setup()
    const row = await screen.findByRole('row', { name: /backup/ })
    await user.click(within(row).getByTitle('Delete'))
    const confirm = screen.getByRole('dialog', { name: 'Delete File' })
    await user.click(within(confirm).getByRole('button', { name: 'Delete', exact: true }))
    await waitFor(() => expect(screen.queryByRole('row', { name: /backup/ })).not.toBeInTheDocument())
    expect(screen.getByRole('dialog', { name: 'Browse Synthetic' })).toBeInTheDocument()
    expect(onClose).not.toHaveBeenCalled()
  })

  it.each(['storage', 'session'])('does not accept late sizes after a %s switch', async (switchKind) => {
    let finish!: () => void
    let started!: () => void
    const gate = new Promise<void>(resolve => { finish = resolve })
    const entered = new Promise<void>(resolve => { started = resolve })
    let sizeCalls = 0
    server.use(
      http.get('/api/v1/storage/:id/files', () => HttpResponse.json({ files: [directory('backup')], path: '' })),
      http.get('/api/v1/storage/:id/files/size', async ({ request }) => {
        const call = ++sizeCalls
        if (call === 1) { started(); await gate }
        return HttpResponse.json({ path: new URL(request.url).searchParams.get('path'), state: 'completed', bytes: call === 1 ? 999 : 12 })
      }),
    )
    const { client, rerender, onClose, unmount } = mount()
    await entered
    if (switchKind === 'session') {
      unmount()
      clearSession()
      client.clear()
      render(<QueryClientProvider client={client}><StorageBrowser storage={storage} onClose={onClose} /></QueryClientProvider>)
    } else {
      rerender(<QueryClientProvider client={client}><StorageBrowser storage={{ ...storage, id: 7 }} onClose={onClose} /></QueryClientProvider>)
    }
    expect(await screen.findByText('12.0 B')).toBeInTheDocument()
    finish()
    await waitFor(() => expect(client.getQueryCache().findAll({ queryKey: ['storage-size'] })).toHaveLength(1))
    expect(screen.queryByText('999.0 B')).not.toBeInTheDocument()
    expect(screen.getByText('12.0 B')).toBeInTheDocument()
  })

  it('re-requests sizes on refresh and renders transport failure as unavailable', async () => {
    let calls = 0
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: [directory('backup')], path: '' })),
      http.get('/api/v1/storage/local/files/size', () => ++calls === 1 ? HttpResponse.json({ path: 'backup', state: 'completed', bytes: 21 }) : new HttpResponse(null, { status: 503 })),
    )
    mount()
    expect(await screen.findByText('21.0 B')).toBeInTheDocument()
    await userEvent.click(screen.getByTitle('Refresh'))
    expect(await screen.findByText('Unavailable')).toBeInTheDocument()
    expect(screen.queryByText('21.0 B')).not.toBeInTheDocument()
  })
})
