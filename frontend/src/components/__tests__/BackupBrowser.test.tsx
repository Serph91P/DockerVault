import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import BackupBrowser from '../BackupBrowser'
import { server } from '../../test/mocks/server'
import type { Backup } from '../../api'

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals() })

it.each([false, true])('browses bounded pages, including remote-only encrypted=%s', async (encrypted) => {
  const requests: number[] = []
  const files = Array.from({ length: 2005 }, (_, index) => ({
    name: `file-${index.toString().padStart(4, '0')}`, path: `${index < 2000 ? 'first-page' : 'last-page'}/file-${index.toString().padStart(4, '0')}`,
    size: 1, size_human: '1 B', is_dir: false, mode: '644', mtime: '',
  }))
  const handler = async ({ request }: { request: Request }) => {
    const url = new URL(request.url)
    if (url.searchParams.get('limit') !== '2000') return HttpResponse.json({ detail: 'A bounded page is required' }, { status: 422 })
    const offset = Number(url.searchParams.get('offset'))
    requests.push(offset)
    if (encrypted) expect(await request.json()).toEqual({ private_key: 'owned-test-key' })
    return HttpResponse.json(files.slice(offset, offset + 2000), { headers: {
      'X-Total-Count': '2005', 'X-Has-More': offset === 0 ? 'true' : 'false',
    } })
  }
  server.use(encrypted ? http.post('/api/v1/backups/917/files', handler) : http.get('/api/v1/backups/917/files', handler))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const backup = { id: 917, encrypted, local_available: false, remote_synced: true, file_path: null } as Backup
  render(<QueryClientProvider client={client}><BackupBrowser backup={backup} onClose={() => {}} /></QueryClientProvider>)
  const user = userEvent.setup()
  if (encrypted) {
    await user.type(screen.getByPlaceholderText(/AGE-SECRET-KEY/), 'owned-test-key')
    await user.click(screen.getByRole('button', { name: /Decrypt/ }))
  }
  await screen.findByText('first-page')
  expect(screen.queryByText('last-page')).not.toBeInTheDocument()
  expect(screen.getByText(/1-2000 of 2005/)).toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Next page' }))
  await user.click(await screen.findByText('last-page'))
  await screen.findByText('file-2004')
  expect(screen.queryByText('first-page')).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Next page' })).toBeDisabled()
  expect(screen.getByText(/2001-2005 of 2005/)).toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Previous page' }))
  await screen.findByText('first-page')
  await waitFor(() => expect(requests).toEqual([0, 2000, 0]))
})

it.each([false, true])('keeps decryption material out of successful and failed browse caches: failure=%s', async (failed) => {
  const secret = 'synthetic-private-input'
  server.use(http.post('/api/v1/backups/918/files', () => failed
    ? HttpResponse.json({ detail: 'Invalid identity' }, { status: 400 })
    : HttpResponse.json([], { headers: { 'X-Total-Count': '0', 'X-Has-More': 'false' } })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const view = render(<QueryClientProvider client={client}><BackupBrowser backup={{ id: 918, encrypted: true } as Backup} onClose={() => {}} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.type(screen.getByPlaceholderText(/AGE-SECRET-KEY/), secret)
  await user.click(screen.getByRole('button', { name: /Decrypt/ }))
  await screen.findByText(failed ? 'Decryption Failed' : 'This backup is empty')
  const cache = JSON.stringify(client.getQueryCache().getAll().map(q => [q.queryKey, q.state]))
  expect(cache.includes(secret)).toBe(false)
  view.unmount()
  await waitFor(() => expect(client.getQueryCache().getAll()).toHaveLength(0))
})

it('downloads the actual nested archive path, not a stripped display path', async () => {
  let requested = ''
  server.use(
    http.get('/api/v1/backups/919/files', () => HttpResponse.json([{ name: 'data.txt', path: 'folder/nested/data.txt', size: 3, size_human: '3 B', is_dir: false, mode: '644', mtime: '' }], { headers: { 'X-Total-Count': '1', 'X-Has-More': 'false' } })),
    http.get('/api/v1/backups/919/files/:path', ({ params }) => { requested = String(params.path); return new HttpResponse('abc') }),
  )
  vi.stubGlobal('URL', class extends URL { static createObjectURL() { return 'blob:test' }; static revokeObjectURL() {} })
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
  render(<QueryClientProvider client={new QueryClient()}><BackupBrowser backup={{ id: 919, encrypted: false } as Backup} onClose={() => {}} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(await screen.findByText('folder'))
  await user.click(screen.getByText('nested'))
  await user.click(screen.getByTitle('Download file'))
  await waitFor(() => expect(requested).toBe('folder/nested/data.txt'))
  click.mockRestore()
  vi.unstubAllGlobals()
})
