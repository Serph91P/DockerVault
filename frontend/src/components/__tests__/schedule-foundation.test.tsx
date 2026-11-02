import { StrictMode } from 'react'
import { render, screen, waitFor, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { describe, it, expect } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Schedules from '../../pages/Schedules'

describe('schedule form foundation vertical slice', () => {
  it('associates labels and validation, saves once and displays authoritative readback under StrictMode', async () => {
    let saved: { id: number; name: string; cron_expression: string; enabled: boolean; description: string }[] = []
    server.use(
      http.get('/api/v1/schedules', () => HttpResponse.json(saved)),
      http.post('/api/v1/schedules', async ({ request }) => {
        const data = await request.json() as typeof saved[number]
        saved = [{ ...data, id: 72 }]
        return HttpResponse.json(saved[0])
      }),
    )
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const user = userEvent.setup()
    render(<StrictMode><QueryClientProvider client={client}><Schedules /></QueryClientProvider></StrictMode>)
    await user.click(screen.getByRole('button', { name: 'New Schedule' }))
    const dialog = screen.getByRole('dialog', { name: 'New Schedule' })
    const name = within(dialog).getByRole('textbox', { name: 'Name' })
    await user.click(within(dialog).getByRole('button', { name: 'Save' }))
    expect(name).toHaveAttribute('aria-invalid', 'true')
    expect(name).toHaveAccessibleDescription('Enter a schedule name.')
    await user.type(name, 'Synthetic nightly schedule')
    await user.click(within(dialog).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(await screen.findByRole('heading', { name: 'Synthetic nightly schedule' })).toBeInTheDocument()
    expect(saved).toHaveLength(1)
  })
})
