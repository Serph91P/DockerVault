export class ResponseError extends Error {
  constructor(public status?: number, public fields: Record<string, string> = {}) { super('Response unavailable. Check the current server state before retrying.') }
}
export function responseStatus(error: unknown): number | undefined {
  return error instanceof ResponseError ? error.status : (error as { response?: { status?: number } } | null)?.response?.status
}

const validationFields = new Set(['target_type', 'container_name', 'stack_name', 'volume_name', 'host_path', 'selected_volumes', 'include_paths', 'exclude_paths', 'per_volume_rules', 'dependencies', 'stop_container', 'schedule_id', 'schedule_cron', 'retention_policy_id', 'remote_storage_ids', 'delete_local_after_sync', 'compression_enabled', 'pre_backup_command', 'post_backup_command', 'name', 'cron_expression', 'description', 'keep_last', 'keep_daily', 'keep_weekly', 'keep_monthly', 'keep_yearly', 'max_age_days', 'host', 'port', 'username', 'password', 'base_path', 'storage_type', 'api_url', 'api_key', 'api_secret', 'enabled', 'target_path', 's3_bucket', 's3_region', 's3_access_key', 's3_secret_key', 's3_endpoint_url', 'webdav_url', 'rclone_remote', 'ssh_key_path'])
const validationMessages: Record<string, string> = { missing: 'This field is required.', greater_than: 'Value is outside the allowed range.', greater_than_equal: 'Value is outside the allowed range.', less_than: 'Value is outside the allowed range.', less_than_equal: 'Value is outside the allowed range.', int_parsing: 'Enter a whole number.', int_type: 'Enter a whole number.', string_too_short: 'The value is too short.', string_too_long: 'The value is too long.', value_error: 'Check this value.', url_parsing: 'Enter a valid URL.' }
export function safeRequestError(error: { response?: { status?: number; data?: unknown } }) {
  const fields: Record<string, string> = {}
  const detail = (error.response?.data as { detail?: unknown } | undefined)?.detail
  if (error.response?.status === 422 && Array.isArray(detail)) for (const item of detail) {
    if (!item || !Array.isArray(item.loc) || item.loc.length !== 2 || item.loc[0] !== 'body' || !validationFields.has(item.loc[1])) continue
    fields[item.loc[1]] = Object.prototype.hasOwnProperty.call(validationMessages, item.type) ? validationMessages[item.type] : 'Check this value.'
  }
  return new ResponseError(error.response?.status, fields)
}

const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value)
const string = (value: unknown) => typeof value === 'string'
const timestamp = (value: unknown) => typeof value === 'string' && Number.isFinite(Date.parse(value))
const number = (value: unknown) => typeof value === 'number' && Number.isFinite(value)
const id = (value: unknown) => number(value) && Number.isSafeInteger(value) && (value as number) > 0
const boolean = (value: unknown) => typeof value === 'boolean'
const count = (value: unknown) => number(value) && Number.isSafeInteger(value) && Number(value) >= 0
const fields = (value: unknown, required: Record<string, (value: unknown) => boolean>) => object(value) && Object.entries(required).every(([key, check]) => check(value[key]))

const listFields: Record<string, Record<string, (value: unknown) => boolean>> = {
  '/targets': { id, name: string, target_type: v => typeof v === 'string' && ['container', 'volume', 'stack', 'path'].includes(v), enabled: boolean },
  '/backups': { id, target_id: id, status: v => typeof v === 'string' && ['pending', 'running', 'completed', 'failed', 'cancelled'].includes(v), created_at: timestamp },
  '/storage': { id, name: string, storage_type: v => typeof v === 'string' && ['local', 'ssh', 'sftp', 's3', 'webdav', 'ftp', 'rclone'].includes(v), enabled: boolean, base_path: string },
  '/schedules': { id, name: string, cron_expression: string, enabled: boolean },
  '/retention': { id, name: string, keep_last: number, keep_daily: number, keep_weekly: number, keep_monthly: number },
  '/docker/containers': { id: string, name: string, status: string },
  '/docker/volumes': { name: string },
  '/docker/stacks': { name: string, containers: Array.isArray, volumes: Array.isArray },
  '/storage/ssh-keys': { name: string, public_key: string },
}
const objectFields: Record<string, Record<string, (value: unknown) => boolean>> = {
  '/settings/storage': { check_quota_before_upload: boolean },
  '/encryption/status': { setup_completed: boolean, encryption_enabled: boolean },
  '/settings/komodo': { enabled: boolean, has_api_key: boolean, has_api_secret: boolean, connected: boolean },
  '/settings/system-info': { disk_total: number, disk_used: number, disk_free: number, backup_count: number, target_count: number, uptime_seconds: number, app_version: string },
  '/docker/health': { status: v => v === 'healthy' },
  '/schedules/compatibility/inventory': { items: value => Array.isArray(value) && value.every(item => fields(item, { kind: v => v === 'schedule' || v === 'target_legacy', id, name: string, expression: string, warnings: v => Array.isArray(v) && v.every(string) })), timezone: string, contract: string },
}

