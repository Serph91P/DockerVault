import { render, screen, waitFor, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import RemovalDialog from '../RemovalDialog'

describe('exact removal confirmation', () => {
  it('keeps original skipped, retained and successful copies when retrying only a failed remote copy', async () => {
    const identity = { backup_id: 3, target_id: 1, target_name: 'fixture' }
    const skipped = { ...identity, copy: 'local', storage_id: null, label: 'Local copy', outcome: 'skipped', message: 'scope changed; review again' }
    const failed = { ...identity, copy: 'remote', storage_id: 7, label: 'Remote seven', outcome: 'failed', message: 'offline' }
    const succeeded = { ...identity, copy: 'remote', storage_id: 8, label: 'Remote eight', outcome: 'succeeded', message: 'removed earlier' }
    const retained = { ...identity, backup_id: 4, copy: 'local', storage_id: null, label: 'Unselected local', outcome: 'retained' }
    const tokens: string[] = []
    server.use(
      http.post('/api/v1/backups/removal/preview', () => HttpResponse.json({ token: 'original', scope: 'all', archive_count: 1, copy_count: 3, items: [skipped, failed, succeeded], retained: [retained] })),
      http.post('/api/v1/backups/removal/execute', async ({ request }) => {
        const { token } = await request.json() as { token: string }
        tokens.push(token)
        return HttpResponse.json(token === 'failed-seven-only'
          ? { status: 'completed', succeeded: 1, failed: 0, skipped: 0, items: [{ ...failed, outcome: 'succeeded', message: 'removed on retry' }], retry_token: null }
          : { status: 'incomplete', succeeded: 1, failed: 1, skipped: 1, warning: 'Review changed copies', items: [skipped, failed, succeeded, retained], retry_token: 'failed-seven-only' })
      }),
    )
    render(<RemovalDialog intent={{ kind: 'copies', targetId: 1, name: 'fixture' }} onClose={() => {}} onChanged={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: 'Review exact scope' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Permanently delete confirmed copies' }))
    expect(await screen.findByText('1 succeeded · 1 failed · 1 skipped')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Retry failed confirmed copies only' }))
    expect(await screen.findByText('2 succeeded · 0 failed · 1 skipped')).toBeInTheDocument()
    expect(screen.getByText('Removal incomplete')).toBeInTheDocument()
    expect(screen.queryByText('Selected copies removed')).not.toBeInTheDocument()
    expect(screen.getByText(/scope changed; review again/)).toBeInTheDocument()
    expect(screen.getByText(/removed earlier/)).toBeInTheDocument()
    expect(screen.getByText(/Unselected local: retained/)).toBeInTheDocument()
    expect(screen.getByText(/removed on retry/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Retry failed confirmed copies only' })).not.toBeInTheDocument()
    expect(tokens).toEqual(['original', 'failed-seven-only'])
  })
  it('cancels without mutation and retries only the server failure token after HTTP200 warning', async () => {
    const execute = vi.fn()
    server.use(
      http.post('/api/v1/backups/removal/preview', () => HttpResponse.json({ token: 'frozen', scope: 'local', archive_count: 505, copy_count: 505, items: [{ backup_id: 3, target_id: 1, target_name: 'fixture', copy: 'local', label: 'Local copy' }], retained: [] })),
      http.post('/api/v1/backups/removal/execute', async ({ request }) => {
        const body = await request.json() as { token: string }
        execute(body.token)
        return HttpResponse.json(body.token === 'failed-only' ? { status: 'completed', succeeded: 1, failed: 0, skipped: 0, items: [{ backup_id: 3, target_name: 'fixture', copy: 'local', label: 'Local copy', outcome: 'succeeded', message: 'removed' }], retry_token: null } : { status: 'incomplete', succeeded: 504, failed: 1, skipped: 0, warning: 'not all removed', items: [...Array.from({ length: 504 }, (_, index) => ({ backup_id: index + 10, target_name: 'fixture', copy: 'local', label: 'Local copy', outcome: 'succeeded' })), { backup_id: 3, target_name: 'fixture', copy: 'local', label: 'Local copy', outcome: 'failed', message: 'offline' }], retry_token: 'failed-only' })
      }),
    )
    const close = vi.fn()
    const { unmount } = render(<RemovalDialog intent={{ kind: 'copies', targetId: 1, name: 'fixture' }} onClose={close} onChanged={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(execute).not.toHaveBeenCalled()
    expect(close).toHaveBeenCalledOnce()
    unmount()
    render(<RemovalDialog intent={{ kind: 'copies', targetId: 1, name: 'fixture' }} onClose={() => {}} onChanged={() => {}} />)
    fireEvent.click(screen.getByLabelText('Local copies only'))
    fireEvent.click(screen.getByRole('button', { name: 'Review exact scope' }))
    expect(await screen.findByText(/505 archives · 505 selected copies/)).toBeInTheDocument()
    const confirm = screen.getByRole('button', { name: 'Permanently delete confirmed copies' })
    fireEvent.click(confirm)
    fireEvent.click(confirm)
    expect(await screen.findByText('Removal incomplete')).toBeInTheDocument()
    expect(execute).toHaveBeenCalledTimes(1)
    expect(screen.getByText(/offline/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Retry failed confirmed copies only' }))
    await waitFor(() => expect(execute.mock.calls).toEqual([['frozen'], ['failed-only']]))
    expect(await screen.findByText('Selected copies removed')).toBeInTheDocument()
    expect(screen.getByText('505 succeeded · 0 failed · 0 skipped')).toBeInTheDocument()
  })
})
