import { render, screen, within, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { describe, it, expect } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Storage from '../../pages/Storage'

describe('storage wizard focused activation', () => {
 it('reaches Review without submitting from Next, then saves only from explicit confirmation', async () => {
  const saved: unknown[] = []
  server.use(http.get('/api/v1/storage',()=>HttpResponse.json([])),http.get('/api/v1/storage/ssh-keys',()=>HttpResponse.json([])),http.post('/api/v1/storage',async({request})=>{saved.push(await request.json());return HttpResponse.json({id:8})}))
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});const user=userEvent.setup()
  render(<QueryClientProvider client={client}><Storage/></QueryClientProvider>)
  await user.click(screen.getByRole('button',{name:'Add Storage',exact:true}))
  const dialog=screen.getByRole('dialog',{name:'Add New Storage'})
  await user.type(within(dialog).getByPlaceholderText('My Backup Storage'),'Synthetic local')
  await user.click(within(dialog).getByRole('button',{name:/Local/}))
  await user.click(within(dialog).getByRole('button',{name:'Next',exact:true}))
  await user.click(within(dialog).getByRole('button',{name:'Next',exact:true}))
  expect(saved).toHaveLength(0)
  expect(within(dialog).getByText('Path',{exact:true})).toBeInTheDocument()
  await user.click(within(dialog).getByRole('button',{name:'Add Storage',exact:true}))
  await waitFor(()=>expect(saved).toHaveLength(1))
 })
})
