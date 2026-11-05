import { afterEach, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import BackupWizard from './index'
import { initialData, type WizardData } from './planEditorModel'
import { DraftWriter } from './planEditorDraft'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../../store/auth'

const root = '/api/v1'
const plan = {
  id: 3, name: 'fixture', target_type: 'path', host_path: '/fixture', enabled: true,
  selected_volumes: [], dependencies: [], include_paths: [], exclude_paths: [], per_volume_rules: {},
  stop_container: true, compression_enabled: true, remote_storage_ids: [], delete_local_after_sync: false,
  updated_at: '2026-11-05T00:00:00Z',
}
const schedule = { id: 1, name: 'inline schedule', cron_expression: '0 0 * * *', enabled: true, description: '' }
const policy = { id: 2, name: 'inline retention', keep_last: 3, keep_daily: 7, keep_weekly: 4, keep_monthly: 6, keep_yearly: 2, max_age_days: 365 }
const newSchedule = { name: schedule.name, cronExpression: schedule.cron_expression, description: '' }
const newRetentionPolicy = { name: policy.name, keepLast: 3, keepDaily: 7, keepWeekly: 4, keepMonthly: 6, keepYearly: 2, maxAgeDays: 365 }
const save = () => userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
const read = () => userEvent.click(screen.getByRole('button', { name: 'Read back candidates' }))
async function failed(kind: string) {
  await screen.findByText(`Unconfirmed ${kind} creation`)
  await waitFor(() => expect(screen.getByRole('button', { name: 'Read back candidates' })).toBeEnabled())
}
afterEach(() => { cleanup(); localStorage.clear(); vi.restoreAllMocks() })
async function mount(extra: Partial<WizardData> = {}) {
  useAuthStore.setState({ user: { id: 7, username: 'fixture', is_admin: true, created_at: '', last_login: null }, isAuthenticated: true })
  new DraftWriter(7, 'new', null).checkpoint({ ...initialData, targetType: 'path', targetName: plan.name, hostPath: '/fixture', ...extra }, 2)
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const saved = vi.fn()
  render(<QueryClientProvider client={client}><BackupWizard isOpen onClose={() => {}} onSaved={saved} /></QueryClientProvider>)
  await userEvent.click(await screen.findByRole('button', { name: 'Restore draft' }))
  return { saved, client }
}
function defer() {
  let resolve!: (value: Response) => void
  const promise = new Promise<Response>(done => { resolve = done })
  return { promise, resolve }
}

it.each([true, false])('requires fresh typed selection through Schedule → optional Retention → Target with colliding IDs and no duplicate POST (retention=%s)', async withRetention => {
  const posts = { schedule: 0, retention: 0, target: 0 }
  const gets: string[] = []
  server.use(
    http.post(root + '/schedules', () => { posts.schedule++; return HttpResponse.error() }),
    http.post(root + '/retention', () => { posts.retention++; return HttpResponse.error() }),
    http.post(root + '/targets', () => { posts.target++; return HttpResponse.error() }),
    http.get(root + '/schedules', () => HttpResponse.json([schedule])),
    http.get(root + '/retention', () => HttpResponse.json([{ ...policy, id: 1, name: 'foreign default' }, policy])),
    http.get(root + '/targets', () => HttpResponse.json([{ ...plan, id: 1, name: 'foreign plan' }, plan])),
    http.get(root + '/schedules/1', () => { gets.push('schedule:1'); return HttpResponse.json(schedule) }),
    http.get(root + '/retention/:id', ({ params }) => { gets.push(`retention:${params.id}`); return HttpResponse.json(policy) }),
    http.get(root + '/targets/:id', ({ params }) => { gets.push(`target:${params.id}`); return HttpResponse.json(plan) }),
  )
  const { saved } = await mount({ newSchedule, ...(withRetention ? { newRetentionPolicy } : {}) })
  await save(); await failed('schedule'); await read()
  await userEvent.click(await screen.findByRole('button', { name: 'Use explicitly selected #1' }))
  await waitFor(() => expect(screen.queryByText('Unconfirmed schedule creation')).not.toBeInTheDocument())
  if (withRetention) {
    await save(); await failed('retention')
    expect(screen.queryByRole('button', { name: 'Use explicitly selected #1' })).not.toBeInTheDocument()
    expect(screen.queryByText(/inline schedule \(#1\)/)).not.toBeInTheDocument()
    await read(); await userEvent.click(await screen.findByRole('button', { name: 'Use explicitly selected #2' }))
    await waitFor(() => expect(screen.queryByText('Unconfirmed retention creation')).not.toBeInTheDocument())
  }
  await save(); await failed('target')
  expect(screen.queryByRole('button', { name: /Open and inspect #[12]$/ })).not.toBeInTheDocument()
  expect(saved).not.toHaveBeenCalled()
  expect(Object.keys(localStorage).some(k => k.startsWith('dockervault-plan-draft:'))).toBe(true)
  await read(); await userEvent.click(await screen.findByRole('button', { name: 'Open and inspect #3' }))
  await waitFor(() => expect(saved).toHaveBeenCalledWith(3))
  expect(posts).toEqual({ schedule: 1, retention: withRetention ? 1 : 0, target: 1 })
  expect(gets).toEqual(withRetention ? ['schedule:1', 'retention:2', 'target:3'] : ['schedule:1', 'target:3'])
  expect(Object.keys(localStorage).filter(k => k.startsWith('dockervault-plan-draft:'))).toEqual([])
})

it.each(['success', 'failure'])('ignores a late %s from the previous readback after a newer read completes', async outcome => {
  const late = defer()
  let reads = 0
  server.use(
    http.post(root + '/schedules', () => HttpResponse.error()),
    http.get(root + '/schedules', () => { reads++; return reads === 2 ? late.promise : HttpResponse.json([schedule]) }),
  )
  await mount({ newSchedule }); await save(); await failed('schedule')
  await read(); await waitFor(() => expect(reads).toBe(2))
  await read(); await screen.findByRole('button', { name: 'Use explicitly selected #1' })
  await act(async () => { late.resolve(outcome === 'success' ? HttpResponse.json([{ ...schedule, id: 99 }]) : HttpResponse.error()) })
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Use explicitly selected #99' })).not.toBeInTheDocument())
  expect(screen.getByRole('button', { name: 'Use explicitly selected #1' })).toBeInTheDocument()
  expect(screen.queryByText(/Candidate readback unavailable/)).not.toBeInTheDocument()
})

it('ignores a delayed schedule readback after explicit resolution and a retention POST uncertainty', async () => {
  const late = defer()
  let reads = 0
  server.use(
    http.post(root + '/schedules', () => HttpResponse.error()),
    http.post(root + '/retention', () => HttpResponse.error()),
    http.get(root + '/schedules', () => { reads++; return reads === 2 ? late.promise : HttpResponse.json([schedule]) }),
    http.get(root + '/schedules/1', () => HttpResponse.json(schedule)),
  )
  await mount({ newSchedule, newRetentionPolicy }); await save(); await failed('schedule')
  await read(); await waitFor(() => expect(reads).toBe(2)); await read()
  await userEvent.click(await screen.findByRole('button', { name: 'Use explicitly selected #1' }))
  await waitFor(() => expect(screen.queryByText('Unconfirmed schedule creation')).not.toBeInTheDocument())
  await save(); await failed('retention')
  await act(async () => { late.resolve(HttpResponse.json([schedule])) })
  expect(screen.queryByRole('button', { name: 'Use explicitly selected #1' })).not.toBeInTheDocument()
  expect(screen.queryByText(/inline schedule \(#1\)/)).not.toBeInTheDocument()
})

it('invalidates pending readback on Discard, even when the next unknown intent has the same type and name', async () => {
  const late = defer()
  let reads = 0
  server.use(
    http.post(root + '/targets', () => HttpResponse.error()),
    http.get(root + '/targets', () => { reads++; return reads === 2 ? late.promise : HttpResponse.json([plan]) }),
  )
  const { saved } = await mount(); await save(); await failed('target'); await read()
  await waitFor(() => expect(reads).toBe(2))
  await userEvent.click(screen.getByRole('button', { name: 'Discard changes' }))
  await userEvent.click(screen.getByRole('button', { name: /Host Path Backup a directory/ }))
  await userEvent.type(screen.getByPlaceholderText('/path/to/directory'), '/fixture')
  await userEvent.clear(screen.getByRole('textbox', { name: 'Backup name' }))
  await userEvent.type(screen.getByRole('textbox', { name: 'Backup name' }), plan.name)
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  await save(); await failed('target')
  await act(async () => { late.resolve(HttpResponse.json([plan])) })
  expect(screen.queryByRole('button', { name: 'Open and inspect #3' })).not.toBeInTheDocument()
  expect(saved).not.toHaveBeenCalled()
  await read(); await screen.findByRole('button', { name: 'Open and inspect #3' })
})

it.each([true, false])('invalidates completed/pending candidates on an intent edit (pending=%s)', async pending => {
  const late = defer()
  let reads = 0
  server.use(
    http.post(root + '/targets', () => HttpResponse.error()),
    http.get(root + '/targets', () => { reads++; return pending && reads === 2 ? late.promise : HttpResponse.json([plan]) }),
  )
  await mount(); await save(); await failed('target'); await read()
  if (pending) await waitFor(() => expect(reads).toBe(2))
  else await screen.findByRole('button', { name: 'Open and inspect #3' })
  await userEvent.click(screen.getByRole('button', { name: 'Source & scope' }))
  await userEvent.type(screen.getByRole('textbox', { name: 'Backup name' }), ' changed')
  if (pending) await act(async () => { late.resolve(HttpResponse.json([plan])) })
  expect(screen.queryByRole('button', { name: 'Open and inspect #3' })).not.toBeInTheDocument()
})

it('cancels a pending candidate choice without navigation or draft removal', async () => {
  const late = defer()
  server.use(
    http.post(root + '/targets', () => HttpResponse.error()),
    http.get(root + '/targets', () => HttpResponse.json([plan])),
    http.get(root + '/targets/3', () => late.promise),
  )
  const { saved } = await mount(); await save(); await failed('target'); await read()
  await userEvent.click(await screen.findByRole('button', { name: 'Open and inspect #3' }))
  // Group controls remain available while the private request is pending; Discard cancels it.
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
  await act(async () => { late.resolve(HttpResponse.json(plan)) })
  expect(saved).not.toHaveBeenCalled()
  expect(Object.keys(localStorage).some(k => k.startsWith('dockervault-plan-draft:'))).toBe(true)
})

it.each(['', '   ', '  pg_dump --version  '])('normalizes only empty/whitespace hooks before private write and readback (%j)', async body => {
  let stored: Record<string, unknown> = plan
  let payload: Record<string, unknown> = {}
  server.use(
    http.post(root + '/targets', async ({ request }) => {
      payload = await request.json() as Record<string, unknown>
      stored = { ...plan, ...payload, pre_backup_command: body.trim() ? body : null, post_backup_command: body.trim() ? body : null }
      return HttpResponse.json(stored)
    }),
    http.get(root + '/targets/3', () => HttpResponse.json(stored)),
  )
  const { saved, client } = await mount()
  await userEvent.click(screen.getByRole('button', { name: 'Advanced Options' }))
  for (const name of ['Pre-backup Command', 'Post-backup Command']) {
    if (body) await userEvent.type(screen.getByRole('textbox', { name }), body)
  }
  await save(); await waitFor(() => expect(saved).toHaveBeenCalledWith(3))
  expect(payload.pre_backup_command).toBe(body.trim() ? body : null)
  expect(payload.post_backup_command).toBe(body.trim() ? body : null)
  if (body.trim()) {
    expect(JSON.stringify(client.getQueryCache().getAll())).not.toContain(body)
    expect(JSON.stringify(localStorage)).not.toContain(body)
  }
})
