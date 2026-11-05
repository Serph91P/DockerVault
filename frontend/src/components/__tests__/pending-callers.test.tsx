import { StrictMode } from 'react'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Retention from '../../pages/Retention'
import Storage from '../../pages/Storage'
import { MemoryRouter } from 'react-router-dom'

const cases = [
  { title: 'Delete Retention Policy', trigger: 'Delete policy Synthetic policy', confirm: 'Delete', method: 'delete', url: '/api/v1/retention/982', Page: Retention },
  { title: 'Cleanup Orphaned Files', trigger: 'Cleanup', confirm: 'Cleanup', method: 'post', url: '/api/v1/retention/cleanup-orphaned', Page: Retention },
  { title: 'Delete Storage', trigger: 'Delete', confirm: 'Delete', method: 'delete', url: '/api/v1/storage/982', Page: Storage },
] as const

function fixtures() {
  server.use(
    http.get('/api/v1/retention', () => HttpResponse.json([{ id: 982, name: 'Synthetic policy', keep_last: 3, keep_daily: 7, keep_weekly: 4, keep_monthly: 12, keep_yearly: 1, max_age_days: 365 }])),
    http.get('/api/v1/targets', () => HttpResponse.json([])),
    http.get('/api/v1/storage', () => HttpResponse.json([{ id: 982, name: 'Synthetic storage', storage_type: 'local', enabled: true, base_path: '/synthetic' }])),
    http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([])),
  )
}

describe('real confirmation callers retain pending state until the result', () => {
  it.each(cases)('$title keeps focus and dismissal locked, then allows failure retry and closes on success', async ({ title, trigger, confirm, method, url, Page }) => {
    fixtures()
    let release!: (response: HttpResponse) => void
    let requests = 0
    server.use(http[method](url, async () => {
      requests++
      return await new Promise<HttpResponse>((resolve) => { release = resolve })
    }))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    const user = userEvent.setup()
    render(<StrictMode><QueryClientProvider client={client}><MemoryRouter><Page /></MemoryRouter></QueryClientProvider></StrictMode>)
    await user.click(await screen.findByRole('button', { name: trigger, exact: true }))
    const dialog = screen.getByRole('dialog', { name: title })
    await user.click(within(dialog).getByRole('button', { name: confirm, exact: true }))
    await waitFor(() => expect(requests).toBe(1))
    expect(dialog).toBeVisible()
    expect(dialog).toHaveAttribute('aria-busy', 'true')
    expect(within(dialog).getByRole('button', { name: 'Processing...' })).toBeDisabled()
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeDisabled()
    await user.keyboard('{Escape}')
    await user.tab()
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
    expect(dialog).toBeVisible()
    release(HttpResponse.json({ detail: 'Synthetic failure' }, { status: 503 }))
    await waitFor(() => expect(within(dialog).getByRole('button', { name: confirm, exact: true })).toBeEnabled())
    expect(within(dialog).getByRole('alert')).toHaveTextContent(/failed/i)
    await user.click(within(dialog).getByRole('button', { name: confirm, exact: true }))
    await waitFor(() => expect(requests).toBe(2))
    expect(dialog).toHaveAttribute('aria-busy', 'true')
    release(HttpResponse.json({ deleted: 0 }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: title })).not.toBeInTheDocument())
  })

  it.each(cases)('$title can be cancelled without starting a request', async ({ title, trigger, method, url, Page }) => {
    fixtures()
    let requests = 0
    server.use(http[method](url, () => { requests++; return HttpResponse.json({ deleted: 0 }) }))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const user = userEvent.setup()
    render(<QueryClientProvider client={client}><MemoryRouter><Page /></MemoryRouter></QueryClientProvider>)
    const opener = await screen.findByRole('button', { name: trigger, exact: true })
    await user.click(opener)
    await user.click(within(screen.getByRole('dialog', { name: title })).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(requests).toBe(0)
    expect(opener).toHaveFocus()
  })
})
