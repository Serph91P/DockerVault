import { expect, it } from 'vitest'
import api from './index'
import { http, HttpResponse } from 'msw'
import { server } from '../test/mocks/server'
it('removes credentials and query tokens before storage metadata reaches consumers', async () => {
  server.use(http.get('/api/v1/storage', () => HttpResponse.json([{id:1,name:'Synthetic',storage_type:'webdav',enabled:true,base_path:'/data',webdav_url:'https://synthetic-user:synthetic-secret@example.invalid/path?token=synthetic-token#private'}])))
  const response=await api.get('/storage')
  expect(response.data[0].webdav_url).toBe('https://example.invalid/path')
  expect(JSON.stringify(response.data)).not.toMatch(/synthetic-user|synthetic-secret|synthetic-token/)
})
