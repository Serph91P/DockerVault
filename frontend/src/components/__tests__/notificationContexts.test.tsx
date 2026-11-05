import { cleanup, render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Backups from '../../pages/Backups'
import Storage from '../../pages/Storage'
import NotificationContext from '../NotificationContext'

afterEach(cleanup)
it.each(['1e3', '1.0', '+1'])('does not normalize a malformed route ID %s into another valid object', raw => {
  const client = new QueryClient()
  const view = render(<QueryClientProvider client={client}><MemoryRouter initialEntries={['/history?run=' + encodeURIComponent(raw)]}><NotificationContext domain="backups" /></MemoryRouter></QueryClientProvider>)
  expect(screen.getByRole('alert', { name: 'Run #' + raw + ' context' })).toHaveTextContent('Invalid notification target')
  view.unmount(); client.clear()
})
it.each([
  ['/protection?plan=919', '/targets/919', 'Plan #919'],
  ['/history?run=919', '/backups/919', 'Run #919'],
  ['/history?archive=919', '/backups/919', 'Archive #919'],
  ['/storage?operation=b658c36b-4872-4364-aafc-72b227142815&storage=local', '/operations/b658c36b-4872-4364-aafc-72b227142815', 'Operation b658c36b-4872-4364-aafc-72b227142815'],
])('revisits %s and explains a removed or denied object', async (route, endpoint, label) => {
  server.use(http.get(`/api/v1${endpoint}`, () => new HttpResponse(null, { status: 404 })))
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  const view = render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{route.startsWith('/storage') ? <Storage /> : <Backups workspace={route.startsWith('/history') ? 'history' : 'protection'} />}</MemoryRouter></QueryClientProvider>)
  expect(await screen.findByRole('alert', { name: `${label} context` })).toHaveTextContent(/removed|unavailable|access/i)
  view.unmount(); client.clear()
})
