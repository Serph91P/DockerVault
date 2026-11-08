import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, Routes, Route, useNavigate } from 'react-router-dom'
import { it, expect } from 'vitest'
import { http, HttpResponse } from 'msw'
import Layout from '../../components/Layout'
import Dashboard from '../Dashboard'
import { server } from '../../test/mocks/server'

function Navigation() {
  const navigate = useNavigate()
  return <button onClick={() => navigate(-1)}>Browser back</button>
}
function mount(path = '/') {
  server.use(http.get('/api/v1/settings/system-info', () => HttpResponse.json({ disk_status: 'unavailable', disk_total: 0, disk_used: 0, disk_free: 0, backup_count: 0, target_count: 0, uptime_seconds: 1, app_version: 'fixture' })))
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}><MemoryRouter initialEntries={[path]}><Routes><Route element={<Layout />}><Route path="/" element={<><Dashboard /><Navigation /></>} /></Route></Routes></MemoryRouter></QueryClientProvider>)
}
it('keyboard capacity navigation focuses the real retry context and Back restores main focus', async () => {
  const user = userEvent.setup()
  mount()
  const action = await screen.findByRole('link', { name: 'Review capacity and retry' })
  action.focus()
  await user.keyboard('{Enter}')
  const capacity = screen.getByRole('region', { name: 'Local capacity' })
  expect(capacity).toHaveFocus()
  await user.tab()
  expect(screen.getByRole('button', { name: 'Refresh System information', exact: true })).toHaveFocus()
  await user.click(screen.getByRole('button', { name: 'Browser back' }))
  expect(screen.getByRole('main')).toHaveFocus()
})
it('direct or reloaded capacity fragment preserves target focus in the real shell', async () => {
  mount('/#overview-capacity')
  expect(await screen.findByRole('region', { name: 'Local capacity' })).toHaveFocus()
})