function validRow(url: string, row: unknown) {
  if (!fields(row, listFields[url]) || !object(row)) return false
  for (const key of ['created_at', 'updated_at', 'started_at', 'completed_at', 'next_run', 'last_run']) if (row[key] !== undefined && row[key] !== null && !(url === '/storage/ssh-keys' && key === 'created_at' ? number(row[key]) : timestamp(row[key]))) return false
  for (const key of ['description', 'target_name', 'container_name', 'volume_name', 'stack_name', 'host_path', 'private_path', 'fingerprint']) if (row[key] !== undefined && row[key] !== null && !string(row[key])) return false
  if (url === '/targets') {
    for (const key of ['selected_volumes', 'include_paths', 'exclude_paths', 'dependencies']) if (row[key] !== undefined && (!Array.isArray(row[key]) || !(row[key] as unknown[]).every(string))) return false
    if (row.remote_storage_ids !== undefined && (!Array.isArray(row.remote_storage_ids) || !row.remote_storage_ids.every(id))) return false
    if (row.schedule != null && !fields(row.schedule, { id, name: string, cron_expression: string })) return false
    if (row.per_volume_rules != null && (!object(row.per_volume_rules) || !Object.values(row.per_volume_rules).every(rule => object(rule) && ['include_paths', 'exclude_paths'].every(key => rule[key] === undefined || (Array.isArray(rule[key]) && (rule[key] as unknown[]).every(string)))))) return false
  }
  if (url === '/backups' && row.remote_sync_details !== undefined && (!Array.isArray(row.remote_sync_details) || !row.remote_sync_details.every(copy => fields(copy, { storage_id: id, storage_name: string, status: string })))) return false
  return true
}

function validOperation(job: unknown) {
  if (!fields(job, { id: string, action: v => v === 'storage_delete', state: v => typeof v === 'string' && ['queued', 'running', 'completed', 'failed', 'cancelled'].includes(v), total: count, completed: count, succeeded: count, failed: count, skipped: count, unknown: count, created_at: string, updated_at: string, path: string, items: Array.isArray }) || !object(job)) return false
  if (!(job.storage_id === 'local' || id(job.storage_id)) || !Array.isArray(job.items) || !job.items.every(item => fields(item, { path: string, state: v => typeof v === 'string' && ['pending', 'running', 'succeeded', 'failed', 'skipped', 'unknown'].includes(v) }))) return false
  const outcomes = ['succeeded', 'failed', 'skipped', 'unknown']
  return job.items.length === job.total && Number(job.completed) <= Number(job.total) && outcomes.reduce((sum, key) => sum + Number(job[key]), 0) === job.completed && outcomes.every(outcome => job[outcome] === (job.items as Record<string, unknown>[]).filter(item => item.state === outcome).length)
}

// Validate before data enters the shared cache; malformed is never confirmed empty.
export function validateWrite(url: string | undefined, data: unknown) {
  if (!url) return
  const familyPath = url.replace(/\/\d+$/, '')
  const shape = listFields[familyPath]
  if (shape && ['/targets', '/storage', '/schedules', '/retention'].includes(familyPath) && !validRow(familyPath, data)) throw new ResponseError()
  if (objectFields[url] && !fields(data, objectFields[url])) throw new ResponseError()
}

export function validateRead(url: string | undefined, data: unknown) {
  if (url === '/schedules/jobs' && (!Array.isArray(data) || !data.every(row => fields(row, { id: value => typeof value === 'string' && value.length > 0, name: string, next_run: value => value === null || timestamp(value) })))) throw new ResponseError()
  if (url && /^\/targets\/\d+$/.test(url) && !validRow('/targets', data)) throw new ResponseError()
  if (url && /^\/backups\/\d+$/.test(url) && !validRow('/backups', data)) throw new ResponseError()
  if (url && /^\/backups\/\d+\/logs$/.test(url) && (!Array.isArray(data) || !data.every(row => fields(row, { id, level: string, step: string, message: string, created_at: timestamp })))) throw new ResponseError()
  if (url && /^\/operations(?:\/[^/]+)?$/.test(url)) {
    const jobs = url === '/operations' ? data : [data]
    if (!Array.isArray(jobs) || !jobs.every(validOperation)) throw new ResponseError()
  }
  const required = url && listFields[url]
  if (required && (!Array.isArray(data) || !data.every(row => validRow(url!, row)))) throw new ResponseError()
  const shape = url && objectFields[url]
  if (shape && !fields(data, shape)) throw new ResponseError()
  return data
}
