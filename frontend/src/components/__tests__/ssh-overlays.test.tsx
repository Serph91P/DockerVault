import { StrictMode } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import SSHKeyManager from '../SSHKeyManager'

beforeEach(()=>server.use(http.get('/api/v1/storage/ssh-keys',()=>HttpResponse.json([{name:'synthetic',public_key:'synthetic-public',private_path:'/synthetic/key'}]))))
function setup(){const client=new QueryClient({defaultOptions:{queries:{retry:false}}});render(<StrictMode><QueryClientProvider client={client}><SSHKeyManager/></QueryClientProvider></StrictMode>)}
describe('SSH overlay callers',()=>{
 it('uses the shared confirmation instead of a browser-native blocking prompt',async()=>{
  const user=userEvent.setup();setup();await user.click(await screen.findByRole('button',{name:'Delete key'}))
  expect(screen.getByRole('dialog',{name:'Delete SSH key'})).toHaveAccessibleDescription('Delete SSH key "synthetic"? This cannot be undone.')
  await user.keyboard('{Escape}');await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  expect(screen.getByRole('button',{name:'Delete key'})).toHaveFocus()
 })
 it.each([['Generate','Generate SSH key'],['Install on remote','Install key on remote']])('names and escapes %s',async(button,title)=>{
  const user=userEvent.setup();setup();await user.click(await screen.findByRole('button',{name:button,exact:true}));expect(screen.getByRole('dialog',{name:title})).toBeInTheDocument();await user.keyboard('{Escape}');await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument());expect(screen.getByRole('button',{name:button,exact:true})).toHaveFocus()
 })
})
