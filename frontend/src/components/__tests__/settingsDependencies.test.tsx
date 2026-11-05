import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import SSHKeyManager from '../SSHKeyManager'
import EncryptionSetup from '../EncryptionSetup'
import BackupLogViewer from '../BackupLogViewer'

const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()) })
it.each([
  ['/storage/ssh-keys', 'SSH keys', [], <SSHKeyManager />],
  ['/encryption/status', 'Encryption status', { setup_completed: false, encryption_enabled: false }, <EncryptionSetup />],
  ['/backups/1/logs', 'Run logs', [], <BackupLogViewer backupId={1} onClose={() => {}} />],
] as const)('%s errors are not successful empty/disabled and preserve dated snapshots on refetch failure', async (path, label, valid, element) => {
  let failed = true
  server.use(http.get('/api/v1' + path, () => failed ? new HttpResponse(null, { status: 503 }) : HttpResponse.json(valid)))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(<QueryClientProvider client={client}>{element}</QueryClientProvider>)
  await screen.findByRole('alert', { name: label + ' status' })
  expect(screen.queryByText(/No SSH keys yet/)).not.toBeInTheDocument()
  expect(screen.queryByText('Encryption Disabled', { exact: true })).not.toBeInTheDocument()
  failed = false
  await userEvent.click(screen.getByRole('button', { name: 'Retry ' + label }))
  await waitFor(() => expect(screen.getByLabelText(label + ' status')).toHaveTextContent(/updated/))
  failed = true
  await client.invalidateQueries()
  expect(await screen.findByRole('alert', { name: label + ' status' })).toHaveTextContent(/stale data/)
})
