import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import StepSchedule from './StepSchedule'
import type { WizardData } from './index'

const data = {
  scheduleId: null,
  newSchedule: { name: 'owned', cronExpression: '0 3 * * mon-fri', description: '' },
} as WizardData

function show() {
  const updateData = vi.fn()
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <StepSchedule data={data} updateData={updateData} schedules={[]} isLoadingSchedules={false} />
  </QueryClientProvider>)
  return updateData
}

afterEach(cleanup)

it('renders server preview, timezone and compatibility warnings, without a local parser', async () => {
  const requests: unknown[] = []
  server.use(
    http.get('/api/v1/schedules', () => HttpResponse.json([])),
    http.post('/api/v1/schedules/preview', async ({ request }) => {
      requests.push(await request.json())
      return HttpResponse.json({ next_runs: ['2026-11-02T03:00:00+01:00'], timezone: 'Europe/Berlin', contract: 'Monday=0; Sunday=6; AND', warnings: ['Review numeric weekdays'] })
    }),
  )
  show()
  expect(await screen.findByText(/Europe\/Berlin/)).toBeInTheDocument()
  expect(await screen.findByText(/Review numeric weekdays/)).toBeInTheDocument()
  expect(screen.getByText(/Monday=0; Sunday=6; AND/)).toBeInTheDocument()
  expect(screen.getByText(new Date('2026-11-02T03:00:00+01:00').toLocaleString())).toBeInTheDocument()
  expect(requests).toContainEqual({ cron_expression: '0 3 * * mon-fri' })
})

it('Sunday preset sends a named Sunday expression', async () => {
  server.use(http.get('/api/v1/schedules', () => HttpResponse.json([])),
    http.post('/api/v1/schedules/preview', () => HttpResponse.json({ next_runs: [], timezone: 'UTC', contract: '', warnings: [] })))
  const update = show()
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Preset' }), '0 0 * * sun')
  expect(update).toHaveBeenCalledWith({ newSchedule: expect.objectContaining({ cronExpression: '0 0 * * sun' }) })
})

it('shows server validation failure rather than misleading local next dates', async () => {
  server.use(http.get('/api/v1/schedules', () => HttpResponse.json([])),
    http.post('/api/v1/schedules/preview', () => HttpResponse.json({ detail: 'Invalid cron expression: five fields required' }, { status: 400 })))
  show()
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Schedule preview unavailable. Check the server before saving.'))
})
