import { useEffect, useState } from 'react'
import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { RetentionFields } from './PolicyFields'
import { DraftWriter, readDraft, safeIntent } from './planEditorDraft'
import { initialData, retentionDefaults, validateEditor, serializeCreate, type WizardData } from './planEditorModel'

const fields = [
  ['keepLast', 'Keep Last'], ['keepDaily', 'Daily'], ['keepWeekly', 'Weekly'],
  ['keepMonthly', 'Monthly'], ['keepYearly', 'Yearly'], ['maxAgeDays', 'Max age (days)'],
] as const
const data: WizardData = { ...initialData, targetType: 'path', hostPath: '/fixture', targetName: 'Preserved plan', newRetentionPolicy: { ...retentionDefaults, name: 'Preserved policy' } }
afterEach(() => { cleanup(); localStorage.clear(); vi.useRealTimers() })

function DraftForm({ initial, writer }: { initial: WizardData; writer: DraftWriter }) {
  const [draft, setDraft] = useState(initial)
  useEffect(() => { writer.queue(draft, 1); return () => writer.cancel() }, [draft, writer])
  return <RetentionFields value={draft.newRetentionPolicy!} onChange={newRetentionPolicy => setDraft({ ...draft, newRetentionPolicy })} />
}

it.each(fields)('persists blank %s without crashing or masking it; resume and correction retain all other intent', async (key, label) => {
  const writer = new DraftWriter(7, 'new', null)
  const user = userEvent.setup()
  const mounted = render(<DraftForm initial={data} writer={writer} />)
  await user.clear(screen.getByRole('spinbutton', { name: label, exact: true }))
  expect(screen.getByRole('spinbutton', { name: label, exact: true })).toHaveValue(null)
  await waitFor(() => expect(readDraft(7, 'new')?.data.newRetentionPolicy?.[key]).toBe(''))
  const restored = readDraft(7, 'new')!.data
  expect(restored).toEqual({ ...data, newRetentionPolicy: { ...data.newRetentionPolicy, [key]: '' } })
  expect(validateEditor(restored)).not.toEqual([])
  expect(() => serializeCreate(restored)).toThrow('Invalid protection configuration')
  mounted.unmount()
  render(<DraftForm initial={restored} writer={writer} />)
  expect(screen.getByRole('spinbutton', { name: label, exact: true })).toHaveValue(null)
  await user.type(screen.getByRole('spinbutton', { name: label, exact: true }), '4')
  await waitFor(() => expect(readDraft(7, 'new')?.data.newRetentionPolicy?.[key]).toBe(4))
  const corrected = readDraft(7, 'new')!.data
  expect(validateEditor(corrected)).toEqual([])
  expect(serializeCreate(corrected).name).toBe('Preserved plan')
})

it.each([NaN, Infinity, null, '4', {}, undefined])('still rejects malformed retention draft value %s', value => {
  expect(() => safeIntent({ ...data, newRetentionPolicy: { ...data.newRetentionPolicy!, keepLast: value } } as WizardData)).toThrow('Invalid retention draft')
})
