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
it.each([[{ deleted: 2 }, 'unknown'], [{ deleted: 2, warnings: ['SYNTHETIC DETAIL'] }, 'unknown'], [{ deleted: 0, success: false }, 'unknown'], [{ outcome: 'partial', deleted: 1, failed: 1, protected: 0, scan_errors: 0, scan_complete: true, freed_bytes: 6, freed_human: '6.00 B' }, 'partial'], [{ outcome: 'failure', deleted: 0, failed: 1, protected: 0, scan_errors: 0, scan_complete: true, freed_bytes: 0, freed_human: '0.00 B' }, 'failed']])('cleanup %j does not infer overall completion from a counter', async (body, phase) => {
  server.use(http.post('/api/v1/retention/cleanup-orphaned', () => HttpResponse.json(body)))
  render(<QueryClientProvider client={queryClient}><MemoryRouter initialEntries={['/protection/retention']}><Retention /></MemoryRouter></QueryClientProvider>)
  await userEvent.click(screen.getByRole('button', { name: 'Cleanup', exact: true }))
  await userEvent.click(within(screen.getByRole('dialog', { name: 'Cleanup Orphaned Files' })).getByRole('button', { name: 'Cleanup', exact: true }))
  await waitFor(() => expect(useNotificationStore.getState().notifications[0]?.phase).toBe(phase))
})
it('schedule backend validation remains inline with the preserved cron input', async () => {
  server.use(http.post('/api/v1/schedules', () => HttpResponse.json({ detail: [{ loc: ['body', 'cron_expression'], type: 'value_error', input: 'DO NOT RETAIN' }] }, { status: 422 })))
  render(<QueryClientProvider client={queryClient}><MemoryRouter initialEntries={['/protection/schedules']}><Schedules /></MemoryRouter></QueryClientProvider>)
  await userEvent.click(screen.getByRole('button', { name: 'New Schedule' }))
  await userEvent.type(screen.getByRole('textbox', { name: 'Schedule name' }), 'Preserved schedule')
  await userEvent.click(screen.getByRole('button', { name: 'Save', exact: true }))
  const cron = screen.getByRole('textbox', { name: 'Cron expression' })
  await waitFor(() => expect(cron).toHaveAttribute('aria-invalid', 'true'))
  expect(cron).toHaveAccessibleDescription(/check this value/i)
  expect(screen.getByRole('textbox', { name: 'Schedule name' })).toHaveValue('Preserved schedule')
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
it.each(['detail', 'errors'])('retains form input and associates static %s validation without caching server echoes', async envelope => {
  const marker = 'SYNTHETIC-PRIVATE-INPUT'
  const body = envelope === 'detail'
    ? { detail: [{ loc: ['body', 'keep_last'], type: 'greater_than_equal', msg: marker, input: marker, ctx: { value: marker } }, { loc: ['body', marker], type: marker, msg: marker }] }
    : { detail: 'Validation error', errors: [{ field: 'body.keep_last', message: marker, msg: marker, input: marker, ctx: { value: marker } }, { field: 'body.' + marker, message: marker }] }
  server.use(http.post('/api/v1/retention', () => HttpResponse.json(body, { status: 422 })))
  render(<QueryClientProvider client={queryClient}><MemoryRouter initialEntries={['/protection/retention']}><Retention /></MemoryRouter></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'New Policy' }))
  await user.type(screen.getByRole('textbox', { name: 'Policy name' }), 'Retained policy input')
  await user.click(screen.getByRole('button', { name: 'Save', exact: true }))
  const field = screen.getByRole('spinbutton', { name: 'Keep Last' })
  await waitFor(() => expect(field).toHaveAttribute('aria-invalid', 'true'))
  expect(field).toHaveAccessibleDescription(envelope === 'detail' ? /outside the allowed range/i : /check this value/i)
  expect(screen.getByRole('textbox', { name: 'Policy name' })).toHaveValue('Retained policy input')
  expect(screen.getByRole('alert')).toHaveTextContent(/review the highlighted fields/i)
  const errors = queryClient.getMutationCache().getAll().map(m => m.state.error)
  expect(JSON.stringify(errors)).not.toContain(marker)
  expect(errors.some(error => error && 'config' in error)).toBe(false)
})
