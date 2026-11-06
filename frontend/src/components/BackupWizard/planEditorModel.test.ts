import { describe, expect, it } from 'vitest'
import { initialData, targetToWizardData, serializeCreate, serializeUpdate, validateEditor, changeSource } from './planEditorModel'
import type { BackupTarget } from '../../api'

const target = { id: 9, name: 'fixture', target_type: 'container', container_name: 'service', enabled: false,
  selected_volumes: ['data'], include_paths: ['config'], exclude_paths: ['cache'],
  per_volume_rules: { 'volume:data': { include_paths: [], exclude_paths: [] }, legacy: {} },
  dependencies: ['db'], stop_container: false, compression_enabled: false, schedule_id: null,
  schedule_cron: '0 3 * * *', retention_policy_id: null, remote_storage_ids: [2, 3], delete_local_after_sync: true,
} as unknown as BackupTarget

describe('one safe editor model', () => {
  it.each(['container', 'stack', 'volume', 'path'] as const)('roundtrips every mutable field for %s without pretending source updates', kind => {
    const original = { ...target, target_type: kind, stack_name: 'stack', volume_name: 'data', host_path: '/fixture' }
    const data = targetToWizardData(original)
    expect(serializeUpdate(data)).toEqual({ name: 'fixture', enabled: false, selected_volumes: ['data'], include_paths: ['config'], exclude_paths: ['cache'], per_volume_rules: target.per_volume_rules, dependencies: ['db'], stop_container: false, compression_enabled: false, schedule_id: null, schedule_cron: '0 3 * * *', retention_policy_id: null, remote_storage_ids: [2, 3], delete_local_after_sync: true })
    const create = serializeCreate(data)
    expect(create.target_type).toBe(kind)
    expect(Object.keys(create).filter(k => ['container_name','stack_name','volume_name','host_path'].includes(k))).toEqual([{container:'container_name',stack:'stack_name',volume:'volume_name',path:'host_path'}[kind]])
    expect(data).not.toHaveProperty('preCommand')
  })
  it('only explicit all serializes [] and an invalid empty subset cannot widen scope', () => {
    const data = { ...initialData, targetType: 'container' as const, containerName: 'service', targetName: 'fixture', volumeMode: 'subset' as const }
    expect(validateEditor(data)).toContain('Select at least one volume or explicitly choose all volumes.')
    expect(() => serializeCreate(data)).toThrow()
    expect(serializeCreate({ ...data, volumeMode: 'all' }).selected_volumes).toEqual([])
  })
  it('manual clears both schedules; empty remotes clear local deletion', () => {
    const data = { ...targetToWizardData(target), scheduleMode: 'manual' as const, remoteStorageIds: [] }
    expect(serializeUpdate(data)).toMatchObject({ schedule_id: null, schedule_cron: null, delete_local_after_sync: false })
  })
  it('source changes explicitly invalidate dependencies and volume rules', () => {
    expect(changeSource(targetToWizardData(target), { containerName: 'other' })).toMatchObject({ selectedVolumes: [], volumeMode: 'all', perVolumeRules: {}, dependencies: [] })
  })
  it('validates full policy fields and disallows unsupported format', () => {
    expect(validateEditor({ ...targetToWizardData(target), newRetentionPolicy: { name: 'x', keepLast: 0, keepDaily: 7, keepWeekly: 4, keepMonthly: 6, keepYearly: 2, maxAgeDays: 365 } })).toContain('Keep Last must be 1–1000; age must be 1–36500; GFS buckets must be 0–1000.')
  })
})
