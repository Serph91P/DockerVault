import type { BackupTarget, Container, Stack, Volume } from '../../api'

export interface VolumeRule {
  includePaths?: string[]
  excludePaths?: string[]
}
export interface WizardData {
  targetType: BackupTarget['target_type'] | null
  targetName: string
  containerName: string
  volumeName: string
  hostPath: string
  stackName: string
  volumeMode: 'all' | 'subset'
  selectedVolumes: string[]
  includePaths: string[]
  excludePaths: string[]
  perVolumeRules: Record<string, VolumeRule>
  dependencies: string[]
  stopContainer: boolean
  scheduleMode: 'manual' | 'legacy' | 'shared'
  scheduleCron: string | null
  scheduleId: number | null
  newSchedule: { name: string; cronExpression: string; description: string; enabled?: boolean } | null
  remoteStorageIds: number[]
  deleteLocalAfterSync: boolean
  retentionPolicyId: number | null
  newRetentionPolicy: {
    name: string
    keepLast: number
    keepDaily: number
    keepWeekly: number
    keepMonthly: number
    keepYearly: number
    maxAgeDays: number
  } | null
  compression: 'none' | 'gzip'
  enabled: boolean
  unconfirmed: 'schedule' | 'retention' | 'target' | null
  confirmedTargetId: number | null
}
export const initialData: WizardData = {
  targetType: null,
  targetName: '',
  containerName: '',
  volumeName: '',
  hostPath: '',
  stackName: '',
  volumeMode: 'all',
  selectedVolumes: [],
  includePaths: [],
  excludePaths: [],
  perVolumeRules: {},
  dependencies: [],
  stopContainer: true,
  scheduleMode: 'manual',
  scheduleCron: null,
  scheduleId: null,
  newSchedule: null,
  remoteStorageIds: [],
  deleteLocalAfterSync: false,
  retentionPolicyId: null,
  newRetentionPolicy: null,
  compression: 'gzip',
  enabled: true,
  unconfirmed: null,
  confirmedTargetId: null,
}
export const volumeRuleKey = (name: string) => `volume:${name}`
export const retentionDefaults = {
  name: '',
  keepLast: 3,
  keepDaily: 7,
  keepWeekly: 4,
  keepMonthly: 6,
  keepYearly: 2,
  maxAgeDays: 365,
}
export function targetToWizardData(target: BackupTarget): WizardData {
  return {
    ...initialData,
    targetType: target.target_type,
    targetName: target.name,
    containerName: target.container_name || '',
    volumeName: target.volume_name || '',
    hostPath: target.host_path || '',
    stackName: target.stack_name || '',
    volumeMode: target.selected_volumes?.length ? 'subset' : 'all',
    selectedVolumes: target.selected_volumes || [],
    includePaths: target.include_paths || [],
    excludePaths: target.exclude_paths || [],
    perVolumeRules: Object.fromEntries(
      Object.entries(target.per_volume_rules || {}).map(([key, rule]) => [
        key,
        {
          ...(rule.include_paths !== undefined ? { includePaths: rule.include_paths } : {}),
          ...(rule.exclude_paths !== undefined ? { excludePaths: rule.exclude_paths } : {}),
        },
      ])
    ),
    dependencies: target.dependencies || [],
    stopContainer: target.stop_container,
    scheduleMode: target.schedule_id ? 'shared' : target.schedule_cron ? 'legacy' : 'manual',
    scheduleCron: target.schedule_cron || null,
    scheduleId: target.schedule_id ?? null,
    retentionPolicyId: target.retention_policy_id ?? null,
    remoteStorageIds: target.remote_storage_ids || [],
    deleteLocalAfterSync: target.delete_local_after_sync || false,
    compression: target.compression_enabled ? 'gzip' : 'none',
    enabled: target.enabled,
  }
}
export function changeSource(data: WizardData, updates: Partial<WizardData>): WizardData {
  return { ...data, ...updates, selectedVolumes: [], volumeMode: 'all', dependencies: [], perVolumeRules: {} }
}
export function availableVolumes(data: WizardData, containers: Container[], stacks: Stack[]): string[] {
  return [
    ...new Set(
      data.targetType === 'stack'
        ? stacks.find((s) => s.name === data.stackName)?.volumes || []
        : containers
            .find((c) => c.name === data.containerName)
            ?.mounts.filter((m) => m.type === 'volume')
            .map((m) => m.name || '')
            .filter(Boolean) || []
    ),
  ]
}
export function validateEditor(data: WizardData): string[] {
  const errors: string[] = []
  if (
    !data.targetType ||
    !{ container: data.containerName, stack: data.stackName, volume: data.volumeName, path: data.hostPath }[
      data.targetType
    ]
  )
    errors.push('Choose a source.')
  if (!data.targetName.trim() || data.targetName.length > 255) errors.push('Name must contain 1–255 characters.')
  if (
    ['container', 'stack'].includes(data.targetType || '') &&
    data.volumeMode === 'subset' &&
    !data.selectedVolumes.length
  )
    errors.push('Select at least one volume or explicitly choose all volumes.')
  if (data.newSchedule && (!data.newSchedule.name.trim() || !data.newSchedule.cronExpression.trim()))
    errors.push('Schedule name and cron expression are required.')
  const r = data.newRetentionPolicy
  if (r) {
    if (!r.name.trim()) errors.push('Retention name is required.')
    const valid = (n: number, min: number, max: number) => Number.isInteger(n) && n >= min && n <= max
    if (
      !valid(r.keepLast, 1, 1000) ||
      !valid(r.maxAgeDays, 1, 36500) ||
      [r.keepDaily, r.keepWeekly, r.keepMonthly, r.keepYearly].some((n) => !valid(n, 0, 1000))
    )
      errors.push('Keep Last must be 1–1000; age must be 1–36500; GFS buckets must be 0–1000.')
  }
  return errors
}
export function topologyErrors(
  data: WizardData,
  containers: Container[],
  stacks: Stack[],
  volumes: Volume[]
): string[] {
  const errors: string[] = []
  if (
    (data.targetType === 'container' && !containers.some((c) => c.name === data.containerName)) ||
    (data.targetType === 'stack' && !stacks.some((s) => s.name === data.stackName)) ||
    (data.targetType === 'volume' && !volumes.some((v) => v.name === data.volumeName))
  )
    errors.push('Source is missing from current discovery. Selection has not been widened.')
  if (['container', 'stack'].includes(data.targetType || '')) {
    const known = availableVolumes(data, containers, stacks)
    if (!known.length || data.selectedVolumes.some((v) => !known.includes(v)))
      errors.push('Selected volumes are missing from current discovery. Recheck scope.')
    if (Object.keys(data.perVolumeRules).some((key) => key.startsWith('volume:') && !known.includes(key.slice(7))))
      errors.push('A volume rule refers to a missing volume. Rebind or remove it explicitly.')
  }
  if (data.dependencies.some((name) => !containers.some((c) => c.name === name)))
    errors.push('A dependency is missing. Recheck execution rules.')
  return errors
}
export function serializeUpdate(data: WizardData): Partial<BackupTarget> {
  if (validateEditor(data).length) throw new Error('Invalid protection configuration')
  return {
    name: data.targetName,
    enabled: data.enabled,
    dependencies: data.dependencies,
    selected_volumes: data.volumeMode === 'all' ? [] : data.selectedVolumes,
    include_paths: data.includePaths,
    exclude_paths: data.excludePaths,
    per_volume_rules: Object.fromEntries(
      Object.entries(data.perVolumeRules).map(([key, rule]) => [
        key,
        {
          ...(rule.includePaths !== undefined ? { include_paths: rule.includePaths } : {}),
          ...(rule.excludePaths !== undefined ? { exclude_paths: rule.excludePaths } : {}),
        },
      ])
    ),
    stop_container: data.stopContainer,
    compression_enabled: data.compression === 'gzip',
    schedule_id: data.scheduleMode === 'manual' ? null : data.scheduleId,
    schedule_cron: data.scheduleMode === 'manual' ? null : data.scheduleCron,
    retention_policy_id: data.retentionPolicyId,
    remote_storage_ids: data.remoteStorageIds,
    delete_local_after_sync: data.remoteStorageIds.length > 0 && data.deleteLocalAfterSync,
  }
}
export function serializeCreate(data: WizardData): Partial<BackupTarget> {
  const source = {
    container: { container_name: data.containerName },
    stack: { stack_name: data.stackName },
    volume: { volume_name: data.volumeName },
    path: { host_path: data.hostPath },
  }
  return { ...serializeUpdate(data), target_type: data.targetType!, ...source[data.targetType!] }
}
