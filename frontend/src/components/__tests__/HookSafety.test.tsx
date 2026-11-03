import { afterEach, expect, it, vi } from 'vitest'
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
  localStorage.setItem('dockervault-wizard-draft', JSON.stringify({ currentStep: 7, savedAt: Date.now(), data: { targetType: 'path', targetName: 'Hook safety', hostPath: '/owned/source' } }))
  const close = vi.fn()
  render(<QueryClientProvider client={queryClient}><BackupWizard isOpen onClose={close} /></QueryClientProvider>)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Restore' }))
  await user.click(screen.getByRole('button', { name: 'Advanced Options' }))
  await user.type(screen.getByPlaceholderText(/pg_dump/), hooks.pre_backup_command)
  await user.type(screen.getByPlaceholderText(/rm \/backup/), hooks.post_backup_command)
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await user.dblClick(screen.getByRole('button', { name: 'Create Target' }))
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
    if (wizard) await user.click(screen.getByRole('button', { name: 'Options' }))
    await user.click(screen.getByRole('button', { name: wizard ? 'Advanced Options' : 'Advanced: Pre/Post Commands' }))
    return user
  }
  async function save() {
    if (wizard) await userEvent.click(screen.getByRole('button', { name: 'Summary' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }))
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
    await waitFor(() => fail ? expect(screen.getByRole('button', { name: 'Save Changes' })).toBeEnabled() : expect(view.close).toHaveBeenCalledOnce())
    cachesSafe()
  })
  it(`${editor} preserves unloaded hooks after a failed private load`, async () => {
    let sent: Record<string, unknown> | undefined
    server.use(http.get('/api/v1/targets/917', () => HttpResponse.json({ detail: hooks }, { status: 503 })), http.put('/api/v1/targets/917', async ({ request }) => { sent = await request.json() as Record<string, unknown>; return HttpResponse.json(target) }))
    mount()
    await options()
    await screen.findByText(/Could not load hook commands/)
    await save()
    await waitFor(() => expect(sent).toBeDefined())
    expect(Object.hasOwn(sent!, 'pre_backup_command')).toBe(false)
    expect(Object.hasOwn(sent!, 'post_backup_command')).toBe(false)
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
    fireEvent.click(screen.getByRole('button', { name: wizard ? 'Saving...' : 'Save Changes' }))
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
    release()
    await waitFor(() => expect(view.close).toHaveBeenCalledOnce())
    expect(screen.queryByDisplayValue(hooks.pre_backup_command)).not.toBeInTheDocument()
    cachesSafe()
  })
}
