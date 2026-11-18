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
  expect(screen.getByTitle('Select files')).toHaveAttribute('aria-pressed','true')
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
it('distinguishes symbolic links from ordinary files in the visible listing', async () => {
  fixtures(); server.use(http.get('/api/v1/storage/local/files',()=>HttpResponse.json({path:'',files:[{name:'linked',is_dir:false,is_symlink:true,size:null,modified:0}]})))
  mount('/storage?storage=local&kind=link')
  expect(await screen.findByRole('row',{name:/linked.*Symbolic link/})).toBeVisible()
})
it.each(['kind=unknown','sort=unknown','direction=unknown'])('does not represent invalid view options as an empty directory: %s', async query => {
  fixtures(); mount(`/storage?storage=local&${query}`)
  expect(await screen.findByRole('alert')).toHaveTextContent('Invalid file view parameters')
  expect(screen.queryByRole('table')).not.toBeInTheDocument()
})
it('invalidates saved-test presentation after an authoritative configuration revision changes', async () => {
  fixtures(); server.use(http.post('/api/v1/storage/7/test',()=>HttpResponse.json({success:true,message:'Observed'})))
  const client=mount('/storage?storage=7&tab=connection'), user=userEvent.setup()
  await user.click(await screen.findByRole('button',{name:'Test saved connection'}))
  await screen.findByText(/Connection confirmed.*observed/)
  client.setQueryData(['storages','list'],[{...row,updated_at:'2026-02-01'}])
  await screen.findByText(/Test observation stale; configuration changed/)
  expect(screen.queryByText(/Connection confirmed.*observed/)).not.toBeInTheDocument()
})
it('revokes the private editor and test observation when its authoritative root changes', async () => {
  fixtures(); const client=mount(); const user=userEvent.setup()
  await user.click(await screen.findByTitle('Edit'))
  expect(screen.getByRole('dialog',{name:'Edit Storage'})).toBeVisible()
  client.setQueryData(['storages','list'],[{...row,base_path:'/changed',updated_at:'2026-02-01'}])
  await waitFor(()=>expect(screen.queryByRole('dialog',{name:'Edit Storage'})).not.toBeInTheDocument())
})
it('allows a custom SSH key reference on create without an existing managed key', async () => {
  fixtures(); mount(); const user=userEvent.setup()
  await user.click(screen.getByRole('button',{name:'Add Storage',exact:true}))
  const dialog=screen.getByRole('dialog',{name:'Add New Storage'})
  await user.type(within(dialog).getByLabelText('Name'),'SSH custom')
  await user.click(within(dialog).getByRole('button',{name:/SSH \/ SFTP/}))
  await user.click(within(dialog).getByRole('button',{name:'Next',exact:true}))
  await user.type(within(dialog).getByLabelText('SSH key path'),'/app/data/ssh_keys/synthetic')
  expect(within(dialog).getByLabelText('SSH key path')).toHaveValue('/app/data/ssh_keys/synthetic')
})
it('filters the loaded location list without offering provider tiles as daily navigation', async () => {
  fixtures(); mount(); const user=userEvent.setup()
  expect(await screen.findByText('Configured local',{exact:true})).toBeVisible()
  expect(screen.queryByRole('button',{name:/Local \/ NFS/})).not.toBeInTheDocument()
  await user.type(screen.getByLabelText('Filter locations'), 'not-present')
  expect(screen.queryByText('Configured local',{exact:true})).not.toBeInTheDocument()
  expect(screen.getByText('No locations match this filter.')).toBeVisible()
})
it.each(['ssh','sftp','ftp'])('redacts typed %s endpoint userinfo and query text in Review', async provider => {
  fixtures(); server.use(http.get('/api/v1/storage',()=>HttpResponse.json([{...row,storage_type:provider,host:'example.invalid',username:'synthetic'}])))
  mount(); const user=userEvent.setup(); await user.click(await screen.findByTitle('Edit'))
  const dialog=screen.getByRole('dialog',{name:'Edit Storage'})
  await user.clear(within(dialog).getByLabelText('Host'))
  await user.type(within(dialog).getByLabelText('Host'),'https://private-user:private-password@example.invalid/path?token=private-query')
  await user.click(within(dialog).getByRole('button',{name:'Next',exact:true}))
  expect(within(dialog).getByText(/example.invalid/)).toBeVisible()
  expect(within(dialog).queryByText(/private-user|private-password|private-query/)).not.toBeInTheDocument()
})
it('never renders endpoint credentials in location summaries or review', async () => {
  fixtures(); server.use(http.get('/api/v1/storage', () => HttpResponse.json([{...row,storage_type:'webdav',webdav_url:'https://private-user:private-password@example.invalid/path?token=private-query'}])))
  mount(); const user=userEvent.setup()
  await user.click(await screen.findByTitle('Edit'))
  await user.click(screen.getByRole('button',{name:'Next',exact:true}))
  expect(screen.queryByText(/private-user|private-password|private-query/)).not.toBeInTheDocument()
})
it.each(['local','ssh','sftp','s3','webdav','ftp','rclone'])('shares %s edit fields with draft testing and saving while preserving identity and blank secrets', async provider => {
  fixtures()
  const savedRow={...row,storage_type:provider,host:'example.invalid',port:2222,username:'synthetic',base_path:provider==='s3' ? '' : 'nested',ssh_key_path:'/app/data/ssh_keys/synthetic',s3_bucket:'bucket',s3_region:'',s3_endpoint_url:'https://example.invalid',webdav_url:'https://example.invalid/dav',rclone_remote:'synthetic'}
  const tests: Record<string,unknown>[]=[], puts: Record<string,unknown>[]=[]
  server.use(http.get('/api/v1/storage',()=>HttpResponse.json([savedRow])),http.post('/api/v1/storage/test-config',async({request})=>{tests.push(await request.json() as Record<string,unknown>);return HttpResponse.json({success:true,message:'Observed'})}),http.put('/api/v1/storage/7',async({request})=>{puts.push(await request.json() as Record<string,unknown>);return HttpResponse.json(savedRow)}))
  mount(); const user=userEvent.setup()
  await user.click(await screen.findByTitle('Edit'))
  const dialog=screen.getByRole('dialog',{name:'Edit Storage'})
  expect(within(dialog).getByLabelText('Base path')).toHaveValue(savedRow.base_path)
  if (['ssh','sftp','ftp'].includes(provider)) { expect(within(dialog).getByLabelText('Host')).toHaveValue(savedRow.host); expect(within(dialog).getByLabelText('Port')).toHaveValue(2222) }
  if (provider==='s3') { expect(within(dialog).getByLabelText('S3 region')).toHaveValue(''); expect(within(dialog).getByLabelText('S3 bucket')).toHaveValue('bucket') }
  if (provider==='webdav') expect(within(dialog).getByLabelText('WebDAV URL')).toHaveValue(savedRow.webdav_url)
  if (provider==='rclone') expect(within(dialog).getByLabelText('Rclone remote')).toHaveValue('synthetic')
  await user.click(within(dialog).getByRole('button',{name:'Test draft',exact:true}))
  await within(dialog).findByText(/Draft test succeeded/)
  expect(tests[0]).toMatchObject({mode:'edit',storage_id:7,config:{base_path:savedRow.base_path,s3_region:'',password:'',s3_access_key:'',s3_secret_key:''}})
  expect(tests[0].config).not.toHaveProperty('storage_type')
  await user.click(within(dialog).getByRole('button',{name:'Next',exact:true}))
  await user.click(within(dialog).getByRole('checkbox',{name:'Storage enabled'}))
  await user.click(within(dialog).getByRole('button',{name:'Save',exact:true}))
  await waitFor(()=>expect(puts).toHaveLength(1))
  expect(puts[0]).toMatchObject({base_path:savedRow.base_path,s3_region:'',enabled:false})
  expect(puts[0]).not.toHaveProperty('storage_type')
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
