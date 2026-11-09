import { render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { http, HttpResponse } from 'msw'
import { expect, it } from 'vitest'
import userEvent from '@testing-library/user-event'
import { server } from '../../test/mocks/server'
import StorageLocationContext from '../StorageLocationContext'
it('opens the canonical storage browser at the linked relative path with safe return context', async () => {
  server.use(http.get('/api/v1/storage/local/files', ({ request }) => { expect(new URL(request.url).searchParams.get('path')).toBe('archive/source'); return HttpResponse.json({ files: [], path: 'archive/source' }) }))
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><MemoryRouter initialEntries={['/storage?storage=local&path=archive%2Fsource&returnTo=%2Fhistory%2Farchives%2F7%3Fq%3Dkeep']}><StorageLocationContext storages={[]} /></MemoryRouter></QueryClientProvider>)
  expect(await screen.findByRole('region', {name:'Browse Local Backups'})).toBeVisible()
  await screen.findByText('This directory is empty')
  await userEvent.click(screen.getByRole('button', { name: 'Close storage browser' }))
  expect(screen.getByRole('link', { name: 'Return to archive context' })).toHaveAttribute('href','/history/archives/7?q=keep')
})
