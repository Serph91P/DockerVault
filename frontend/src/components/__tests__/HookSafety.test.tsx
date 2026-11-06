import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import api, { targetsApi, type BackupTarget } from '../../api'
import { clearSession, queryClient } from '../../store/session'
import { server } from '../../test/mocks/server'
import { useNotificationStore } from '../../store/notifications'
import BackupEditDialog from '../BackupEditDialog'
import BackupWizard from '../BackupWizard'
import { useAuthStore } from '../../store/auth'
import { initialData } from '../BackupWizard/planEditorModel'
import { draftKey } from '../BackupWizard/planEditorDraft'
beforeEach(() => useAuthStore.setState({ user: { id: 7, username: 'fixture', is_admin: true, created_at: '', last_login: null }, isAuthenticated: true }))

const hooks = { pre_backup_command: 'synthetic confidential pre body', post_backup_command: 'synthetic confidential post body' }
const target = { id: 917, name: 'Hook safety', target_type: 'path', host_path: '/owned/source', enabled: true, dependencies: [], selected_volumes: [], include_paths: [], exclude_paths: [], per_volume_rules: {}, stop_container: false, compression_enabled: true, remote_storage_ids: [], delete_local_after_sync: false } as unknown as BackupTarget
function safe(value: unknown) {
  const text = JSON.stringify(value, (_key, entry) => entry instanceof Error ? { message: entry.message, ...Object.fromEntries(Object.getOwnPropertyNames(entry).map(key => [key, (entry as unknown as Record<string, unknown>)[key]])) } : entry) ?? ''
  expect(Object.values(hooks).some(body => text.includes(body))).toBe(false)
}
function cachesSafe() {
  safe(queryClient.getQueryCache().getAll().map(q => [q.queryKey, q.state]))
  safe(queryClient.getMutationCache().getAll().map(m => [m.options.mutationKey, m.state]))
  safe([localStorage, sessionStorage, location.href])
  safe(useNotificationStore.getState().notifications)
}
afterEach(() => { cleanup(); clearSession(); vi.restoreAllMocks() })

it.each(['list', 'detail', 'toggle', 'schedule'] as const)('redacts ordinary %s responses before shared caches, preserving non-hook fields', async kind => {
  server.use(http.get('/api/v1/targets', () => HttpResponse.json([{ ...target, ...hooks }])), http.get('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks })), http.put('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks })))
  const read = () => kind === 'list' ? targetsApi.list() : kind === 'detail' ? targetsApi.get(917) : targetsApi.update(917, kind === 'toggle' ? { enabled: false } : { schedule_id: null, schedule_cron: null })
  const result = kind === 'list' || kind === 'detail'
    ? await queryClient.fetchQuery({ queryKey: ['targets', kind], queryFn: read })
    : await queryClient.getMutationCache().build(queryClient, { mutationFn: read }).execute(undefined)
  for (const [key, value] of Object.entries(result)) {
    expect(Object.values(hooks).some(body => (JSON.stringify(value) ?? '').includes(body)), `unsafe response field ${key}`).toBe(false)
  }
  expect(kind === 'list' ? result.data[0] : result.data).toEqual(target)
  cachesSafe()
})

it.each(['list', 'detail', 'toggle', 'schedule'])('does not cache %s validation echoes or raw Axios context on failure', async kind => {
  server.use(http.get('/api/v1/targets', () => HttpResponse.json({ detail: hooks }, { status: 503 })), http.get('/api/v1/targets/917', () => HttpResponse.json({ detail: hooks }, { status: 503 })), http.put('/api/v1/targets/917', () => HttpResponse.json({ detail: hooks }, { status: 422 })))
  if (kind === 'list' || kind === 'detail') await queryClient.fetchQuery({ queryKey: ['targets', kind], queryFn: () => kind === 'list' ? targetsApi.list() : targetsApi.get(917), retry: false }).catch(() => undefined)
  else await queryClient.getMutationCache().build(queryClient, { mutationFn: () => api.put('/targets/917', kind === 'toggle' ? { enabled: false } : { schedule_id: null }) }).execute(undefined).catch(() => undefined)
  cachesSafe()
})

