import { afterEach, expect, it } from 'vitest'
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import ProtectionPlan from '../ProtectionPlan'
import { server } from '../../test/mocks/server'
import { useAuthStore } from '../../store/auth'
import { clearSession, queryClient } from '../../store/session'
import { queryKeys } from '../../api/queryKeys'
import type { BackupTarget } from '../../api'
import { DraftWriter } from '../../components/BackupWizard/planEditorDraft'
import { targetToWizardData } from '../../components/BackupWizard/planEditorModel'

// Real route/editor/cache/API adapter; only HTTP and storage are synthetic.
const fixture: BackupTarget = {
  id: 917, name: 'Earlier plan', target_type: 'path', host_path: '/synthetic',
  enabled: true, removed_at: null, schedule_id: null, schedule_cron: null,
  retention_policy_id: null, dependencies: [], selected_volumes: [],
  include_paths: [], exclude_paths: [], per_volume_rules: {},
  stop_container: false, compression_enabled: true,
  remote_storage_ids: [], delete_local_after_sync: false,
  pre_backup_command: null, post_backup_command: null,
  created_at: '2026-11-01T00:00:00Z', updated_at: '2026-11-01T00:00:00Z',
}
const firstRevision = '2026-11-01T00:00:01Z'
let releaseOutstanding = () => {}
afterEach(() => {
  releaseOutstanding()
  cleanup()
  clearSession()
  localStorage.clear()
  sessionStorage.clear()
})
function mount() {
  clearSession()
  useAuthStore.setState({ user: { id: 7, username: 'synthetic', is_admin: true, created_at: '', last_login: null }, isAuthenticated: true })
  server.use(...['schedules', 'retention', 'storage', 'backups', 'docker/containers', 'docker/stacks', 'docker/volumes'].map(path =>
    http.get('/api/v1/' + path, () => HttpResponse.json([]))))
  queryClient.setQueryData(queryKeys.targets.detail(917), { ...fixture })
  return render(<QueryClientProvider client={queryClient}><MemoryRouter initialEntries={['/protection/917/edit']}><Routes>
    <Route path="/protection/:id/edit" element={<ProtectionPlan editing />} />
    <Route path="/protection/:id" element={<ProtectionPlan />} />
  </Routes></MemoryRouter></QueryClientProvider>)
}
async function nameInput() { return screen.findByRole('textbox', { name: 'Plan name', exact: true }) }
async function review() { await userEvent.click(await screen.findByRole('button', { name: 'Execution & review', exact: true })) }
async function hookInput() {
  await review()
  await userEvent.click(screen.getByRole('button', { name: 'Advanced Options', exact: true }))
  const input = screen.getByRole('textbox', { name: 'Pre-backup Command', exact: true })
  await waitFor(() => expect(input).toBeEnabled())
  return input
}
async function changeNameAndSave(name: string) {
  await userEvent.click(screen.getByRole('button', { name: 'Source & scope', exact: true }))
  const input = await nameInput()
  await userEvent.clear(input)
  await userEvent.type(input, name)
  await review()
  await userEvent.click(screen.getByRole('button', { name: 'Save plan', exact: true }))
}
it.each(['ordinary', 'refetch-before-reedit', 'refetch-after-reedit'] as const)(
  'own successful save followed by immediate re-edit permits another save (%s)', async timing => {
    let stored = { ...fixture }, writes = 0, readsAfterWrite = 0, held = 0
    let released = timing === 'ordinary'
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    releaseOutstanding = () => { released = true; release() }
    server.use(
      http.get('/api/v1/targets/917', async () => {
        // Permit verified private write readback; hold only the subsequent query GET.
        if (writes === 1 && readsAfterWrite++ > 0 && !released) { held++; await gate }
        return HttpResponse.json(stored)
      }),
      http.get('/api/v1/targets', () => HttpResponse.json([stored])),
      http.put('/api/v1/targets/917', async ({ request }) => {
        writes++
        stored = { ...stored, ...await request.json() as Partial<BackupTarget>, updated_at: firstRevision }
        return HttpResponse.json(stored)
      }),
    )
    mount()
    await hookInput()
    await changeNameAndSave('First saved plan')
    await screen.findByRole('link', { name: 'Edit plan', exact: true })
    expect(writes).toBe(1)
    expect(stored.name).toBe('First saved plan')
    if (timing !== 'ordinary') await waitFor(() => expect(held).toBeGreaterThan(0))
    if (timing === 'refetch-before-reedit') await act(async () => { releaseOutstanding() })
    if (timing !== 'refetch-after-reedit')
      await waitFor(() => expect(queryClient.getQueryData<BackupTarget>(queryKeys.targets.detail(917))?.updated_at).toBe(firstRevision))
    await userEvent.click(screen.getByRole('link', { name: 'Edit plan', exact: true }))
    if (timing === 'refetch-after-reedit') await act(async () => { releaseOutstanding() })
    await nameInput()
    await changeNameAndSave('Second saved plan')
    await waitFor(() => expect(writes).toBe(2))
    expect(screen.queryByText(/Server revision changed\./)).not.toBeInTheDocument()
    expect(stored.name).toBe('Second saved plan')
    await screen.findByRole('link', { name: 'Edit plan', exact: true })
  },
)
it.each([403, 404, 500])('fresh edit initialization never falls back to stale cached data after GET %s', async status => {
  server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({}, { status })))
  mount()
  await screen.findByText(/Could not load|Access denied|No plan at this address/)
  expect(screen.queryByRole('textbox', { name: 'Plan name', exact: true })).not.toBeInTheDocument()
  expect(screen.queryByRole('button', { name: 'Save plan', exact: true })).not.toBeInTheDocument()
  server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ ...fixture, name: 'Fresh retry', updated_at: firstRevision })))
  await userEvent.click(screen.getByRole('button', { name: 'Retry Plan #917', exact: true }))
  expect(await nameInput()).toHaveValue('Fresh retry')
})
it.each([false, true])('initial read gates stale form data and compares drafts against the fresh revision (draft=%s)', async draft => {
  let release!: () => void, hit!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  const entered = new Promise<void>(resolve => { hit = resolve })
  releaseOutstanding = release
  server.use(http.get('/api/v1/targets/917', async () => {
    hit(); await gate
    return HttpResponse.json({ ...fixture, name: 'Fresh entry', updated_at: firstRevision, pre_backup_command: 'pg_dump stored-private-marker' })
  }))
  mount()
  await entered
  expect(screen.queryByRole('textbox', { name: 'Plan name', exact: true })).not.toBeInTheDocument()
  if (draft) new DraftWriter(7, 'target:917', fixture.updated_at).checkpoint({ ...targetToWizardData(fixture), targetName: 'Draft intent' }, 0)
  await act(async () => { release() })
  expect(await nameInput()).toHaveValue('Fresh entry')
  expect(JSON.stringify(queryClient.getQueryCache().getAll().map(q => q.state))).not.toContain('stored-private-marker')
  if (draft) {
    await userEvent.click(screen.getByRole('button', { name: 'Restore draft', exact: true }))
    await screen.findByText(/Server revision changed\./)
    expect(await nameInput()).toHaveValue('Fresh entry')
    await review()
    expect(screen.getByRole('button', { name: 'Save plan', exact: true })).toBeDisabled()
  }
})
it.each(['unchanged', 'changed', 'failed'] as const)('background refetch preserves dirty safe/private inputs (%s)', async outcome => {
  let stored = { ...fixture }, failed = false, puts = 0
  server.use(
    http.get('/api/v1/targets/917', () => HttpResponse.json(stored, { status: failed ? 500 : 200 })),
    http.get('/api/v1/targets', () => HttpResponse.json([stored])),
    http.put('/api/v1/targets/917', async ({ request }) => {
      puts++
      stored = { ...stored, ...await request.json() as Partial<BackupTarget> }
      return HttpResponse.json(stored)
    }),
  )
  mount()
  const input = await nameInput()
  await userEvent.clear(input); await userEvent.type(input, 'Dirty safe intent')
  const hook = await hookInput()
  await userEvent.type(hook, 'pg_dump private-unsaved-marker')
  let release!: () => void, hit!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  const entered = new Promise<void>(resolve => { hit = resolve })
  releaseOutstanding = release
  server.use(http.get('/api/v1/targets/917', async () => { hit(); await gate; return HttpResponse.json(stored, { status: failed ? 500 : 200 }) }))
  let refetch!: Promise<unknown>
  act(() => { refetch = queryClient.invalidateQueries({ queryKey: queryKeys.targets.detail(917) }) })
  await entered
  expect(hook).toBeInTheDocument()
  expect(hook).toHaveValue('pg_dump private-unsaved-marker')
  if (outcome === 'changed') stored = { ...stored, name: 'External writer', updated_at: firstRevision }
  failed = outcome === 'failed'
  await act(async () => { release(); await refetch })
  expect(hook).toHaveValue('pg_dump private-unsaved-marker')
  await userEvent.click(screen.getByRole('button', { name: 'Source & scope', exact: true }))
  expect(await nameInput()).toHaveValue('Dirty safe intent')
  expect(JSON.stringify(queryClient.getQueryCache().getAll().map(q => q.state))).not.toContain('private-unsaved-marker')
  expect(JSON.stringify(localStorage)).not.toContain('private-unsaved-marker')
  failed = false
  await review()
  await userEvent.click(screen.getByRole('button', { name: 'Save plan', exact: true }))
  if (outcome === 'changed') {
    await screen.findByText(/Server revision changed\./)
    expect(puts).toBe(0)
    expect(screen.getByRole('button', { name: 'Save plan', exact: true })).toBeDisabled()
  } else {
    await screen.findByRole('link', { name: 'Edit plan', exact: true })
    expect(puts).toBe(1)
    expect(stored.name).toBe('Dirty safe intent')
    expect(stored.pre_backup_command).toBe('pg_dump private-unsaved-marker')
  }
})
it('session abort rejects a delayed initial read before the next account can initialize an editor', async () => {
  let release!: () => void, hit!: () => void
  const gate = new Promise<void>(resolve => { release = resolve })
  const entered = new Promise<void>(resolve => { hit = resolve })
  releaseOutstanding = release
  server.use(http.get('/api/v1/targets/917', async () => { hit(); await gate; return HttpResponse.json(fixture) }))
  mount()
  await entered
  await act(async () => {
    clearSession()
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({}, { status: 403 })))
    useAuthStore.setState({ user: { id: 8, username: 'other', is_admin: false, created_at: '', last_login: null } })
    release()
  })
  await waitFor(() => expect(queryClient.isFetching()).toBe(0))
  expect(screen.queryByRole('textbox', { name: 'Plan name', exact: true })).not.toBeInTheDocument()
})
