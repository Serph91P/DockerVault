import { render, screen, within, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { expect, it } from 'vitest'
import Storage from '../Storage'
import { server } from '../../test/mocks/server'

function RouteProbe() {
  const location = useLocation(), navigate = useNavigate()
  return <><output aria-label="Route">{location.pathname}{location.search}</output><button onClick={() => navigate(-1)}>Browser back</button></>
}
function mount(route = '/storage') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}><Storage /><RouteProbe /></MemoryRouter></QueryClientProvider>)
  return client
}
const row = { id: 7, name: 'Configured local', storage_type: 'local', enabled: true, base_path: '/synthetic', created_at: '2026-01-01', updated_at: '2026-01-01' }
function fixtures() {
  server.use(http.get('/api/v1/storage', () => HttpResponse.json([row])), http.get('/api/v1/storage/ssh-keys', () => HttpResponse.json([])), http.get('/api/v1/operations', () => HttpResponse.json([])), http.get('/api/v1/storage/local/files', ({ request }) => HttpResponse.json({ path: new URL(request.url).searchParams.get('path') ?? '', files: [{ name: 'folder', is_dir: true, size: null, modified: 0 }, { name: 'a.txt', is_dir: false, size: 2, modified: 0 }, { name: 'z.txt', is_dir: false, size: 0, modified: 0 }] })))
}
it('routes a persistent location, folder, filters, sort and Back without modal or hidden selection', async () => {
  fixtures(); mount('/storage?storage=local'); const user = userEvent.setup()
  expect(await screen.findByRole('table', { name: /Files in/ })).toBeVisible()
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Select All', exact: true }))
  expect(screen.getByText('3 selected')).toBeVisible()
  await user.selectOptions(screen.getByLabelText('Sort entries'), 'size')
  expect(screen.getByText('3 selected')).toBeVisible()
  await user.type(screen.getByLabelText('Filter loaded directory'), 'a.txt')
  expect(screen.getByText('0 selected')).toBeVisible()
  expect(screen.getByRole('status', { name: 'Selection notice' })).toHaveTextContent(/cleared/i)
  expect(screen.getByLabelText('Route')).toHaveTextContent('q=a.txt')
  await user.clear(screen.getByLabelText('Filter loaded directory'))
  await user.click(screen.getByTitle('Select files'))
  await user.click(screen.getByText('folder', { exact: true }))
  await waitFor(() => expect(screen.getByLabelText('Route')).toHaveTextContent('path=folder'))
  await user.click(screen.getByRole('button', { name: 'Browser back' }))
  await waitFor(() => expect(screen.getByLabelText('Route')).not.toHaveTextContent('path=folder'))
  await user.click(screen.getByRole('link', { name: 'Usage', exact: true }))
  expect(await screen.findByRole('heading', {name:'Filesystem capacity of the configured local backup directory'})).toBeVisible()
})
it('records the admitted operation ID in the current location URL without reconstructing an intent', async () => {
  fixtures(); const id='a0139b9a-f3a1-4d30-906f-356da49ad4ad'
  server.use(http.post('/api/v1/operations', () => HttpResponse.json({operation_id:id},{status:202})))
  mount('/storage?storage=local&path=nested'); const user=userEvent.setup()
  await user.click(await screen.findByRole('button',{name:'Select All',exact:true}))
  await user.click(screen.getByRole('button',{name:'Delete',exact:true}))
  await user.click(within(screen.getByRole('dialog',{name:'Delete Files'})).getByRole('button',{name:'Delete',exact:true}))
  await waitFor(() => expect(screen.getByLabelText('Route')).toHaveTextContent(`operation=${id}`))
  expect(screen.getByLabelText('Route')).toHaveTextContent('storage=local&path=nested')
  expect(screen.getByText('0 selected')).toBeVisible()
})
it('never renders endpoint credentials in location summaries or review', async () => {
  fixtures(); server.use(http.get('/api/v1/storage', () => HttpResponse.json([{...row,storage_type:'webdav',webdav_url:'https://private-user:private-password@example.invalid/path?token=private-query'}])))
  mount(); const user=userEvent.setup()
  await user.click(await screen.findByTitle('Edit'))
  await user.click(screen.getByRole('button',{name:'Next',exact:true}))
  expect(screen.queryByText(/private-user|private-password|private-query/)).not.toBeInTheDocument()
})
it('tests an unsaved draft without create and invalidates the observation on field change', async () => {
  fixtures(); const posts: unknown[] = [], drafts: unknown[] = []
  server.use(http.post('/api/v1/storage', async ({request}) => { posts.push(await request.json()); return HttpResponse.json(row) }), http.post('/api/v1/storage/test-config', async ({request}) => { drafts.push(await request.json()); return HttpResponse.json({success:true,message:'Connection test succeeded'}) }))
  mount(); const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Add Storage', exact: true }))
  const dialog = screen.getByRole('dialog', { name: 'Add New Storage' })
  await user.type(within(dialog).getByLabelText('Name'), 'Synthetic draft')
  await user.click(within(dialog).getByRole('button', { name: 'Next', exact: true }))
  await user.click(within(dialog).getByRole('button', { name: 'Test draft', exact: true }))
  expect(await within(dialog).findByText(/Draft test succeeded/)).toBeVisible()
  expect(drafts).toHaveLength(1); expect(posts).toHaveLength(0)
  await user.type(within(dialog).getByLabelText('Base path'), '/changed')
  expect(within(dialog).getByText(/Draft changed.*stale/)).toBeVisible()
  await user.click(within(dialog).getByRole('button', { name: 'Next', exact: true }))
  expect(posts).toHaveLength(0)
  await user.click(within(dialog).getByRole('button', { name: 'Back', exact: true }))
  expect(posts).toHaveLength(0)
})
it('keeps SFTP identity immutable and preserves nonsecret empty values', async () => {
  fixtures(); server.use(http.get('/api/v1/storage', () => HttpResponse.json([{...row, storage_type:'sftp', base_path:'', s3_region:'', host:'example.invalid', username:'synthetic'}])))
  mount(); const user=userEvent.setup()
  await user.click(await screen.findByTitle('Edit'))
  const dialog=screen.getByRole('dialog', {name:'Edit Storage'})
  expect(within(dialog).getByLabelText('Host')).toHaveValue('example.invalid')
  expect(within(dialog).getByLabelText('Base path')).toHaveValue('')
  await user.click(within(dialog).getByRole('button',{name:'Back',exact:true}))
  expect(within(dialog).queryByRole('button',{name:/Local \/ NFS/})).not.toBeInTheDocument()
})