it('creates through the existing wizard with private hooks and one request, never a secret mutation', async () => {
  let release!: () => void
  let calls = 0
  let correct = false
  const gate = new Promise<void>(resolve => { release = resolve })
  server.use(http.post('/api/v1/targets', async ({ request }) => {
    calls++
    const body = await request.json() as Record<string, unknown>
    correct = body.pre_backup_command === hooks.pre_backup_command && body.post_backup_command === hooks.post_backup_command
    await gate
    return HttpResponse.json({ ...target, ...hooks })
  }))
  server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks, name: 'Hook safety', schedule_id: null, schedule_cron: null, retention_policy_id: null, stop_container: true, enabled: true })))
  localStorage.setItem(draftKey(7, 'new'), JSON.stringify({ version: 1, owner: 7, intent: 'new', revision: null, group: 2, savedAt: Date.now(), data: { ...initialData, targetType: 'path', targetName: 'Hook safety', hostPath: '/owned/source' } }))
  const close = vi.fn()
  render(<QueryClientProvider client={queryClient}><BackupWizard isOpen onClose={close} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Restore draft' }))
  await user.click(screen.getByRole('button', { name: 'Advanced Options' }))
  await user.type(screen.getByPlaceholderText(/pg_dump/), hooks.pre_backup_command)
  await user.type(screen.getByPlaceholderText(/rm \/backup/), hooks.post_backup_command)
  await user.dblClick(screen.getByRole('button', { name: 'Save plan' }))
  await waitFor(() => expect(calls).toBe(1))
  expect(correct).toBe(true)
  cachesSafe()
  release()
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(localStorage.getItem('dockervault-wizard-draft')).toBeNull()
  cachesSafe()
})

