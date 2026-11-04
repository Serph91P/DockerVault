import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import EncryptionSetup from '../EncryptionSetup'
import { server } from '../../test/mocks/server'

const secret = 'synthetic-private-material-not-a-real-key'
const pending = { setup_completed: false, encryption_enabled: false, public_key: null, onboarding_deferred: false }
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals() })

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const view = render(<QueryClientProvider client={client}><EncryptionSetup /></QueryClientProvider>)
  return { client, ...view }
}

it('never retains one-time key material in query or mutation caches, including failed export', async () => {
  let generated = 0
  server.use(
    http.get('/api/v1/encryption/status', () => HttpResponse.json(pending)),
    http.post('/api/v1/encryption/setup', () => { generated++; return HttpResponse.json({ public_key: 'public', private_key: secret, recovery_instructions: 'Offline instructions' }) }),
  )
  const { client } = mount()
  const user = userEvent.setup()
  await user.dblClick(await screen.findByRole('button', { name: 'Set Up Encryption' }))
  await screen.findByRole('dialog', { name: 'Save Your Private Key' })
  expect(generated).toBe(1)
  const caches = JSON.stringify([client.getQueryCache().getAll().map(q => [q.queryKey, q.state]), client.getMutationCache().getAll().map(m => m.state)])
  expect(caches.includes(secret)).toBe(false)
  expect(JSON.stringify(localStorage).includes(secret)).toBe(false)
  expect(JSON.stringify(sessionStorage).includes(secret)).toBe(false)
  expect(window.location.href.includes(secret)).toBe(false)
  vi.stubGlobal('URL', class extends URL { static createObjectURL() { throw new Error('Export cancelled') } })
  await user.click(screen.getByRole('button', { name: 'Download Private Key File' }))
  expect(screen.getByRole('button', { name: 'I have saved my private key' })).toBeDisabled()
  expect(screen.queryByText('Key Downloaded')).not.toBeInTheDocument()
  vi.restoreAllMocks()
})

it('exports an age-readable identity file and still requires explicit custody confirmation', async () => {
  let blob!: Blob
  vi.stubGlobal('URL', class extends URL { static createObjectURL(value: Blob) { blob = value; return 'blob:test' }; static revokeObjectURL() {} })
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
  let confirmed = 0
  server.use(
    http.get('/api/v1/encryption/status', () => HttpResponse.json(pending)),
    http.post('/api/v1/encryption/setup', () => HttpResponse.json({ public_key: 'public', private_key: secret, recovery_instructions: 'Recovery steps\nRun age here' })),
    http.post('/api/v1/encryption/confirm-setup', () => { confirmed++; return HttpResponse.json({ message: 'Confirmed' }) }),
  )
  mount()
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Set Up Encryption' }))
  await user.click(await screen.findByRole('button', { name: 'Download Private Key File' }))
  const content = await new Promise<string>((resolve) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.readAsText(blob) })
  expect(content.split('\n').every(line => !line.trim() || line.startsWith('#') || line === secret)).toBe(true)
  expect(confirmed).toBe(0)
  expect(screen.getByRole('button', { name: 'I have saved my private key' })).toBeEnabled()
  await user.click(screen.getByRole('button', { name: 'I have saved my private key' }))
  expect(confirmed).toBe(1)
})

it('does not replay setup for an existing pending public key after reload', async () => {
  server.use(http.get('/api/v1/encryption/status', () => HttpResponse.json({ ...pending, public_key: 'existing-public-key' })))
  mount()
  await screen.findByText('existing-public-key')
  expect(screen.queryByRole('button', { name: 'Set Up Encryption' })).not.toBeInTheDocument()
  expect(screen.getByText(/pending/i)).toBeInTheDocument()
})

it('clears the one-time key when the keyboard closes the custody dialog', async () => {
  server.use(
    http.get('/api/v1/encryption/status', () => HttpResponse.json(pending)),
    http.post('/api/v1/encryption/setup', () => HttpResponse.json({ public_key: 'public', private_key: secret, recovery_instructions: 'Instructions' })),
  )
  mount()
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Set Up Encryption' }))
  const dialog = await screen.findByRole('dialog', { name: 'Save Your Private Key' })
  expect(dialog.contains(document.activeElement)).toBe(true)
  await user.keyboard('{Escape}')
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(document.body.textContent?.includes(secret)).toBe(false)
})

it('shows retry, never key generation, when encryption status is unavailable', async () => {
  server.use(http.get('/api/v1/encryption/status', () => HttpResponse.json({}, { status: 503 })))
  mount()
  await screen.findByRole('button', { name: /retry/i })
  expect(screen.queryByRole('button', { name: 'Set Up Encryption' })).not.toBeInTheDocument()
})
