import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import SSHKeyManager from '../SSHKeyManager'
const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()) })
it('names actual storage references before irreversible SSH file deletion without claiming a backend guard', async () => {
  server.use(http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([{ name: 'fixture', public_key: 'ssh-ed25519 public-fixture', private_path: '/app/data/ssh_keys/fixture', created_at: 1 }])), http.get('/api/v1/storage', () => HttpResponse.json([{ id: 7, name: 'Referenced provider', storage_type: 'ssh', enabled: true, base_path: '/backups', ssh_key_path: '/app/data/ssh_keys/fixture' }])))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client)
  render(<QueryClientProvider client={client}><SSHKeyManager /></QueryClientProvider>)
  await userEvent.click(await screen.findByRole('button', { name: 'Delete key' }))
  expect(await screen.findByText('Referenced provider')).toBeVisible()
  expect(screen.getByRole('dialog')).toHaveTextContent(/Deleting this file can break/)
})
it.each(['generate', 'install'])('announces failed SSH %s without exposing response details', async action => {
  server.use(http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([{ name: 'fixture', public_key: 'ssh-ed25519 public-fixture', private_path: '/app/data/ssh_keys/fixture', created_at: 1 }])), http.post('/api/v1/storage/ssh-keys', () => new HttpResponse(null, { status: 503 })), http.post('/api/v1/storage/ssh-keys/fixture/install', () => new HttpResponse(null, { status: 503 })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client)
  render(<QueryClientProvider client={client}><SSHKeyManager /></QueryClientProvider>)
  await userEvent.click(await screen.findByRole('button', { name: action === 'generate' ? 'Generate' : 'Install on remote', exact: true }))
  if (action === 'generate') await userEvent.type(screen.getByLabelText('Name', { exact: true }), 'new_fixture')
  else { await userEvent.type(screen.getByLabelText('Host', { exact: true }), 'fixture.example'); await userEvent.type(screen.getByLabelText('Username', { exact: true }), 'fixture'); await userEvent.type(screen.getByLabelText('Password', { exact: true }), 'private-fixture-password') }
  const buttons = screen.getAllByRole('button', { name: action === 'generate' ? 'Generate' : 'Install', exact: true })
  await userEvent.click(buttons[buttons.length - 1])
  expect(await screen.findByRole('alert')).toBeVisible()
})

it.each(['completed', 'partial', 'accepted', 'failed', 'unknown'])('private SSH installation reports %s without caching its password-bearing closure', async outcome => {
  server.use(http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([{ name: 'fixture', public_key: 'ssh-ed25519 public-fixture', private_path: '/app/data/ssh_keys/fixture', created_at: 1 }])), http.post('/api/v1/storage/ssh-keys/fixture/install', () => HttpResponse.json(outcome === 'unknown' ? {} : { status: outcome, success: outcome !== 'failed' })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client)
  render(<QueryClientProvider client={client}><SSHKeyManager /></QueryClientProvider>)
  await userEvent.click(await screen.findByRole('button', { name: 'Install on remote' }))
  await userEvent.type(screen.getByLabelText('Host', { exact: true }), 'fixture.example')
  await userEvent.type(screen.getByLabelText('Username', { exact: true }), 'fixture')
  await userEvent.type(screen.getByLabelText('Password', { exact: true }), 'private-fixture-password')
  await userEvent.click(screen.getByRole('button', { name: 'Install', exact: true }))
  await waitFor(() => expect(screen.getByRole('status', { name: 'SSH installation result' })).toHaveTextContent(outcome))
  expect(screen.getByLabelText('Password', { exact: true })).toHaveValue('')
  expect(client.getMutationCache().getAll()).toHaveLength(0)
  expect(screen.getByRole('status', { name: 'SSH installation result' })).toHaveTextContent(outcome)
  if (outcome !== 'completed') expect(screen.queryByText('Key installed successfully.')).not.toBeInTheDocument()
  await userEvent.type(screen.getByLabelText('Host', { exact: true }), '/changed')
  expect(screen.queryByRole('status', { name: 'SSH installation result' })).not.toBeInTheDocument()
})