for (const wizard of [false, true]) {
  const editor = wizard ? 'wizard' : 'dialog'
  function mount(close = vi.fn()) {
    const element = (id = 917, open = true) => <QueryClientProvider client={queryClient}>{wizard ? <BackupWizard editTarget={{ ...target, id }} isOpen={open} onClose={close} /> : <BackupEditDialog target={{ ...target, id }} isOpen={open} onClose={close} />}</QueryClientProvider>
    return { ...render(element()), element, close }
  }
  async function options() {
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'Execution & review' }))
    await user.click(screen.getByRole('button', { name: 'Advanced Options' }))
    return user
  }
  async function save() {
    fireEvent.click(screen.getByRole('button', { name: 'Save plan' }))
  }
  it.each([false, true])(`${editor} privately loads and saves hooks without caching pending/success/error payloads (error=%s)`, async fail => {
    let release!: () => void
    let received = false
    const gate = new Promise<void>(r => { release = r })
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks })), http.put('/api/v1/targets/917', async ({ request }) => {
      const body = await request.json() as Record<string, unknown>
      received = body.pre_backup_command === hooks.pre_backup_command && body.post_backup_command === hooks.post_backup_command
      await gate
      return HttpResponse.json({ ...target, ...hooks, detail: hooks }, { status: fail ? 422 : 200 })
    }))
    const view = mount()
    await options()
    await screen.findByDisplayValue(hooks.pre_backup_command)
    await save()
    await waitFor(() => expect(received).toBe(true))
    cachesSafe()
    release()
    await waitFor(() => fail ? expect(screen.getByRole('button', { name: 'Save plan' })).toBeEnabled() : expect(view.close).toHaveBeenCalledOnce())
    cachesSafe()
  })
  it.each(['failed', 'pending'])(`${editor} preserves unloaded hooks after a %s private load`, async state => {
    let sent: Record<string, unknown> | undefined
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    server.use(http.get('/api/v1/targets/917', async () => { if (state === 'pending') await gate; return HttpResponse.json({ detail: hooks }, { status: 503 }) }), http.put('/api/v1/targets/917', async ({ request }) => { sent = await request.json() as Record<string, unknown>; return HttpResponse.json(target) }))
    mount()
    await options()
    await screen.findByText(state === 'failed' ? /Could not load hook commands/ : /Loading private hook commands/)
    expect(screen.getByPlaceholderText(/pg_dump/)).toBeDisabled()
    await save()
    await waitFor(() => expect(sent).toBeDefined())
    expect(Object.hasOwn(sent!, 'pre_backup_command')).toBe(false)
    expect(Object.hasOwn(sent!, 'post_backup_command')).toBe(false)
    release()
    expect(queryClient.getMutationCache().getAll()).toHaveLength(0)
    cachesSafe()
  })
  it(`${editor} drops late loads across close/reopen and target switches`, async () => {
    let release!: () => void
    let started = false
    const abort = vi.spyOn(XMLHttpRequest.prototype, 'abort')
    const gate = new Promise<void>(resolve => { release = resolve })
    server.use(http.get('/api/v1/targets/917', async () => { started = true; await gate; return HttpResponse.json({ ...target, ...hooks }) }), http.get('/api/v1/targets/918', () => HttpResponse.json({ ...target, id: 918, pre_backup_command: 'next hook', post_backup_command: null })))
    const view = mount()
    await waitFor(() => expect(started).toBe(true))
    view.rerender(view.element(917, false))
    view.rerender(view.element(918))
    // Axios uses XHR here; MSW's synthetic Request.signal does not mirror XHR.abort.
    await waitFor(() => expect(abort).toHaveBeenCalled())
    const user = await options()
    const input = await screen.findByDisplayValue('next hook')
    await user.clear(input)
    await user.type(input, 'newer input')
    release()
    await waitFor(() => expect(screen.getByDisplayValue('newer input')).toBeInTheDocument())
    expect(screen.queryByDisplayValue(hooks.pre_backup_command)).not.toBeInTheDocument()
    cachesSafe()
  })
  it(`${editor} cancels private save on logout and ignores late success, with duplicate submits guarded`, async () => {
    let release!: () => void
    let calls = 0
    const gate = new Promise<void>(resolve => { release = resolve })
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks })), http.put('/api/v1/targets/917', async () => { calls++; await gate; return HttpResponse.json({ ...target, ...hooks }) }))
    const view = mount()
    await options()
    await screen.findByDisplayValue(hooks.pre_backup_command)
    await save()
    await waitFor(() => expect(calls).toBe(1))
    fireEvent.click(screen.getByRole('button', { name: 'Saving…' }))
    clearSession()
    release()
    await waitFor(() => expect(screen.queryByDisplayValue(hooks.pre_backup_command)).not.toBeInTheDocument())
    expect(view.close).not.toHaveBeenCalled()
    expect(calls).toBe(1)
    cachesSafe()
  })
  it(`${editor} retries a failed load without clearing commands`, async () => {
    let failed = true
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json(failed ? {} : { ...target, ...hooks }, { status: failed ? 503 : 200 })))
    mount()
    await screen.findByRole('button', { name: 'Retry hook load' })
    failed = false
    await userEvent.click(screen.getByRole('button', { name: 'Retry hook load' }))
    await options()
    await screen.findByDisplayValue(hooks.pre_backup_command)
    cachesSafe()
  })
  it(`${editor} revokes a save immediately on Cancel even before the parent unmounts`, async () => {
    let release!: () => void
    let started = false
    const gate = new Promise<void>(resolve => { release = resolve })
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ ...target, ...hooks })), http.put('/api/v1/targets/917', async () => { started = true; await gate; return HttpResponse.json({ ...target, ...hooks }) }))
    const view = mount()
    await options()
    await screen.findByDisplayValue(hooks.pre_backup_command)
    await save()
    await waitFor(() => expect(started).toBe(true))
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(view.close).toHaveBeenCalledOnce()
    expect(screen.queryByDisplayValue(hooks.pre_backup_command)).not.toBeInTheDocument()
    release()
    await waitFor(() => expect(view.close).toHaveBeenCalledOnce())
    expect(screen.queryByDisplayValue(hooks.pre_backup_command)).not.toBeInTheDocument()
    cachesSafe()
  })
}
