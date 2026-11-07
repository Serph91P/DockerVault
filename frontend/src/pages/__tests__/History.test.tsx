import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, Routes, Route, useLocation } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import { expect, it } from 'vitest'
import { server } from '../../test/mocks/server'
import History from '../History'

const row = { id: 507, target_id: 1, target_name: 'Retained source', backup_type: 'full', status: 'completed', created_at: '2026-01-01T00:00:00Z', file_size: 7, file_size_human: '7 B', encrypted: false, local_available: true, remote_synced: false, remote_sync_details: [], source_type: 'path', source_identity: '/fixture', plan_removed: true, retention_context: 'Retained after plan removal', archive_state: 'recorded', local_state: 'available', can_restore: true, can_sync: false }
const page = { items: [row], total: 507, offset: 0, limit: 50, snapshot: 507, dataset: 'Catalog placement, not a live availability scan' }
function Location() { const l = useLocation(); return <output aria-label="Location">{l.pathname}{l.search}</output> }
function mount(path = '/history?q=Retained&custom=keep', client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[path]}><Location /><Routes><Route path="/history/*" element={<History />} /></Routes></MemoryRouter></QueryClientProvider>)
  return userEvent.setup()
}
it('enters the restore route and returns with every query parameter intact', async () => {
  fixtures()
  server.use(http.get('/api/v1/backups/507/restore-info', () => HttpResponse.json({backup:{id:507,encrypted:false,created_at:row.created_at,local_available:true},target:{id:1,name:'Retained source',target_type:'path'},original_supported:true,volume_discovery:'available',available_volumes:[],remote_copies:[]})))
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}})
  const user=mount('/history/archives/507?q=keep&custom=value&offset=500',client)
  await user.click(await screen.findByRole('link',{name:'Restore archive'}))
  await screen.findByRole('heading',{name:'Source and availability'})
  expect(screen.getByLabelText('Location')).toHaveTextContent('/history/archives/507/restore?q=keep&custom=value&offset=500')
  await user.click(screen.getByRole('button',{name:'Close restore'}))
  expect(screen.getByLabelText('Location')).toHaveTextContent('/history/archives/507?q=keep&custom=value&offset=500')
  await waitFor(()=>expect(screen.getByRole('link',{name:'Restore archive'})).toHaveFocus())
  const refresh=screen.getByRole('button',{name:'Refresh Archive #507'})
  server.use(http.get('/api/v1/backups/history/507',()=>HttpResponse.json({...row,file_size:8})))
  await user.click(refresh)
  await waitFor(()=>expect(client.isFetching()).toBe(0))
  expect(refresh).toHaveFocus()
})
function fixtures() {
  server.use(http.get('/api/v1/backups/history', () => HttpResponse.json(page)), http.get('/api/v1/backups/history/507', () => HttpResponse.json(row)), http.get('/api/v1/backups/507/logs', () => HttpResponse.json([])))
}
it.each([false, true])('reaches plan-wide copy preview from a durable History route for removed=%s without selecting the visible page', async removed => {
  fixtures()
  const previews: unknown[] = [], executions: unknown[] = []
  server.use(
    http.get('/api/v1/backups/history/507', () => HttpResponse.json({ ...row, plan_removed: removed })),
    http.get('/api/v1/targets/1', () => HttpResponse.json({}, { status: removed ? 404 : 200 })),
    http.post('/api/v1/backups/removal/preview', async ({ request }) => { previews.push(await request.json()); return HttpResponse.json({ token: 'server-frozen-plan', scope: 'all', archive_count: 513, copy_count: 513, items: [], retained: [] }) }),
    http.post('/api/v1/backups/removal/execute', async ({ request }) => { executions.push(await request.json()); return HttpResponse.json({ status: 'completed', succeeded: 513, failed: 0, skipped: 0, items: [] }) }),
  )
  const user = mount('/history/runs/507?target_id=1&q=narrow&offset=500')
  await screen.findByRole('heading', { name: 'Run #507', exact: true })
  expect(screen.getByLabelText('Location')).toHaveTextContent('/history/runs/507')
  const action = await screen.findByRole('button', { name: 'Review archive copy deletion for this plan', exact: true })
  await user.click(action)
  const dialog = screen.getByRole('dialog', { name: 'Delete archive copies' })
  await user.click(within(dialog).getByRole('button', { name: 'Review exact scope' }))
  expect(await within(dialog).findByText('513 archives · 513 selected copies · all scope')).toBeVisible()
  expect(previews).toEqual([{ scope: 'all', target_id: 1 }])
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
  expect(executions).toEqual([])
  await user.click(action)
  await user.click(screen.getByRole('button', { name: 'Review exact scope' }))
  await user.click(await screen.findByRole('button', { name: 'Permanently delete confirmed copies' }))
  expect(await screen.findByRole('heading', { name: 'Selected copies removed' })).toBeVisible()
  expect(executions).toEqual([{ token: 'server-frozen-plan' }])
})

