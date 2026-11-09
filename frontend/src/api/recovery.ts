import api from './index'

export const recoveryTables = {
  backup_targets: 'Targets', schedules: 'Schedules', retention_policies: 'Retention policies',
  remote_storages: 'Storages', encryption_config: 'Archive encryption configuration', app_settings: 'Portable settings',
  backups: 'Catalog entries', backup_schedules: 'Target / schedule bindings', backup_storage_syncs: 'Catalog storage copies',
} as const
export type RecoveryCounts = Record<keyof typeof recoveryTables, number>
export type ConnectionState = 'connected' | 'offline' | 'credentials_required'
export interface RecoveryStorage { id: number; type: string; state: ConnectionState | 'not_tested' }
export interface RecoveryPreview { counts: RecoveryCounts; exported_at: string; storages: RecoveryStorage[]; archives_included: false; archive_private_keys_included: false }
export interface RecoveryResult {
  ssh_public_keys: { name: string; public_key: string }[];
  counts: RecoveryCounts; storages: RecoveryStorage[]; komodo: { state: ConnectionState | 'disabled' };
  catalog: { id: number; local: 'available' | 'unavailable' | 'credentials_required'; copies: { storage_id: number; state: 'available' | 'unavailable' | 'offline' | 'credentials_required' }[] }[];
  archives_modified: false
}
const invalid = (): never => { throw new Error('Invalid recovery response') }
const object = (value: unknown): Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : invalid()
const id = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : invalid()
const member = <T extends string>(value: unknown, options: readonly T[]): T => typeof value === 'string' && options.includes(value as T) ? value as T : invalid()
function counts(value: unknown): RecoveryCounts {
  const data = object(value), result = {} as RecoveryCounts
  if (Object.keys(data).length !== Object.keys(recoveryTables).length) invalid()
  for (const key of Object.keys(recoveryTables) as (keyof RecoveryCounts)[]) {
    if (typeof data[key] !== 'number' || !Number.isSafeInteger(data[key]) || (data[key] as number) < 0) invalid()
    result[key] = data[key] as number
  }
  return result
}
function storages(value: unknown, preview: boolean): RecoveryStorage[] {
  if (!Array.isArray(value)) return invalid()
  return value.map(item => {
    const data = object(item)
    return { id: id(data.id), type: member(data.type, ['local', 'ssh', 'sftp', 's3', 'ftp', 'webdav', 'rclone']), state: member(data.state, preview ? ['credentials_required', 'not_tested'] : ['connected', 'offline', 'credentials_required']) }
  })
}
function preview(value: unknown): RecoveryPreview {
  const data = object(value)
  if (data.archives_included !== false || data.archive_private_keys_included !== false || typeof data.exported_at !== 'string' || !Number.isFinite(Date.parse(data.exported_at))) invalid()
  return { counts: counts(data.counts), storages: storages(data.storages, true), exported_at: data.exported_at as string, archives_included: false, archive_private_keys_included: false }
}
function publicReferences(value: unknown): RecoveryResult['ssh_public_keys'] {
  if (value === undefined) return []
  if (!Array.isArray(value)) return invalid()
  return value.map(item => {
    const data = object(item)
    if (typeof data.name !== 'string' || !/^[a-zA-Z0-9_.-]{1,128}$/.test(data.name) || typeof data.public_key !== 'string' || data.public_key.length > 16384 || !/^[a-zA-Z0-9@._+-]+ [A-Za-z0-9+/]+=*(?: [^\r\n]*)?$/.test(data.public_key)) return invalid()
    return { name: data.name, public_key: data.public_key }
  })
}
export function parseRecoveryResult(value: unknown): RecoveryResult {
  const data = object(value)
  if (data.archives_modified !== false || !Array.isArray(data.catalog)) return invalid()
  return { ssh_public_keys: publicReferences(data.ssh_public_keys), counts: counts(data.counts), storages: storages(data.storages, false), komodo: { state: member(object(data.komodo).state, ['connected', 'offline', 'credentials_required', 'disabled']) }, catalog: data.catalog.map(item => {
    const entry = object(item)
    if (!Array.isArray(entry.copies)) return invalid()
    return { id: id(entry.id), local: member(entry.local, ['available', 'unavailable', 'credentials_required']), copies: entry.copies.map(item => {
      const copy = object(item)
      return { storage_id: id(copy.storage_id), state: member(copy.state, ['available', 'unavailable', 'offline', 'credentials_required']) }
    }) }
  }), archives_modified: false }
}
const options = (signal: AbortSignal) => ({ signal, headers: { 'Cache-Control': 'no-store' } })
export const recoveryApi = {
  export: (password: string, confirm_password: string, signal: AbortSignal) => api.post<Blob>('/recovery/export', { password, confirm_password }, { ...options(signal), responseType: 'blob' }),
  preview: async (bundle: string, password: string, signal: AbortSignal) => preview((await api.post('/recovery/preview', { bundle, password }, options(signal))).data),
  restore: async (bundle: string, password: string, username: string, admin_password: string, confirm_admin_password: string, signal: AbortSignal) => {
    const data = object((await api.post('/recovery/restore', { bundle, password, username, admin_password, confirm_admin_password }, options(signal))).data)
    if (object(data.user).is_admin !== true) return invalid()
    id(object(data.user).id)
    return parseRecoveryResult(data.result)
  },
  status: async (signal: AbortSignal) => { const value = (await api.get('/recovery/status', options(signal))).data; return value === null ? null : parseRecoveryResult(value) },
}
