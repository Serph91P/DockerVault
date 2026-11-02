import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi } from 'vitest'
import { EntityTable, Pagination, SelectionToolbar, StatusLabel, PageHeader, FilterBar, QueryState } from '../ui/workspace'

describe('shared workspace compositions', () => {
 it('exposes a captioned table in an explicitly scrollable named region', () => {
  render(<EntityTable label="Files"><thead><tr><th scope="col">Name</th></tr></thead><tbody><tr><td>Fixture</td></tr></tbody></EntityTable>)
  expect(screen.getByRole('table',{name:'Files'})).toBeInTheDocument()
  expect(screen.getByRole('region',{name:/Files/})).toHaveAttribute('tabindex','0')
 })
 it('disables pagination bounds and announces selection and state without color alone', async () => {
  const change=vi.fn(), clear=vi.fn()
  render(<><PageHeader title="Files" description="Browse known items" /><FilterBar><input aria-label="Search files" /></FilterBar><Pagination page={1} pages={3} onPageChange={change} /><SelectionToolbar count={2} onClear={clear}>Download selected</SelectionToolbar><StatusLabel tone="warning">Partial</StatusLabel><QueryState state="error" title="Files unavailable" description="Retry the request." /></>)
  expect(screen.getByRole('button',{name:'Previous page'})).toBeDisabled()
  await userEvent.click(screen.getByRole('button',{name:'Next page'}))
  expect(change).toHaveBeenCalledWith(2)
  await userEvent.click(screen.getByRole('button',{name:'Clear selection'}))
  expect(clear).toHaveBeenCalledOnce()
  expect(screen.getByRole('status')).toHaveTextContent('2 selected')
  expect(screen.getByRole('alert')).toHaveTextContent('Files unavailable')
  expect(screen.getByText('Partial')).toBeInTheDocument()
 })
})
