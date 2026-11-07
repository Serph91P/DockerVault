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
    http.get('/api/v1/backups/44/restore-info', () => HttpResponse.json({ original_supported:true,volume_discovery:'available',remote_copies:[],backup: { id: 44, encrypted: true, created_at: '', local_available: true }, target: { id: 3, name: 'Synthetic', target_type: 'path', host_path: '/synthetic' }, containers_to_stop: [], available_volumes: [] })),
    http.post('/api/v1/backups/44/restore-review', () => HttpResponse.json({fingerprint:'safe',destination:{type:'original',path:'/synthetic',volume:null},containers_to_stop:[],containers_to_restart:[],layout:'single',overwrite:'merge',key_required:true})),
    http.post('/api/v1/backups/44/restore', () => HttpResponse.json(status === 200 ? { status: 'restored',fingerprint:'safe',destination:{type:'original',path:'/synthetic',volume:null},containers_to_stop:[],containers_to_restart:[],layout:'single',overwrite:'merge',key_required:true } : { detail: 'Failed' }, { status })),
  )
  const client = new QueryClient()
  let closed = false
  render(<QueryClientProvider client={client}><RestoreWizard backupId={44} onClose={() => { closed = true }} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Next' }))
  await user.type(screen.getByPlaceholderText(/AGE-SECRET-KEY/), secret)
  await user.click(screen.getByRole('button', { name: 'Review effects' }))
  await user.click(await screen.findByLabelText(/I reviewed/))
  await user.click(screen.getByRole('button', { name: 'Confirm restore' }))
  if (status === 200) {
    expect(await screen.findByRole('heading', {name:'Restore completed'})).toBeVisible()
    expect(closed).toBe(false)
  }
  else await waitFor(() => expect(screen.getByPlaceholderText(/AGE-SECRET-KEY/)).toHaveValue(''))
  const cache = JSON.stringify(client.getMutationCache().getAll().map(m => m.state))
  expect(cache.includes(secret)).toBe(false)
})
