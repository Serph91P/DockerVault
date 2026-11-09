import { render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import Layout from '../../components/Layout'
import { it, expect } from 'vitest'
import Settings from '../Settings'

const labels: Record<string, string> = { connection: 'Docker connection settings', encryption: 'Encryption settings', 'ssh-keys': 'SSH key security', system: 'System settings', 'storage-safeguards': 'Storage safeguards', 'system-recovery': 'System recovery' }
it.each(Object.keys(labels))('shell preserves Settings %s fragment focus instead of stealing it', async id => {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter initialEntries={[`/settings#${id}`]}><Routes><Route element={<Layout />}><Route path="/settings" element={<Settings />} /></Route></Routes></MemoryRouter></QueryClientProvider>)
  const section = await screen.findByRole('region', { name: labels[id] })
  expect(section).toHaveFocus()
})

it.each(Object.keys(labels))('opens and focuses the real Settings %s context', async id => {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter initialEntries={[`/settings#${id}`]}><Settings /></MemoryRouter></QueryClientProvider>)
  const section = await screen.findByRole('region', { name: labels[id] })
  expect(section).toHaveAttribute('id', id)
  expect(section).toHaveAttribute('tabindex', '-1')
  expect(section).toHaveFocus()
})
