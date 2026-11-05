import { afterEach, expect, it, vi } from 'vitest'
import { DraftWriter, readDraft, draftKey, safeIntent } from './planEditorDraft'
import { initialData } from './planEditorModel'

afterEach(() => { localStorage.clear(); vi.useRealTimers() })
it('allowlists nested safe intent, never hook/credential extras', () => {
  const intent = { ...initialData, targetName: 'fixture', preCommand: 'private', credentials: 'private', newSchedule: { name: 'policy', cronExpression: '0 0 * * *', description: '', privateKey: 'private' } }
  const safe = safeIntent(intent)
  expect(safe).not.toHaveProperty('preCommand')
  expect(JSON.stringify(safe)).not.toContain('private')
})
it('rejects malformed nested or scalar draft state instead of reviving a crashing form', () => {
  const base = { version: 1, owner: 7, intent: 'new', revision: null, savedAt: Date.now(), group: 0 }
  for (const data of [{...initialData,targetName:42}, {...initialData,selectedVolumes:[{}]}, {...initialData,newSchedule:{name:[],cronExpression:'* * * * *',description:''}}, {...initialData,perVolumeRules:{x:{excludePaths:'not a list'}}}]) {
    localStorage.setItem(draftKey(7, 'new'), JSON.stringify({...base,data}))
    expect(readDraft(7, 'new')).toBeNull()
  }
})

it('preserves a legacy explicit subset instead of defaulting its missing mode to all', () => {
  localStorage.setItem('dockervault-wizard-draft', JSON.stringify({ownerId:7,savedAt:Date.now(),data:{targetType:'container',containerName:'service',selectedVolumes:['data']}}))
  const restored=readDraft(7,'new')
  expect(restored?.data.volumeMode).toBe('subset')
  expect(restored?.data.selectedVolumes).toEqual(['data'])
})

it('debounces and invalidates timers before success/discard/unmount/session cleanup', () => {
  vi.useFakeTimers()
  const writer = new DraftWriter(7, 'new', null)
  writer.queue(initialData, 0)
  writer.queue({ ...initialData, targetName: 'last' }, 1)
  vi.advanceTimersByTime(350)
  expect(readDraft(7, 'new')?.data.targetName).toBe('last')
  writer.queue({ ...initialData, targetName: 'obsolete' }, 2)
  writer.clear()
  vi.runAllTimers()
  expect(readDraft(7, 'new')).toBeNull()
})
it('isolates owners/intents/versions/expiry and retains base revision for conflict review', () => {
  vi.useFakeTimers()
  const writer = new DraftWriter(7, 'target:9', 'revision')
  writer.queue(initialData, 2)
  vi.advanceTimersByTime(350)
  expect(readDraft(7, 'target:9')?.revision).toBe('revision')
  expect(readDraft(8, 'target:9')).toBeNull()
  expect(readDraft(7, 'new')).toBeNull()
  vi.advanceTimersByTime(24 * 60 * 60 * 1000)
  expect(readDraft(7, 'target:9')).toBeNull()
  localStorage.setItem(draftKey(7, 'new'), JSON.stringify({ version: 0 }))
  expect(readDraft(7, 'new')).toBeNull()
})
