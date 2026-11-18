import { render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { expect, it } from 'vitest'
import Settings from '../Settings'
it('owns a single SSH key manager in Security and returns only to a safe workspace', async () => {
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><MemoryRouter initialEntries={['/settings?storageReturn=%2Fstorage%3Fstorage%3D7%26tab%3Dconnection%26path%3Dnested#ssh-keys']}><Settings /></MemoryRouter></QueryClientProvider>)
  expect(await screen.findByRole('region', {name:'SSH key security'})).toHaveFocus()
  expect(screen.getByRole('link', {name:'Return to storage connection'})).toHaveAttribute('href','/storage?storage=7&tab=connection&path=nested')
  expect(screen.getAllByRole('button',{name:'Generate',exact:true})).toHaveLength(1)
})
