import { cleanup, render, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import Dashboard from '../Dashboard'
import Backups from '../Backups'

const clients: QueryClient[] = []
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()) })

it.each([[false, true], [true, false]])('active/historical target navigation %s -> %s has independent HTTP identity', async (first, second) => {
  const reads: boolean[] = []
  server.use(http.get('/api/v1/targets', ({ request }) => { reads.push(new URL(request.url).searchParams.get('include_removed') === 'true'); return HttpResponse.json([]) }))
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: 10000, retry: false } } })
  clients.push(client)
  const page = (historical: boolean) => <QueryClientProvider client={client}><MemoryRouter>{historical ? <Backups workspace="history" /> : <Dashboard />}</MemoryRouter></QueryClientProvider>
  const view = render(page(first))
  await waitFor(() => expect(reads).toEqual([first]))
  await waitFor(() => expect(client.isFetching()).toBe(0))
  view.rerender(page(second))
  await waitFor(() => expect(reads).toEqual([first, second]))
})

// Catches callers sharing a fresh cache entry despite sending different limits.
it.each([[10, 500], [500, 10]])('navigation %i -> %i reads the requested archive window', async (first, second) => {
  const reads: number[] = []
  server.use(http.get('/api/v1/backups', ({ request }) => {
    const limit = Number(new URL(request.url).searchParams.get('limit'))
    reads.push(limit)
    return HttpResponse.json([])
  }))
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: 10000, retry: false } } })
  clients.push(client)
  const page = (limit: number) => <QueryClientProvider client={client}><MemoryRouter>{limit === 10 ? <Dashboard /> : <Backups />}</MemoryRouter></QueryClientProvider>
  const view = render(page(first))
  await waitFor(() => expect(reads).toEqual([first]))
  await waitFor(() => expect(client.isFetching()).toBe(0))
  view.rerender(page(second))
  await waitFor(() => expect(reads).toEqual([first, second]))
})
