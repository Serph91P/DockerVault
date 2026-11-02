import { StrictMode, useState } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import ConfirmDialog from '../ConfirmDialog'

function Confirmation({ pending = false }: { pending?: boolean }) {
  const [open, setOpen] = useState(false)
  return <><button onClick={() => setOpen(true)}>Remove plan</button><a href="#background">Background</a><ConfirmDialog isOpen={open} onClose={() => setOpen(false)} onConfirm={vi.fn()} title="Remove plan?" message="Review the scope." isLoading={pending} /></>
}

describe('shared confirmation contract', () => {
  it('names one StrictMode portal, contains keyboard focus, escapes and returns focus', async () => {
    const user = userEvent.setup()
    render(<StrictMode><Confirmation /></StrictMode>)
    const trigger = screen.getByRole('button', { name: 'Remove plan' })
    await user.click(trigger)
    const dialog = screen.getByRole('dialog', { name: 'Remove plan?' })
    expect(dialog).toHaveAccessibleDescription('Review the scope.')
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
    for (let i = 0; i < 8; i++) {
      await user.tab({ shift: i % 2 === 0 })
      await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
    }
    await user.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(trigger).toHaveFocus()
  })

  it('cannot dismiss pending confirmation with Escape or an outside click', async () => {
    const user = userEvent.setup()
    render(<Confirmation pending />)
    await user.click(screen.getByRole('button', { name: 'Remove plan' }))
    expect(screen.getByRole('button', { name: 'Processing...' })).toBeDisabled()
    await user.keyboard('{Escape}')
    expect(screen.getByRole('dialog', { name: 'Remove plan?' })).toBeInTheDocument()
  })
})
