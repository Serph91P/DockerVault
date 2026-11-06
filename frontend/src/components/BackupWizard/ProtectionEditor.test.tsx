import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import BackupWizard from './index'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../../store/auth'
import { initialData, type WizardData } from './planEditorModel'
import { DraftWriter } from './planEditorDraft'

const plan = {
  id: 917,
  name: 'fixture',
  target_type: 'path',
  host_path: '/fixture',
  enabled: true,
  selected_volumes: [],
  dependencies: [],
  include_paths: [],
  exclude_paths: [],
  per_volume_rules: {},
  stop_container: false,
  compression_enabled: true,
  remote_storage_ids: [],
  delete_local_after_sync: false,
}
afterEach(() => {
  cleanup()
  localStorage.clear()
  vi.restoreAllMocks()
})
async function mount(data?: Partial<WizardData>, editTarget?: import('../../api').BackupTarget) {
  useAuthStore.setState({
    user: { id: 7, username: 'fixture', is_admin: true, created_at: '', last_login: null },
    isAuthenticated: true,
  })
  if (data) {
    const writer = new DraftWriter(7, 'new', null)
    writer.queue({ ...initialData, targetType: 'path', targetName: 'fixture', hostPath: '/fixture', ...data }, 2)
    await new Promise((r) => setTimeout(r, 320))
  }
  const close = vi.fn()
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <BackupWizard editTarget={editTarget} isOpen onClose={close} />
    </QueryClientProvider>
  )
  if (data) await userEvent.click(await screen.findByRole('button', { name: 'Restore draft' }))
  return { close, client }
}
it.each([false,true])('discard resets private unsaved commands too (editing=%s)', async editing => {
  server.use(http.get('/api/v1/targets/917',()=>HttpResponse.json({...plan,pre_backup_command:'pg_dump --version'})))
  await mount(undefined, editing ? plan as import('../../api').BackupTarget : undefined)
  await userEvent.click(screen.getByRole('button',{name:'Execution & review'}))
  await userEvent.click(screen.getByRole('button',{name:'Advanced Options'}))
  const input=screen.getByRole('textbox',{name:'Pre-backup Command'})
  await waitFor(()=>expect(input).toBeEnabled());await userEvent.clear(input);await userEvent.type(input,'pg_dump unsaved-private')
  await userEvent.click(screen.getByRole('button',{name:'Discard changes'}))
  await userEvent.click(screen.getByRole('button',{name:'Execution & review'}));await userEvent.click(screen.getByRole('button',{name:'Advanced Options'}))
  await waitFor(()=>expect(screen.getByRole('textbox',{name:'Pre-backup Command'})).toHaveValue(editing?'pg_dump --version':''))
})

it('uses three named groups with real tar/gzip and no modal or Enter interception', async () => {
  await mount()
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  for (const name of ['Source & scope', 'Strategy', 'Execution & review'])
    expect(screen.getByRole('button', { name })).toHaveAttribute('type', 'button')
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  expect(screen.queryByText('Zstandard')).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: /^Gzip/ })).toBeInTheDocument()
})
it('retains confirmed inline policy ID after target validation failure and never runs a backup on save', async () => {
  let posts = 0,
    targets = 0,
    runs = 0
  let stored: Record<string, unknown> = plan
  server.use(
    http.post('/api/v1/retention', () => {
      posts++
      return HttpResponse.json({
        id: 88,
        name: 'inline',
        keep_last: 3,
        keep_daily: 7,
        keep_weekly: 4,
        keep_monthly: 6,
        keep_yearly: 2,
        max_age_days: 365,
      })
    }),
    http.post('/api/v1/targets', async ({ request }) => {
      targets++
      const body = (await request.json()) as Record<string, unknown>
      expect(body.retention_policy_id).toBe(88)
      if (targets === 1) return HttpResponse.json({ detail: 'validation' }, { status: 400 })
      stored = { ...plan, ...body }
      return HttpResponse.json(stored)
    }),
    http.get('/api/v1/targets/917', () => HttpResponse.json(stored)),
    http.post('/api/v1/backups', () => {
      runs++
      return HttpResponse.json({})
    })
  )
  const { close } = await mount({
    newRetentionPolicy: {
      name: 'inline',
      keepLast: 3,
      keepDaily: 7,
      keepWeekly: 4,
      keepMonthly: 6,
      keepYearly: 2,
      maxAgeDays: 365,
    },
  })
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText(/Could not save/)
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(posts).toBe(1)
  expect(targets).toBe(2)
  expect(runs).toBe(0)
  await new Promise((r) => setTimeout(r, 350))
  expect(Object.keys(localStorage).filter((k) => k.startsWith('dockervault-plan-draft:'))).toEqual([])
})
it('does not replay or automatically adopt an unknown target POST', async () => {
  let posts = 0
  server.use(
    http.post('/api/v1/targets', () => {
      posts++
      return HttpResponse.error()
    })
  )
  await mount({})
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText(/Unconfirmed target creation/)
  expect(screen.getByRole('button', { name: 'Save plan' })).toBeDisabled()
  expect(posts).toBe(1)
  expect(screen.getByRole('button', { name: 'Read back candidates' })).toBeInTheDocument()
})

