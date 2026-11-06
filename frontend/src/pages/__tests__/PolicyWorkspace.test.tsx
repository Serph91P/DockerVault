import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, Routes, Route, useLocation } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '../../test/mocks/server'
import '../../store/session'
import Retention from '../Retention'
import Schedules from '../Schedules'

const policy = {
  id: 7,
  name: 'default',
  keep_last: 3,
  keep_daily: 7,
  keep_weekly: 4,
  keep_monthly: 6,
  keep_yearly: 2,
  max_age_days: 365,
}
const schedule = {
  id: 8,
  name: 'Nightly',
  cron_expression: '0 2 * * *',
  description: 'Clear me',
  enabled: true,
  target_count: 0,
}
function Location() {
  const location = useLocation()
  return (
    <output aria-label="Location">
      {location.pathname}
      {location.search}
    </output>
  )
}
function mount(
  path: string,
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  }),
) {
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[path]}>
        <Location />
        <Routes>
          <Route path="/protection/retention/*" element={<Retention />} />
          <Route path="/protection/schedules/*" element={<Schedules />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  )
  return userEvent.setup()
}
function fixtures() {
  server.use(
    http.get('/api/v1/targets', () => HttpResponse.json([])),
    http.get('/api/v1/retention', () => HttpResponse.json([policy])),
    http.get('/api/v1/retention/7', () => HttpResponse.json(policy)),
    http.get('/api/v1/schedules', () => HttpResponse.json([schedule])),
    http.get('/api/v1/schedules/8', () =>
      HttpResponse.json({ ...schedule, targets: [] }),
    ),
    http.get('/api/v1/schedules/jobs', () => HttpResponse.json([])),
    http.post('/api/v1/schedules/preview', () =>
      HttpResponse.json({
        next_runs: ['2027-01-02T02:00:00Z'],
        timezone: 'UTC',
        contract: 'Server cron',
        warnings: [],
      }),
    ),
  )
}
describe('durable reusable policy management', () => {
  it.each(['detail still exists', 'incomplete usage'])('does not confirm deletion with contradictory readback: %s', async mode => {
    fixtures(); let deleted = false
    server.use(
      http.delete('/api/v1/schedules/8', () => { deleted = true; return HttpResponse.json({ status: 'deleted' }) }),
      http.get('/api/v1/schedules/8', () => !deleted || mode === 'detail still exists' ? HttpResponse.json({ ...schedule, targets: [] }) : new HttpResponse(null, { status: 404 })),
      http.get('/api/v1/schedules', () => HttpResponse.json(deleted ? [] : [schedule])),
      http.get('/api/v1/targets', () => HttpResponse.json(deleted && mode === 'incomplete usage' ? [{ id: 1, name: 'Unverified', target_type: 'path', enabled: true, removed_at: null }] : [])),
    )
    const user = mount('/protection/schedules/8')
    await user.click(await screen.findByRole('button', { name: 'Delete schedule Nightly' }))
    await user.click(within(screen.getByRole('dialog', { name: 'Delete Schedule' })).getByRole('button', { name: 'Delete', exact: true }))
    await screen.findByText('Failed to verify deletion. Inspect server state before retrying.')
    expect(screen.getByLabelText('Location')).toHaveTextContent('/protection/schedules/8')
  })
  it('locks an unconfirmed run batch instead of offering a blind second POST', async () => {
    fixtures(); let requests = 0
    server.use(http.post('/api/v1/schedules/8/trigger-all', () => { requests++; return HttpResponse.error() }))
    const user = mount('/protection/schedules/8')
    await user.click(await screen.findByRole('button', { name: 'Run All', exact: true }))
    await screen.findByText(/Action result not confirmed/)
    expect(screen.getByRole('button', { name: 'Run All', exact: true })).toBeDisabled()
    expect(screen.getByRole('link', { name: 'Inspect runs' })).toHaveAttribute('href', '/history')
    expect(requests).toBe(1)
  })
  it('does not steal result-modal keyboard focus when background usage refreshes', async () => {
    fixtures()
    server.use(http.post('/api/v1/retention/cleanup-orphaned', () => HttpResponse.json({ outcome: 'empty', deleted: 0, failed: 0, protected: 0, scan_errors: 0, scan_complete: true, freed_bytes: 0, freed_human: '0.00 B' })))
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const user = mount('/protection/retention', client)
    await user.click(await screen.findByRole('button', { name: 'Cleanup', exact: true }))
    const dialog = screen.getByRole('dialog', { name: 'Cleanup Orphaned Files', exact: true })
    await user.click(within(dialog).getByRole('button', { name: 'Cleanup', exact: true }))
    await within(dialog).findByText('Cleanup result: empty')
    await user.tab()
    const close = within(dialog).getByRole('button', { name: 'Close' })
    await waitFor(() => expect(close).toHaveFocus())
    await client.invalidateQueries({ queryKey: ['targets'] })
    await waitFor(() => expect(close).toHaveFocus())
  })
  it('shows prospective null-fallback impact when renaming a rule to default', async () => {
    fixtures()
    server.use(
      http.get('/api/v1/retention/7', () =>
        HttpResponse.json({ ...policy, name: 'other' }),
      ),
      http.get('/api/v1/targets', () =>
        HttpResponse.json([
          {
            id: 2,
            name: 'Newly affected',
            target_type: 'path',
            enabled: true,
            removed_at: null,
            retention_policy_id: null,
          },
        ]),
      ),
    )
    const user = mount('/protection/retention/7/edit')
    const name = await screen.findByRole('textbox', { name: 'Policy name' })
    expect(
      screen.queryByRole('link', { name: 'Newly affected', exact: true }),
    ).not.toBeInTheDocument()
    await user.clear(name)
    await user.type(name, 'default')
    expect(
      await screen.findByRole('link', { name: 'Newly affected', exact: true }),
    ).toBeVisible()
    expect(
      screen.getByRole('heading', { name: 'Proposed fallback impact' }),
    ).toBeVisible()
  })
  it('treats incomplete assignment inventory as unknown rather than zero or default usage', async () => {
    fixtures()
    server.use(
      http.get('/api/v1/targets', () =>
        HttpResponse.json([
          { id: 1, name: 'Incomplete', target_type: 'path', enabled: true },
        ]),
      ),
    )
    mount('/protection/retention/7')
    expect(await screen.findByText('Usage unknown')).toBeVisible()
    expect(screen.queryByText('Not in use')).not.toBeInTheDocument()
  })
  it('maps authoritative cron rejection to the cron field without losing the editor', async () => {
    fixtures()
    server.use(
      http.post('/api/v1/schedules/preview', () =>
        HttpResponse.json({ detail: 'Invalid cron' }, { status: 400 }),
      ),
    )
    const user = mount('/protection/schedules/new')
    await user.type(
      await screen.findByRole('textbox', { name: 'Schedule name' }),
      'Keep input',
    )
    await user.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(
        screen.getByRole('textbox', { name: 'Cron expression' }),
      ).toHaveAttribute('aria-invalid', 'true'),
    )
    expect(screen.getByRole('textbox', { name: 'Schedule name' })).toHaveValue(
      'Keep input',
    )
    expect(screen.getByLabelText('Location')).toHaveTextContent(
      '/protection/schedules/new',
    )
  })
  it.each([undefined, 503])(
    'never blindly repeats an unknown Create (%s), retains input and reconciles through reload/jobs',
    async (status) => {
      fixtures()
      let writes = 0,
        reloads = 0,
        jobs = 0
      server.use(
        http.post('/api/v1/schedules', () => {
          writes++
          return status
            ? HttpResponse.json({ detail: 'stored, reload failed' }, { status })
            : HttpResponse.error()
        }),
        http.post('/api/v1/schedules/reload', () => {
          reloads++
          return HttpResponse.json({ status: 'reloaded' })
        }),
        http.get('/api/v1/schedules/jobs', () => {
          jobs++
          return HttpResponse.json([])
        }),
      )
      const user = mount('/protection/schedules/new?q=Nightly')
      await user.type(
        await screen.findByRole('textbox', { name: 'Schedule name' }),
        'Unconfirmed intent',
      )
      await user.click(screen.getByRole('button', { name: 'Save' }))
      await screen.findByText(/Write outcome requires verification/)
      expect(
        screen.getByRole('textbox', { name: 'Schedule name' }),
      ).toHaveValue('Unconfirmed intent')
      expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
      await user.click(
        screen.getByRole('button', { name: 'Reload and inspect server state' }),
      )
      await waitFor(() =>
        expect(screen.getByLabelText('Location')).toHaveTextContent(
          '/protection/schedules?q=Nightly',
        ),
      )
      expect(writes).toBe(1)
      expect(reloads).toBe(1)
      expect(jobs).toBe(1)
    },
  )
  it.each([
    ['Keep Last', '1001'],
    ['Daily', '-1'],
    ['Weekly', '1001'],
    ['Monthly', '1.5'],
    ['Yearly', '1001'],
    ['Max age (days)', '0'],
    ['Max age (days)', '36501'],
  ])(
    'prevents invalid %s=%s without losing other values',
    async (label, value) => {
      fixtures()
      let writes = 0
      server.use(
        http.post('/api/v1/retention', () => {
          writes++
          return HttpResponse.json(policy)
        }),
      )
      const user = mount('/protection/retention/new')
      await user.type(
        await screen.findByRole('textbox', { name: 'Policy name' }),
        'Preserved',
      )
      await user.clear(
        screen.getByRole('spinbutton', { name: label, exact: true }),
      )
      await user.type(
        screen.getByRole('spinbutton', { name: label, exact: true }),
        value,
      )
      await user.click(screen.getByRole('button', { name: 'Save' }))
      expect(
        screen.getByRole('spinbutton', { name: label, exact: true }),
      ).toHaveAttribute('aria-invalid', 'true')
      expect(screen.getByRole('textbox', { name: 'Policy name' })).toHaveValue(
        'Preserved',
      )
      expect(writes).toBe(0)
    },
  )
  it('creates retention with schema defaults and reads back the new resource', async () => {
    fixtures()
    let writes = 0
    server.use(
      http.post('/api/v1/retention', async ({ request }) => {
        writes++
        const body = await request.json()
        expect(body).toEqual({
          name: 'New rule',
          keep_last: 3,
          keep_daily: 7,
          keep_weekly: 4,
          keep_monthly: 6,
          keep_yearly: 2,
          max_age_days: 365,
        })
        return HttpResponse.json({ ...policy, name: 'New rule' })
      }),
      http.get('/api/v1/retention/7', () =>
        HttpResponse.json({ ...policy, name: 'New rule' }),
      ),
    )
    const user = mount('/protection/retention/new?q=rule')
    await user.type(
      await screen.findByRole('textbox', { name: 'Policy name' }),
      'New rule',
    )
    await user.click(screen.getByRole('button', { name: 'Save' }))
    expect(
      await screen.findByRole('heading', { name: 'New rule' }),
    ).toBeVisible()
    expect(writes).toBe(1)
    expect(screen.getByLabelText('Location')).toHaveTextContent(
      '/protection/retention/7?q=rule',
    )
  })
  it('keeps an empty numeric field empty, prevents Keep Last zero, and recovers', async () => {
    fixtures()
    let writes = 0
    server.use(
      http.post('/api/v1/retention', () => {
        writes++
        return HttpResponse.json(policy)
      }),
    )
    const user = mount('/protection/retention/new')
    await user.type(
      await screen.findByRole('textbox', { name: 'Policy name' }),
      'Rule',
    )
    const keep = screen.getByRole('spinbutton', { name: 'Keep Last' })
    await user.clear(keep)
    expect(keep).toHaveValue(null)
    await user.click(screen.getByRole('button', { name: 'Save' }))
    expect(keep).toHaveAttribute('aria-invalid', 'true')
    expect(writes).toBe(0)
    await user.type(keep, '0')
    await user.click(screen.getByRole('button', { name: 'Save' }))
    expect(writes).toBe(0)
  })
  it('retains 422 field feedback and all input on the same edit route and sends description clear', async () => {
    fixtures()
    let writes = 0
    server.use(
      http.put('/api/v1/schedules/8', async ({ request }) => {
        const body = (await request.json()) as typeof schedule
        expect(body.description).toBe('')
        writes++
        return HttpResponse.json(
          {
            detail: 'Validation error',
            errors: [
              { field: 'body.cron_expression', message: 'unsafe raw text' },
            ],
          },
          { status: 422 },
        )
      }),
    )
    const user = mount('/protection/schedules/8/edit')
    await user.clear(
      await screen.findByRole('textbox', { name: 'Description' }),
    )
    await user.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(
        screen.getByRole('textbox', { name: 'Cron expression' }),
      ).toHaveAttribute('aria-invalid', 'true'),
    )
    expect(screen.getByRole('textbox', { name: 'Description' })).toHaveValue('')
    expect(screen.getByLabelText('Location')).toHaveTextContent('/8/edit')
    expect(screen.queryByText('unsafe raw text')).not.toBeInTheDocument()
    expect(writes).toBe(1)
  })
  it('shows default fallback, disabled usage and removed FK constraints with plan links', async () => {
    fixtures()
    server.use(
      http.get('/api/v1/targets', ({ request }) => {
        expect(new URL(request.url).searchParams.get('include_removed')).toBe(
          'true',
        )
        return HttpResponse.json([
          {
            id: 1,
            name: 'Explicit',
            removed_at: null,
            target_type: 'path',
            enabled: true,
            retention_policy_id: 7,
          },
          {
            id: 2,
            name: 'Implicit',
            removed_at: null,
            target_type: 'path',
            enabled: false,
            retention_policy_id: null,
          },
          {
            id: 3,
            name: 'Removed',
            target_type: 'path',
            enabled: false,
            removed_at: '2026-01-01T00:00:00Z',
            retention_policy_id: 7,
          },
        ])
      }),
    )
    mount('/protection/retention/7')
    expect(
      await screen.findByRole('link', { name: 'Implicit' }),
    ).toHaveAttribute('href', expect.stringContaining('/protection/2'))
    expect(screen.getByRole('link', { name: 'Removed' })).toBeVisible()
    expect(screen.getByText(/Default fallback/)).toBeVisible()
    expect(screen.getByText(/Removed plan.*reference retained/)).toBeVisible()
    expect(screen.getByText(/2 effective plans/)).toBeVisible()
  })
  it('never turns failed usage into zero and cannot delete with unknown impact', async () => {
    fixtures()
    server.use(
      http.get('/api/v1/targets', () => HttpResponse.json({}, { status: 503 })),
    )
    mount('/protection/retention/7')
    expect(await screen.findByText('Usage unknown')).toBeVisible()
    expect(screen.queryByText('Not in use')).not.toBeInTheDocument()
  })
  it('retains cleanup partial/failure results inside the trapped dialog without a destructive resubmit', async () => {
    fixtures()
    let writes = 0
    server.use(
      http.post('/api/v1/retention/cleanup-orphaned', () => {
        writes++
        return HttpResponse.json({
          outcome: 'partial',
          deleted: 1,
          failed: 1,
          protected: 2,
          scan_errors: 0,
          scan_complete: true,
          freed_bytes: 6,
          freed_human: '6.00 B',
        })
      }),
    )
    const user = mount('/protection/retention')
    await user.click(await screen.findByRole('button', { name: 'Cleanup' }))
    const modal = screen.getByRole('dialog', { name: 'Cleanup Orphaned Files' })
    await user.click(
      within(modal).getByRole('button', { name: 'Cleanup', exact: true }),
    )
    expect(
      await within(modal).findByText(/Cleanup result: partial/),
    ).toBeVisible()
    expect(within(modal).getByText(/1 deleted.*1 failed/)).toBeVisible()
    for (let i = 0; i < 5; i++) {
      await user.tab()
      await waitFor(() =>
        expect(
          modal.contains(document.activeElement),
          document.activeElement?.outerHTML,
        ).toBe(true),
      )
    }
    expect(
      within(modal).queryByRole('button', { name: 'Cleanup', exact: true }),
    ).not.toBeInTheDocument()
    expect(writes).toBe(1)
  })
  it('does not mount a stale edit form before the fresh read settles', async () => {
    fixtures()
    let release!: () => void
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    server.use(
      http.get('/api/v1/schedules/8', async () => {
        await gate
        return HttpResponse.json({ ...schedule, name: 'Fresh', targets: [] })
      }),
    )
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    client.setQueryData(['schedules', 'detail', 8], {
      ...schedule,
      name: 'Stale',
    })
    mount('/protection/schedules/8/edit', client)
    expect(
      screen.queryByRole('textbox', { name: 'Schedule name' }),
    ).not.toBeInTheDocument()
    release()
    expect(
      await screen.findByRole('textbox', { name: 'Schedule name' }),
    ).toHaveValue('Fresh')
  })
  it('filters in the URL and preserves list context through detail and cancel', async () => {
    fixtures()
    const user = mount('/protection/retention?q=def')
    await user.click(
      await screen.findByRole('link', { name: 'default', exact: true }),
    )
    await user.click(await screen.findByRole('link', { name: 'Edit' }))
    await user.click(await screen.findByRole('button', { name: 'Cancel' }))
    await user.click(
      await screen.findByRole('link', { name: 'Back to Retention Policies' }),
    )
    expect(screen.getByRole('searchbox')).toHaveValue('def')
  })
})
