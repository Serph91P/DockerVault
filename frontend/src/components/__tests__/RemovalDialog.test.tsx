import { render, screen, waitFor, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import RemovalDialog from '../RemovalDialog'

describe('exact removal confirmation', () => {
  it('cancels without mutation and retries only the server failure token after HTTP200 warning', async () => {
    const execute = vi.fn()
    server.use(
      http.post('/api/v1/backups/removal/preview', () => HttpResponse.json({ token: 'frozen', scope: 'local', archive_count: 505, copy_count: 505, items: [{ backup_id: 3, target_id: 1, target_name: 'fixture', copy: 'local', label: 'Local copy' }], retained: [] })),
      http.post('/api/v1/backups/removal/execute', async ({ request }) => {
        const body = await request.json() as { token: string }
        execute(body.token)
        return HttpResponse.json(body.token === 'failed-only' ? { status: 'completed', succeeded: 1, failed: 0, skipped: 0, items: [], retry_token: null } : { status: 'incomplete', succeeded: 504, failed: 1, skipped: 0, warning: 'not all removed', items: [{ backup_id: 3, target_name: 'fixture', copy: 'local', label: 'Local copy', outcome: 'failed', message: 'offline' }], retry_token: 'failed-only' })
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
  })
})
