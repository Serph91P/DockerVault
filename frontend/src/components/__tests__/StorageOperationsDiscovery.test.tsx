import { render, screen, within, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { describe, it, expect } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import StorageBrowser from '../StorageBrowser'
import { sessionGeneration } from '../../store/session'

const job = { id: 'accepted-during-initial-read', action: 'storage_delete', storage_id: 'local', path: '', state: 'completed', stage: 'finished', total: 1, completed: 1, succeeded: 1, failed: 0, skipped: 0, unknown: 0, cancel_requested: false, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', finished_at: '2026-01-01T00:00:00Z', parent_id: null, items: [{ path: 'file', state: 'succeeded', error: null }] }

describe('acceptance overlapping operation discovery without realtime', () => {
  it('rebinds an accepted retry child instead of retaining an older parent-only snapshot', async () => {
    const parent = { ...job, id: 'failed-parent', state: 'failed', succeeded: 0, failed: 1, items: [{ path: 'file', state: 'failed', error: 'Synthetic failure' }] }
    const child = { ...job, id: 'retry-child', parent_id: parent.id }
    let release!: () => void
    const held = new Promise<void>(resolve => { release = resolve })
    let reads = 0, retries = 0
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: [], path: '' })),
      http.get('/api/v1/operations', async () => {
        reads++
        const snapshot = retries ? [parent, child] : [parent]
        if (reads === 1) await held
        return HttpResponse.json(snapshot)
      }),
      http.post('/api/v1/operations/failed-parent/retry', () => { retries++; return HttpResponse.json({ operation_id: child.id }, { status: 202 }) }),
    )
    const client = new QueryClient()
    client.setQueryData(['storage-operations', 'local', sessionGeneration()], [parent])
    const mounted = render(<QueryClientProvider client={client}><StorageBrowser storage={{ id: 'local', name: 'Synthetic', storage_type: 'local', base_path: '/backups' }} onClose={() => undefined} /></QueryClientProvider>)
    try {
      await waitFor(() => expect(reads).toBe(1))
      await userEvent.click(await screen.findByRole('button', { name: 'Retry failed items' }))
      await screen.findByText(child.id)
      release()
      expect(within(screen.getByRole('article', { name: 'Operation failed-parent' })).queryByRole('button', { name: 'Retry failed items' })).not.toBeInTheDocument()
      expect(retries).toBe(1)
      expect(reads).toBeGreaterThanOrEqual(2)
    } finally { release(); mounted.unmount(); client.clear() }
  })
  it.each(['completed', 'running'])('discovers an accepted %s job despite an older empty initial response', async state => {
    let current = { ...job, state, completed: state === 'running' ? 0 : 1, succeeded: state === 'running' ? 0 : 1, items: [{ path: 'file', state: state === 'running' ? 'running' : 'succeeded', error: null }] }
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    let reads = 0, creates = 0
    server.use(
      http.get('/api/v1/storage/local/files', () => HttpResponse.json({ files: [{ name: 'file', is_dir: false, size: 1, modified: 1 }], path: '' })),
      http.get('/api/v1/operations', async () => {
        reads++
        const snapshot = creates ? [current] : []
        if (reads === 1) await gate
        return HttpResponse.json(snapshot)
      }),
      http.post('/api/v1/operations', () => { creates++; return HttpResponse.json({ operation_id: job.id }, { status: 202 }) }),
    )
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    const mounted = render(<QueryClientProvider client={client}><StorageBrowser storage={{ id: 'local', name: 'Synthetic', storage_type: 'local', base_path: '/backups' }} onClose={() => undefined} /></QueryClientProvider>)
    const user = userEvent.setup()
    try {
      await screen.findByRole('row', { name: /file/ })
      await waitFor(() => expect(reads).toBe(1))
      await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
      await user.click(screen.getByRole('button', { name: 'Delete', exact: true }))
      await user.click(within(screen.getByRole('dialog', { name: 'Delete Files' })).getByRole('button', { name: 'Delete', exact: true }))
      await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Delete Files' })).not.toBeInTheDocument())
      release()
      await waitFor(() => expect(screen.getByText(job.id)).toBeVisible(), { timeout: 5500 })
      if (state === 'running') {
        expect(screen.getByText('running - 0/1 entries')).toBeVisible()
        current = { ...job }
        await screen.findByText('completed - 1/1 entries', {}, { timeout: 3500 })
        expect(reads).toBeGreaterThanOrEqual(3)
      }
      expect(creates).toBe(1)
    } finally {
      release(); mounted.unmount(); client.clear()
    }
  }, 10000)
})
