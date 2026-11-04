import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { describe, expect, it } from 'vitest'
import Layout from '../Layout'

function Shell({ path = '/' }: { path?: string }) {
  return <MemoryRouter initialEntries={[path]}><Routes><Route element={<Layout />}><Route path="*" element={<button>Page action</button>} /></Route></Routes></MemoryRouter>
}
describe('task-oriented shell', () => {
  it('provides named navigation, skip link and nested current section', () => {
    render(<Shell path="/protection/schedules" />)
    expect(screen.getByRole('link', { name: 'Skip to main content' })).toHaveAttribute('href', '#main-content')
    expect(screen.getByRole('navigation', { name: 'Primary navigation' })).toBeInTheDocument()
    expect(within(screen.getByRole('navigation', { name: 'Primary navigation' })).getByRole('link', { name: 'Protection' })).toHaveAttribute('aria-current', 'page')
    expect(screen.getByRole('link', { name: 'History & Restore' })).toHaveAttribute('href', '/history')
    expect(screen.getByRole('main')).toHaveAttribute('id', 'main-content')
  })
  it('opens a named mobile navigation sheet and returns focus', async () => {
    const user = userEvent.setup()
    render(<Shell />)
    const trigger = screen.getByRole('button', { name: 'Open navigation' })
    await user.click(trigger)
    expect(screen.getByRole('dialog', { name: 'Navigation' })).toBeInTheDocument()
    await user.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(trigger).toHaveFocus()
  })
})
