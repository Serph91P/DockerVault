import { afterEach, expect, it } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import RestoreWizard from '../RestoreWizard'
import { server } from '../../test/mocks/server'

const secret = 'synthetic-restore-private-input'
afterEach(cleanup)
it.each([200, 500])('never caches a restore key or request config on HTTP %s', async (status) => {
  server.use(
    http.get('/api/v1/backups/44/restore-info', () => HttpResponse.json({ backup: { id: 44, encrypted: true, created_at: '', local_available: true }, target: { id: 3, name: 'Synthetic', target_type: 'path', host_path: '/synthetic' }, containers_to_stop: [], available_volumes: [] })),
    http.post('/api/v1/backups/44/restore', () => HttpResponse.json(status === 200 ? { status: 'restored' } : { detail: 'Failed' }, { status })),
  )
  const client = new QueryClient()
  let closed = false
  render(<QueryClientProvider client={client}><RestoreWizard backupId={44} onClose={() => { closed = true }} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Next' }))
  await user.type(screen.getByPlaceholderText(/AGE-SECRET-KEY/), secret)
  await user.click(screen.getByRole('button', { name: 'Next' }))
  await user.click(screen.getByRole('button', { name: 'Restore Now' }))
  if (status === 200) await waitFor(() => expect(closed).toBe(true))
  else expect(await screen.findByPlaceholderText(/AGE-SECRET-KEY/)).toHaveValue('')
  const cache = JSON.stringify(client.getMutationCache().getAll().map(m => m.state))
  expect(cache.includes(secret)).toBe(false)
})