it('keeps confirmed schedule ID when retention fails before target creation', async () => {
  let schedulePosts = 0,
    retentionPosts = 0,
    targetPosts = 0
  let stored: Record<string, unknown> = plan
  server.use(
    http.post('/api/v1/schedules', () => {
      schedulePosts++
      return HttpResponse.json({ id: 51, name: 'inline', cron_expression: '0 0 * * *', enabled: true })
    }),
    http.post('/api/v1/retention', () => {
      retentionPosts++
      return retentionPosts === 1
        ? HttpResponse.json({}, { status: 422 })
        : HttpResponse.json({ id: 52, name: 'retention', keep_last: 3, keep_daily: 7, keep_weekly: 4, keep_monthly: 6 })
    }),
    http.post('/api/v1/targets', async ({ request }) => {
      targetPosts++
      stored = { ...plan, ...((await request.json()) as object) }
      return HttpResponse.json(stored)
    }),
    http.get('/api/v1/targets/917', () => HttpResponse.json(stored))
  )
  const { close } = await mount({
    newSchedule: { name: 'inline', cronExpression: '0 0 * * *', description: '' },
    newRetentionPolicy: {
      name: 'retention',
      keepLast: 3,
      keepDaily: 7,
      keepWeekly: 4,
      keepMonthly: 6,
      keepYearly: 2,
      maxAgeDays: 365,
    },
  })
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText(/Could not save backup configuration/)
  expect(targetPosts).toBe(0)
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(schedulePosts).toBe(1)
  expect(retentionPosts).toBe(2)
  expect(stored.schedule_id).toBe(51)
})
it('unknown policy POST is never retried or adopted by name; explicit inspected selection continues', async () => {
  let posts = 0
  server.use(
    http.post('/api/v1/schedules', () => {
      posts++
      return HttpResponse.error()
    }),
    http.get('/api/v1/schedules', () =>
      HttpResponse.json([{ id: 51, name: 'inline', cron_expression: '0 0 * * *', enabled: true }])
    ),
    http.get('/api/v1/schedules/51', () =>
      HttpResponse.json({ id: 51, name: 'inline', cron_expression: '0 0 * * *', enabled: true })
    )
  )
  await mount({ newSchedule: { name: 'inline', cronExpression: '0 0 * * *', description: '' } })
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText('Unconfirmed schedule creation')
  await userEvent.click(screen.getByRole('button', { name: 'Read back candidates' }))
  const choose = await screen.findByRole('button', { name: 'Use explicitly selected #51' })
  expect(screen.getByRole('button', { name: 'Save plan' })).toBeDisabled()
  expect(posts).toBe(1)
  await userEvent.click(choose)
  await waitFor(() => expect(screen.queryByText('Unconfirmed schedule creation')).not.toBeInTheDocument())
  expect(posts).toBe(1)
})
it('a known created target ID survives failed readback; retry updates that ID instead of another POST', async () => {
  let posts = 0,
    puts = 0,
    fail = true
  let stored: Record<string, unknown> = plan
  server.use(
    http.post('/api/v1/targets', async ({ request }) => {
      posts++
      stored = { ...plan, ...((await request.json()) as object) }
      return HttpResponse.json(stored)
    }),
    http.get('/api/v1/targets/917', () => (fail ? HttpResponse.json({}, { status: 503 }) : HttpResponse.json(stored))),
    http.put('/api/v1/targets/917', async ({ request }) => {
      puts++
      stored = { ...stored, ...((await request.json()) as object) }
      return HttpResponse.json(stored)
    })
  )
  const { close } = await mount({})
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText(/Could not save backup configuration/)
  await userEvent.click(screen.getByRole('button', { name: 'Source & scope' }))
  expect(screen.queryByPlaceholderText('/path/to/directory')).not.toBeInTheDocument()
  expect(screen.getByText(/Source \(read-only\)/)).toBeInTheDocument()
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  fail = false
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(posts).toBe(1)
  expect(puts).toBe(1)
})
it('stored PUT+503 is read back and scheduler reloaded rather than saved again', async () => {
  const target = { ...plan, updated_at: '2026-11-05T00:00:00Z' } as import('../../api').BackupTarget
  let stored: Record<string, unknown> = target,
    puts = 0,
    reloads = 0,
    jobs = 0
  server.use(
    http.get('/api/v1/targets/917', () => HttpResponse.json(stored)),
    http.put('/api/v1/targets/917', async ({ request }) => {
      puts++
      stored = { ...stored, ...((await request.json()) as object) }
      return HttpResponse.json({}, { status: 503 })
    }),
    http.post('/api/v1/schedules/reload', () => {
      reloads++
      return HttpResponse.json({ status: 'reloaded' })
    }),
    http.get('/api/v1/schedules/jobs', () => {
      jobs++
      return HttpResponse.json([])
    })
  )
  const { close } = await mount(undefined, target)
  await userEvent.clear(screen.getByRole('textbox', { name: 'Plan name' }))
  await userEvent.type(screen.getByRole('textbox', { name: 'Plan name' }), 'changed')
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(puts).toBe(1)
  expect(reloads).toBe(1)
  expect(jobs).toBe(1)
  expect(stored.name).toBe('changed')
})
it('revalidates changed topology and never posts an empty or vanished explicit subset', async () => {
  let posts = 0
  server.use(
    http.post('/api/v1/targets', () => {
      posts++
      return HttpResponse.json(plan)
    }),
    http.get('/api/v1/docker/containers', () =>
      HttpResponse.json([
        {
          id: 'owned',
          name: 'service',
          status: 'running',
          mounts: [{ type: 'volume', name: 'other', destination: '/other' }],
        },
      ])
    )
  )
  await mount({
    targetType: 'container',
    containerName: 'service',
    hostPath: '',
    volumeMode: 'subset',
    selectedVolumes: [],
  })
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText('Select at least one volume or explicitly choose all volumes.')
  expect(posts).toBe(0)
  await userEvent.click(screen.getByRole('button', { name: 'Source & scope' }))
  await userEvent.click(screen.getByRole('checkbox', { name: 'other' }))
  // A fresh topology change must be checked at Save, not silently filter to [].
  server.use(
    http.get('/api/v1/docker/containers', () =>
      HttpResponse.json([{ id: 'owned', name: 'service', status: 'running', mounts: [] }])
    )
  )
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText('Selected volumes are missing from current discovery. Recheck scope.')
  expect(posts).toBe(0)
})
it('does not overwrite a changed server revision without an explicit comparison choice', async () => {
  const target = { ...plan, updated_at: '2026-11-05T00:00:00Z' } as import('../../api').BackupTarget
  let puts = 0
  server.use(
    http.get('/api/v1/targets/917', () =>
      HttpResponse.json({ ...target, updated_at: '2026-11-05T01:00:00Z', name: 'server changed' })
    ),
    http.put('/api/v1/targets/917', () => {
      puts++
      return HttpResponse.json(target)
    })
  )
  await mount(undefined, target)
  await userEvent.click(screen.getByRole('button', { name: 'Execution & review' }))
  await userEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  await screen.findByText(/Server revision changed/)
  expect(puts).toBe(0)
  expect(screen.getByRole('button', { name: 'Save plan' })).toBeDisabled()
})
