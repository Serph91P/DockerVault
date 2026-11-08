import { render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { it, expect } from 'vitest'
import Settings from '../Settings'

it.each(['connection', 'encryption'])('opens and focuses the real Settings %s context', async id => {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter initialEntries={[`/settings#${id}`]}><Settings /></MemoryRouter></QueryClientProvider>)
  const section = await screen.findByRole('region', { name: id === 'connection' ? 'Docker connection settings' : 'Encryption settings' })
  expect(section).toHaveAttribute('id', id)
  expect(section).toHaveAttribute('tabindex', '-1')
  expect(section).toHaveFocus()
})
