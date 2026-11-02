import { StrictMode, useState } from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { ChoiceSelect, SearchSelect } from '../ui/select'
import { Modal } from '../ui/dialog'
import { Menu, MenuContent, MenuItem, MenuTrigger } from '../ui/menu'

const options = [{ value: 'name', label: 'Name' }, { value: 'type', label: 'Type' }, { value: 'created', label: 'Newest first' }]
function Controls() {
 const [value, setValue] = useState('name')
 return <><ChoiceSelect label="Sort plans" value={value} onValueChange={setValue} options={options} /><SearchSelect label="Find plan" value={value} onValueChange={setValue} options={options} /><output data-testid="selection">{value}</output></>
}
describe('portalled selection families', () => {
 it('selects with keyboard and escapes only the top popup inside a modal', async () => {
  const user=userEvent.setup()
  render(<StrictMode><Modal title="Controls" onClose={() => {}}><Controls /></Modal></StrictMode>)
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Sort plans' })).toHaveFocus())
  await user.click(screen.getByRole('combobox', {name:'Sort plans'}))
  await screen.findByRole('listbox')
  await user.keyboard('{End}{Enter}')
  await waitFor(()=>expect(screen.getByTestId('selection')).toHaveTextContent('created'))
  await user.click(screen.getByRole('combobox',{name:'Find plan'}))
  await user.clear(screen.getByRole('combobox',{name:'Find plan'}))
  await user.type(screen.getByRole('combobox',{name:'Find plan'}),'Type')
  await user.keyboard('{ArrowDown}{Enter}')
  await waitFor(()=>expect(screen.getByTestId('selection')).toHaveTextContent('type'))
  expect(screen.getAllByRole('dialog')).toHaveLength(1)
 })
 it('lets Escape leave a closed non-clearable combobox without erasing its selection', async () => {
  function Fixture() {
   const [open, setOpen] = useState(true)
   const [value, setValue] = useState('name')
   return <><output data-testid="value">{value}</output>{open && <Modal title="Editor" onClose={() => setOpen(false)}><SearchSelect label="Plan" value={value} onValueChange={setValue} options={options} /></Modal>}</>
  }
  const user = userEvent.setup()
  render(<Fixture />)
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Plan' })).toHaveFocus())
  await user.keyboard('{Escape}')
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  expect(screen.getByTestId('value')).toHaveTextContent('name')
 })
 it('moves through menu actions with arrows and returns to its trigger on Escape', async () => {
  const user=userEvent.setup()
  render(<Menu><MenuTrigger>Actions</MenuTrigger><MenuContent><MenuItem>Browse</MenuItem><MenuItem>Restore</MenuItem></MenuContent></Menu>)
  screen.getByRole('button',{name:'Actions'}).focus()
  await user.keyboard('{ArrowDown}')
  expect(screen.getByRole('menu')).toBeInTheDocument()
  await user.keyboard('{ArrowDown}')
  expect(screen.getByRole('menuitem',{name:'Restore'})).toHaveFocus()
  await user.keyboard('{Escape}')
  await waitFor(()=>expect(screen.getByRole('button',{name:'Actions'})).toHaveFocus())
 })
})
