import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import RestoreWizard from '../RestoreWizard'
import { server } from '../../test/mocks/server'
import { clearSession } from '../../store/session'

export const impact = { fingerprint: 'review-1', destination: { type: 'path', path: '/backups/custom', volume: null }, containers_to_stop: ['configured'], containers_to_restart: ['configured'], layout: 'Single root contents merge', overwrite: 'Colliding files overwritten; unrelated files remain.', key_required: true }
export const info = { backup: { id: 44, encrypted: true, created_at: '2026-01-01T00:00:00Z', local_available: true }, target: { id: 3, name: 'Synthetic', target_type: 'container' }, original_supported: false, volume_discovery: 'available', available_volumes: ['real-volume'], remote_copies: [] }
afterEach(cleanup)
function mount() {
 const client = new QueryClient()
 const close = vi.fn()
 render(<QueryClientProvider client={client}><RestoreWizard backupId={44} onClose={close} /></QueryClientProvider>)
 return { client, close, user: userEvent.setup() }
}
function handlers(post: () => Response | Promise<Response>) {
 server.use(http.get('/api/v1/backups/44/restore-info', () => HttpResponse.json(info)), http.post('/api/v1/backups/44/restore-review', () => HttpResponse.json(impact)), http.post('/api/v1/backups/44/restore', post))
}
async function review(user: ReturnType<typeof userEvent.setup>) {
 await user.click(await screen.findByRole('button', { name: 'Next' }))
 expect(screen.getByRole('radio', {name: 'Original location'})).toBeDisabled()
 await user.click(screen.getByLabelText('Custom path'))
 await user.type(screen.getByLabelText('Destination path'), '/backups/custom')
 await user.type(screen.getByLabelText('Private recovery key'), 'synthetic-private-marker')
 await user.click(screen.getByRole('button', { name: 'Review effects' }))
 await screen.findByRole('heading', { name: 'Review restore impact' })
}
it('only explicit confirmation submits once and retains the actual result, without caching keys', async () => {
 let calls = 0
 let payload: Record<string, unknown> = {}
 handlers(() => { calls++; return HttpResponse.json({ status: 'restored', ...impact }) })
 server.use(http.post('/api/v1/backups/44/restore', async ({request}) => { payload = await request.json() as Record<string, unknown>; calls++; return HttpResponse.json({status:'restored', ...impact}) }))
 const {user,client,close} = mount()
 await review(user)
 expect(calls).toBe(0)
 const confirm = screen.getByRole('button', {name:'Confirm restore'})
 confirm.focus(); await user.keyboard('{Enter}')
 expect(calls).toBe(0)
 await user.click(screen.getByLabelText(/I reviewed/))
 confirm.focus(); await user.keyboard('{Enter}')
 expect(calls).toBe(0)
 await user.dblClick(confirm)
 expect(await screen.findByRole('heading', {name:'Restore completed'})).toBeInTheDocument()
 expect(calls).toBe(1)
 expect(payload).toEqual({destination_type:'path',target_path:'/backups/custom',review_fingerprint:'review-1',private_key:'synthetic-private-marker'})
 expect(close).not.toHaveBeenCalled()
 expect(JSON.stringify([client.getQueryCache().getAll(), client.getMutationCache().getAll()])).not.toContain('synthetic-private-marker')
})
it('close before confirmation has no effect and discards the private view', async () => {
 const post = vi.fn(() => HttpResponse.json({status:'restored',...impact})); handlers(post)
 const {user,close}=mount(); await review(user)
 await user.click(screen.getByRole('button',{name:'Close restore'}))
 expect(close).toHaveBeenCalledOnce(); expect(post).not.toHaveBeenCalled()
})
it.each([409,500,0])('HTTP %s has no false success and clears private material', async status => {
 handlers(() => status ? HttpResponse.json({detail:'synthetic-private-marker'}, {status}) : HttpResponse.error())
 const {user}=mount(); await review(user); await user.click(screen.getByLabelText(/I reviewed/)); await user.click(screen.getByRole('button',{name:'Confirm restore'}))
 expect(await screen.findByRole('alert')).toHaveTextContent(status===409 ? /changed/i : /not confirmed|unknown/i)
 expect(screen.queryByRole('heading',{name:'Restore completed'})).not.toBeInTheDocument()
 expect(screen.queryByDisplayValue('synthetic-private-marker')).not.toBeInTheDocument()
})
it.each([{...info,target:null},{...info,remote_copies:[null]},{...info,available_volumes:[{}]}])('malformed source data is unavailable, never a crashed or confirmed view',async malformed=>{
 server.use(http.get('/api/v1/backups/44/restore-info',()=>HttpResponse.json(malformed)))
 mount();expect(await screen.findByRole('alert')).toHaveTextContent(/unavailable/)
 expect(screen.queryByRole('button',{name:'Next'})).not.toBeInTheDocument()
})
it('labels the discovered volume selector and sends a volume identity without a browser mountpath',async()=>{
 let selected:unknown
 handlers(()=>HttpResponse.json({status:'restored',...impact}))
 server.use(http.post('/api/v1/backups/44/restore-review',async({request})=>{selected=await request.json();return HttpResponse.json(impact)}))
 const {user}=mount();await user.click(await screen.findByRole('button',{name:'Next'}));await user.click(screen.getByRole('radio',{name:'Different volume'}));await user.selectOptions(screen.getByLabelText('Destination volume',{exact:true}),'real-volume');await user.type(screen.getByLabelText('Private recovery key'),'synthetic');await user.click(screen.getByRole('button',{name:'Review effects'}));await screen.findByRole('heading',{name:'Review restore impact'});expect(selected).toEqual({destination_type:'volume',target_volume:'real-volume'})
})
it('review failure clears the private key rather than retaining failed-view secrets',async()=>{
 handlers(()=>HttpResponse.json({status:'restored',...impact}))
 server.use(http.post('/api/v1/backups/44/restore-review',()=>HttpResponse.json({}, {status:503})))
 const {user}=mount();await user.click(await screen.findByRole('button',{name:'Next'}));await user.type(screen.getByLabelText('Destination path'),'/backups/custom');await user.type(screen.getByLabelText('Private recovery key'),'synthetic-private-marker');await user.click(screen.getByRole('button',{name:'Review effects'}));await screen.findByRole('alert');expect(screen.getByLabelText('Private recovery key')).toHaveValue('')
})
it('session revocation removes key and makes old callbacks ineffective', async () => {
 let release!: () => void
 handlers(async () => { await new Promise<void>(resolve=>{release=resolve}); return HttpResponse.json({status:'restored',...impact}) })
 const {user}=mount(); await review(user); await user.click(screen.getByLabelText(/I reviewed/)); await user.click(screen.getByRole('button',{name:'Confirm restore'}))
 await waitFor(()=>expect(release).toBeDefined()); clearSession(); release()
 await waitFor(()=>expect(screen.queryByDisplayValue('synthetic-private-marker')).not.toBeInTheDocument())
 expect(screen.queryByRole('heading',{name:'Restore completed'})).not.toBeInTheDocument()
})
