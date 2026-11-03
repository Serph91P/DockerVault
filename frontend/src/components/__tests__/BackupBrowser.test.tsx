import { afterEach, expect, it } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import BackupBrowser from '../BackupBrowser'
import { server } from '../../test/mocks/server'
import type { Backup } from '../../api'

afterEach(cleanup)

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
