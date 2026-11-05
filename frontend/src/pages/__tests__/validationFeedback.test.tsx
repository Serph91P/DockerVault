import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClientProvider } from '@tanstack/react-query'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import { queryClient, clearSession } from '../../store/session'
import Retention from '../Retention'
import Storage from '../Storage'
import Schedules from '../Schedules'
import { MemoryRouter } from 'react-router-dom'
import { useNotificationStore } from '../../store/notifications'

afterEach(() => { cleanup(); clearSession() })
it.each([[{ deleted: 2 }, 'unknown'], [{ deleted: 2, warnings: ['SYNTHETIC DETAIL'] }, 'partial'], [{ deleted: 0, success: false }, 'failed']])('cleanup %j does not infer overall completion from a counter', async (body, phase) => {
  server.use(http.post('/api/v1/retention/cleanup-orphaned', () => HttpResponse.json(body)))
  render(<QueryClientProvider client={queryClient}><Retention /></QueryClientProvider>)
  await userEvent.click(screen.getByRole('button', { name: 'Cleanup', exact: true }))
  await userEvent.click(within(screen.getByRole('dialog', { name: 'Cleanup Orphaned Files' })).getByRole('button', { name: 'Cleanup', exact: true }))
  await waitFor(() => expect(useNotificationStore.getState().notifications[0]?.phase).toBe(phase))
})
it('schedule backend validation remains inline with the preserved cron input', async () => {
  server.use(http.post('/api/v1/schedules', () => HttpResponse.json({ detail: [{ loc: ['body', 'cron_expression'], type: 'value_error', input: 'DO NOT RETAIN' }] }, { status: 422 })))
  render(<QueryClientProvider client={queryClient}><Schedules /></QueryClientProvider>)
  await userEvent.click(screen.getByRole('button', { name: 'New Schedule' }))
  await userEvent.type(screen.getByRole('textbox', { name: 'Name' }), 'Preserved schedule')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  const cron = screen.getByRole('textbox', { name: 'Cron Expression' })
  await waitFor(() => expect(cron).toHaveAttribute('aria-invalid', 'true'))
  expect(cron).toHaveAccessibleDescription(/check this value/i)
  expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Preserved schedule')
})
it('storage validation keeps the stepped form and associates a safe name error on return', async () => {
  server.use(http.post('/api/v1/storage', () => HttpResponse.json({ detail: [{ loc: ['body', 'name'], type: 'value_error', input: 'DO NOT RETAIN', msg: 'DO NOT RETAIN' }] }, { status: 422 })))
  render(<QueryClientProvider client={queryClient}><MemoryRouter><Storage /></MemoryRouter></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Add Storage' }))
  await user.type(screen.getByPlaceholderText('My Backup Storage'), 'Retained storage')
  await user.click(screen.getByRole('button', { name: 'Next' }))
  await user.click(screen.getByRole('button', { name: 'Next' }))
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Add Storage', exact: true }))
  await screen.findByRole('alert')
  await user.click(screen.getByRole('button', { name: 'Back', exact: true }))
  await user.click(screen.getByRole('button', { name: 'Back', exact: true }))
  const name = screen.getByPlaceholderText('My Backup Storage')
  expect(name).toHaveValue('Retained storage')
  expect(name).toHaveAttribute('aria-invalid', 'true')
  expect(name).toHaveAccessibleDescription(/check this value/i)
})
it('retains form input and associates static field validation without caching server echoes', async () => {
  const marker = 'SYNTHETIC-PRIVATE-INPUT'
  server.use(http.post('/api/v1/retention', () => HttpResponse.json({ detail: [{ loc: ['body', 'keep_last'], type: 'greater_than_equal', msg: marker, input: marker, ctx: { value: marker } }, { loc: ['body', marker], type: marker, msg: marker }] }, { status: 422 })))
  render(<QueryClientProvider client={queryClient}><Retention /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'New Policy' }))
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'Retained policy input')
  await user.click(screen.getByRole('button', { name: 'Create', exact: true }))
  const field = screen.getByRole('spinbutton', { name: 'Keep Last' })
  await waitFor(() => expect(field).toHaveAttribute('aria-invalid', 'true'))
  expect(field).toHaveAccessibleDescription(/outside the allowed range/i)
  expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('Retained policy input')
  expect(screen.getByRole('alert')).toHaveTextContent(/review the highlighted fields/i)
  const errors = queryClient.getMutationCache().getAll().map(m => m.state.error)
  expect(JSON.stringify(errors)).not.toContain(marker)
  expect(errors.some(error => error && 'config' in error)).toBe(false)
})