it('shows authoritative total, pages past the old sample and preserves context through details', async () => {
  fixtures()
  const requests: URL[] = []
  server.use(http.get('/api/v1/backups/history', ({ request }) => { const url = new URL(request.url); requests.push(url); return HttpResponse.json({ ...page, offset: Number(url.searchParams.get('offset') || 0) }) }))
  const user = mount()
  await screen.findByText('507 matching runs')
  expect(screen.getByRole('table', { name: 'Runs' })).toBeVisible()
  await user.click(screen.getByRole('button', { name: 'Next page' }))
  await waitFor(() => expect(requests.at(-1)?.searchParams.get('offset')).toBe('50'))
  expect(requests.at(-1)?.searchParams.get('snapshot')).toBe('507')
  await user.click(screen.getByRole('link', { name: 'Run #507' }))
  await screen.findByRole('heading', { name: 'Run #507' })
  expect(screen.getByLabelText('Location')).toHaveTextContent('custom=keep')
  await user.click(screen.getByRole('link', { name: 'Back to runs' }))
  await screen.findByText('507 matching runs')
  expect(screen.getByLabelText('Location')).toHaveTextContent('offset=50')
  await user.click(screen.getByRole('button', { name: 'Reset filters' }))
  expect(screen.getByLabelText('Location')).toHaveTextContent('custom=keep')
  expect(screen.getByRole('textbox', { name: 'Search history' })).toHaveValue('')
})
it('preserves saved legacy plan filters as authoritative target_id', async () => {
  fixtures()
  let target = ''
  server.use(http.get('/api/v1/backups/history', ({ request }) => { target = new URL(request.url).searchParams.get('target_id') ?? ''; return HttpResponse.json(page) }))
  mount('/history?plan=7&q=Retained&custom=keep')
  await screen.findByText('507 matching runs')
  expect(target).toBe('7')
  expect(screen.getByLabelText('Location')).toHaveTextContent('custom=keep')
})
it('never presents a failed run without archive as a restore point', async () => {
  fixtures()
  server.use(http.get('/api/v1/backups/history/12', () => HttpResponse.json({ ...row, id: 12, status: 'failed', archive_state: 'never_created', local_state: 'not_recorded', local_available: false, can_restore: false })))
  mount('/history/runs/12?q=keep')
  await screen.findByRole('heading', { name: 'Run #12' })
  expect(await screen.findByText('Archive never created')).toBeVisible()
  expect(screen.queryByRole('button', { name: 'Restore archive' })).not.toBeInTheDocument()
})
it.each([403, 500, 404])('keeps detail context and offers retry after %s', async status => {
  server.use(http.get('/api/v1/backups/history/99', () => HttpResponse.json({}, { status })))
  mount('/history/archives/99?q=keep&custom=value')
  await screen.findByRole('button', { name: 'Retry Archive #99' })
  const back = new URL(screen.getByRole('link', { name: 'Back to archives' }).getAttribute('href')!, 'http://test')
  expect(back.pathname).toBe('/history')
  expect(Object.fromEntries(back.searchParams)).toEqual({ view: 'archives', q: 'keep', custom: 'value' })
  if (status === 404) expect(screen.getByRole('heading', { name: 'Archive not found' })).toBeVisible()
})
it('retains last confirmed total as explicitly stale, never empty on failed refresh', async () => {
  fixtures()
  const user = mount()
  await screen.findByText('507 matching runs')
  server.use(http.get('/api/v1/backups/history', () => HttpResponse.json({}, { status: 500 })))
  await user.click(screen.getByRole('button', { name: 'Refresh History' }))
  await screen.findByText('History: stale data')
  expect(screen.getByText('507 matching runs')).toBeVisible()
  expect(screen.queryByText('No matching runs')).not.toBeInTheDocument()
})
it('uses archive-specific links and textual copy states', async () => {
  fixtures()
  server.use(http.get('/api/v1/backups/history', () => HttpResponse.json({ ...page, items: [{ ...row, local_state: 'missing', local_available: false, remote_synced: true, remote_sync_details: [{ storage_id: 3, storage_name: 'Offsite', status: 'completed', availability: 'unknown' }, { storage_id: 4, storage_name: 'Cold', status: 'failed', availability: 'unknown' }] }] })))
  mount('/history?view=archives')
  const table = await screen.findByRole('table', { name: 'Archives' })
  expect(within(table).getByRole('link', { name: 'Archive #507' })).toHaveAttribute('href', expect.stringContaining('/history/archives/507'))
  expect(within(table).getByText(/Local file missing/)).toBeVisible()
  expect(within(table).getByText(/Cold: failed sync/)).toBeVisible()
})
