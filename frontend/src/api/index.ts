import axios from 'axios'
import { validateRead, validateWrite } from './responses'

const api = axios.create({
  baseURL: '/api/v1',
  headers: {
    'Content-Type': 'application/json',
  },
  // Include credentials (cookies) for authentication
  withCredentials: true,
})

api.interceptors.response.use(response => {
  if (response.config.method === 'get') validateRead(response.config.url, response.data)
  if (response.config.method === 'post' || response.config.method === 'put') validateWrite(response.config.url, response.data)
  return response
})

// Types
export interface Container {
  id: string
  name: string
  image: string
  status: string
  state: string
  created: string
  labels: Record<string, string>
  mounts: Mount[]
  networks: string[]
  compose_project?: string
  compose_service?: string
  depends_on: string[]
}

export interface Mount {
  type: string
  source: string
  destination: string
  name?: string
  mode: string
  rw: boolean
}

export interface Volume {
  name: string
  driver: string
  mountpoint: string
  labels: Record<string, string>
  created_at: string
  used_by: string[]
}

export interface Stack {
  name: string
  containers: Container[]
  volumes: string[]
  networks: string[]
}

export interface BackupTarget {
  removed_at?: string | null
  id: number
  name: string
  target_type: 'container' | 'volume' | 'path' | 'stack'
  container_id?: string
  container_name?: string
  volume_name?: string
  host_path?: string
  stack_name?: string
  schedule_id?: number | null  // Explicit null removes the reference on update
  schedule?: {  // NEW: Embedded schedule info
    id: number
    name: string
    cron_expression: string
  }
  schedule_cron?: string | null  // DEPRECATED: Keep for backwards compatibility
  enabled: boolean
  retention_policy_id?: number | null
  retention_policy?: RetentionPolicyInfo  // Embedded retention policy info
  dependencies: string[]
  // Volume selection for container/stack backups
  selected_volumes: string[]  // Empty = all volumes
  // Path filtering
  include_paths: string[]  // Include only these paths (empty = all)
  exclude_paths: string[]  // Exclude these paths/patterns
  // Per-volume path rules (overrides global for specific volumes)
  per_volume_rules: Record<string, { include_paths?: string[]; exclude_paths?: string[] }>
  pre_backup_command?: string | null
  post_backup_command?: string | null
  stop_container: boolean
  compression_enabled: boolean
  // Remote storage sync
  remote_storage_ids: number[]
  delete_local_after_sync: boolean
  created_at: string
  updated_at: string
}

export interface RemoteSyncInfo {
  storage_id: number
  storage_name: string
  availability?: 'unknown' | 'inaccessible'
  status: 'completed' | 'failed' | 'pending' | string
  synced_at?: string
  remote_path?: string
}

export interface Backup {
  id: number
  target_id: number
  target_name?: string
  backup_type: 'full' | 'incremental'
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled'
  file_path?: string
  file_size?: number
  file_size_human?: string
  checksum?: string
  started_at?: string
  completed_at?: string
  duration_seconds?: number
  error_message?: string
  encrypted?: boolean
  local_available?: boolean
  remote_synced?: boolean
  remote_sync_details?: RemoteSyncInfo[]
  created_at: string
}

export interface RestoreSelection {
  destination_type: 'original' | 'volume' | 'path'
  target_volume?: string
  target_path?: string
}
export interface RestoreImpact {
  fingerprint: string
  destination: { type: RestoreSelection['destination_type']; path: string; volume: string | null }
  containers_to_stop: string[]
  containers_to_restart: string[]
  layout: string
  overwrite: string
  key_required: boolean
}
export interface RestoreInfo {
  original_supported: boolean
  volume_discovery: 'available' | 'unavailable'
  remote_copies: RemoteSyncInfo[]
  backup: {
    id: number
    file_size_human?: string
    encrypted: boolean
    created_at: string
    completed_at?: string
    local_available: boolean
  }
  target: {
    id: number | null
    name: string
    target_type: string | null
    volume_name?: string
    host_path?: string
    container_name?: string
  }
  containers_to_stop: string[]
  available_volumes: string[]
}

export interface BackupLogEntry {
  id: number
  level: 'debug' | 'info' | 'warning' | 'error'
  step: string
  message: string
  details?: Record<string, unknown>
  created_at: string
}

export interface RetentionPolicy {
  id: number
  name: string
  keep_last: number
  keep_daily: number
  keep_weekly: number
  keep_monthly: number
  keep_yearly: number
  max_age_days: number
  created_at: string
  updated_at: string
}

// Embedded retention policy info for targets
export interface RetentionPolicyInfo {
  id: number
  name: string
  keep_last: number
  keep_daily: number
  keep_weekly: number
  keep_monthly: number
}

export interface Schedule {
  id: number
  target_id: number
  target_name: string
  cron_expression: string
  next_run?: string
  last_run?: string
  enabled: boolean
}

// NEW: Schedule entity (standalone, reusable)
export interface ScheduleEntity {
  id: number
  name: string
  cron_expression: string
  description?: string
  enabled: boolean
  target_count: number
  next_run?: string
  created_at: string
  updated_at: string
}

export interface ScheduleWithTargets extends ScheduleEntity {
  targets: Array<{
    id: number
    name: string
    target_type: string
    enabled: boolean
  }>
}

export interface ScheduleCreate {
  name: string
  cron_expression: string
  description?: string
  enabled?: boolean
}

export interface ScheduleUpdate {
  name?: string
  cron_expression?: string
  description?: string
  enabled?: boolean
}

// Docker API
export const dockerApi = {
  getHealth: () => api.get('/docker/health'),
  listContainers: () => api.get<Container[]>('/docker/containers'),
  getContainer: (id: string) => api.get<Container>(`/docker/containers/${id}`),
  listVolumes: () => api.get<Volume[]>('/docker/volumes'),
  listStacks: () => api.get<Stack[]>('/docker/stacks'),
}

// Targets API
export const targetsApi = {
  list: (includeRemoved = false) => api.get<BackupTarget[]>('/targets', { params: { include_removed: includeRemoved } }),
  get: (id: number) => api.get<BackupTarget>(`/targets/${id}`),
  create: (data: Partial<BackupTarget>) => api.post<BackupTarget>('/targets', data),
  update: (id: number, data: Partial<BackupTarget>) => api.put<BackupTarget>(`/targets/${id}`, data),
  delete: (id: number) => api.delete(`/targets/${id}`),
}

// Backups API
export type RemovalScope = 'local' | 'remote' | 'all'
export interface RemovalItem {
  backup_id: number
  target_id: number
  target_name: string
  copy: 'local' | 'remote'
  storage_id: number | null
  label: string
  outcome?: 'succeeded' | 'failed' | 'skipped' | 'retained'
  message?: string
}
export interface RemovalPreview {
  token: string
  scope: RemovalScope
  archive_count: number
  copy_count: number
  items: RemovalItem[]
  retained: RemovalItem[]
}
export interface RemovalResult {
  status: 'completed' | 'incomplete'
  succeeded: number
  failed: number
  skipped: number
  warning?: string
  items: RemovalItem[]
  retry_token: string | null
}
export interface HistoryRecord extends Backup {
  source_type: string
  source_identity: string
  local_storage_path?: string | null
  plan_removed: boolean
  retention_context: string
  archive_state: 'recorded' | 'never_created' | 'unknown'
  local_state: 'available' | 'missing' | 'inaccessible' | 'not_recorded' | 'deleted'
  stage?: string | null
  can_restore: boolean
  can_sync: boolean
}
export interface HistoryPage {
  items: HistoryRecord[]; total: number; limit: number; offset: number; snapshot: number; dataset: string
}
export const backupsApi = {
  history: (params: Record<string, string | number>, signal?: AbortSignal) => api.get<HistoryPage>('/backups/history', { params, signal }),
  historyDetail: (id: number, signal?: AbortSignal) => api.get<HistoryRecord>(`/backups/history/${id}`, { signal }),
  previewRemoval: (selection: { scope: RemovalScope; backup_ids?: number[]; target_id?: number; status?: string }) =>
    api.post<RemovalPreview>('/backups/removal/preview', selection),
  executeRemoval: (token: string) => api.post<RemovalResult>('/backups/removal/execute', { token }),
  list: (params?: { target_id?: number; status?: string; limit?: number }) => 
    api.get<Backup[]>('/backups', { params }),
  get: (id: number) => api.get<Backup>(`/backups/${id}`),
  create: (target_id: number, backup_type = 'full') => 
    api.post<Backup>('/backups', { target_id, backup_type }),
  restore: (id: number, opts?: Partial<RestoreSelection> & { private_key?: string; review_fingerprint?: string }) =>
    api.post(`/backups/${id}/restore`, {
      destination_type: opts?.destination_type,
      target_volume: opts?.target_volume,
      review_fingerprint: opts?.review_fingerprint,
      target_path: opts?.target_path,
      private_key: opts?.private_key,
    }),
  getRestoreInfo: (id: number) =>
    api.get<RestoreInfo>(`/backups/${id}/restore-info`),
  delete: (id: number, opts?: { deleteLocal?: boolean; deleteRemote?: boolean }) => 
    api.delete(`/backups/${id}`, { 
      params: { 
        delete_local: opts?.deleteLocal ?? true, 
        delete_remote: opts?.deleteRemote ?? true 
      } 
    }),
  deleteAll: (targetId?: number, opts?: { deleteLocal?: boolean; deleteRemote?: boolean }) => 
    api.delete('/backups', { 
      params: { 
        ...(targetId ? { target_id: targetId } : undefined),
        delete_local: opts?.deleteLocal ?? true,
        delete_remote: opts?.deleteRemote ?? true,
      } 
    }),
  getStats: (id: number) => api.get(`/backups/${id}/stats`),
  listFiles: (id: number, limit = 2000, offset = 0) => api.get(`/backups/${id}/files`, { params: { limit, offset } }),
  listFilesEncrypted: (id: number, privateKey: string, limit = 2000, offset = 0) =>
    api.post(`/backups/${id}/files`, { private_key: privateKey }, { params: { limit, offset } }),
  retrySync: (id: number) => api.post(`/backups/${id}/retry-sync`),
  getLogs: (id: number) => api.get<BackupLogEntry[]>(`/backups/${id}/logs`),
}

// Schedules API (NEW: CRUD for Schedule entities)
export const schedulesApi = {
  // New Schedule entity CRUD
  list: () => api.get<ScheduleEntity[]>('/schedules'),
  get: (id: number) => api.get<ScheduleWithTargets>(`/schedules/${id}`),
  create: (data: ScheduleCreate) => api.post<ScheduleEntity>('/schedules', data),
  update: (id: number, data: ScheduleUpdate) => api.put<ScheduleEntity>(`/schedules/${id}`, data),
  delete: (id: number) => api.delete(`/schedules/${id}`),
  
  // Scheduler management
  getJobs: () => api.get<Array<{ id: string; name: string; next_run: string | null }>>('/schedules/jobs'),
  reload: () => api.post('/schedules/reload'),
  trigger: (targetId: number) => api.post(`/schedules/target/${targetId}/trigger`),
  triggerAll: (scheduleId: number) => api.post(`/schedules/${scheduleId}/trigger-all`),
  estimate: (target_id: number, cron_expression: string) =>
    api.post('/schedules/estimate', { target_id, cron_expression }),
  getCronHelp: () => api.get('/schedules/cron-help'),
  preview: (cron_expression: string) => api.post<{ next_runs: string[]; timezone: string; contract: string; warnings: string[] }>('/schedules/preview', { cron_expression }),
  cronInventory: () => api.get<{ contract: string; timezone: string; items: { kind: string; id: number; name: string; expression: string; warnings: string[] }[] }>('/schedules/compatibility/inventory'),
  
  // Legacy endpoint for backwards compatibility
  listLegacy: () => api.get<Schedule[]>('/schedules/legacy/by-target'),
}

// Retention API
export interface CleanupResult {
  outcome: 'empty' | 'success' | 'partial' | 'failure'
  deleted: number
  failed: number
  protected: number
  scan_errors: number
  scan_complete: boolean
  freed_bytes: number
  freed_human: string
}
export const retentionApi = {
  listPolicies: () => api.get<RetentionPolicy[]>('/retention'),
  getPolicy: (id: number) => api.get<RetentionPolicy>(`/retention/${id}`),
  createPolicy: (data: Partial<RetentionPolicy>) => api.post<RetentionPolicy>('/retention', data),
  updatePolicy: (id: number, data: Partial<RetentionPolicy>) => 
    api.put<RetentionPolicy>(`/retention/${id}`, data),
  deletePolicy: (id: number) => api.delete(`/retention/${id}`),
  applyRetention: (targetId: number) => api.post(`/retention/${targetId}/apply`),
  getStats: (targetId: number) => api.get(`/retention/${targetId}/stats`),
  cleanupOrphaned: () => api.post<CleanupResult>('/retention/cleanup-orphaned'),
}

// Remote Storage Types
export interface RemoteStorage {
  id: number
  name: string
  storage_type: 'local' | 'ssh' | 'sftp' | 'webdav' | 's3' | 'ftp' | 'rclone'
  enabled: boolean
  host?: string
  port?: number
  username?: string
  base_path: string
  ssh_key_path?: string
  s3_bucket?: string
  s3_region?: string
  s3_endpoint_url?: string
  webdav_url?: string
  rclone_remote?: string
  created_at: string
  updated_at: string
}

// Storage API
export const storageApi = {
  list: () => api.get<RemoteStorage[]>('/storage'),
  get: (id: number) => api.get<RemoteStorage>(`/storage/${id}`),
  create: (data: Partial<RemoteStorage>) => api.post<RemoteStorage>('/storage', data),
  update: (id: number, data: Partial<RemoteStorage>) => api.put<RemoteStorage>(`/storage/${id}`, data),
  delete: (id: number) => api.delete(`/storage/${id}`),
  test: (id: number) => api.post(`/storage/${id}/test`),
  getTypes: () => api.get('/storage/types'),
  listFiles: (id: number, path?: string, signal?: AbortSignal) => api.get(`/storage/${id}/files`, { params: { path: path || '' }, signal }),
  directorySize: (id: number | 'local', path: string, signal?: AbortSignal) =>
    api.get<{ path: string; state: 'completed' | 'unavailable'; bytes: number | null }>(`/storage/${id}/files/size`, { params: { path }, signal }),
  deleteFile: (id: number, path: string) => api.delete(`/storage/${id}/files`, { params: { path } }),
  bulkDeleteFiles: (id: number, paths: string[]) =>
    api.post(`/storage/${id}/files/bulk-delete`, { paths }),
  bulkDownloadFiles: (id: number, paths: string[]) =>
    api.post(`/storage/${id}/files/bulk-download`, { paths }, { responseType: 'blob' }),
  // Local backup files
  listLocalFiles: (path?: string, signal?: AbortSignal) =>
    api.get('/storage/local/files', { params: { path: path || '' }, signal }),
  deleteLocalFile: (path: string) =>
    api.delete('/storage/local/files', { params: { path } }),
  bulkDeleteLocalFiles: (paths: string[]) =>
    api.post('/storage/local/files/bulk-delete', { paths }),
  bulkDownloadLocalFiles: (paths: string[]) =>
    api.post('/storage/local/files/bulk-download', { paths }, { responseType: 'blob' }),
  // SSH key management
  listSSHKeys: () => api.get<SSHKeyInfo[]>('/storage/ssh-keys'),
  generateSSHKey: (data: { name: string; comment?: string; overwrite?: boolean }) =>
    api.post<SSHKeyInfo>('/storage/ssh-keys', data),
  deleteSSHKey: (name: string) => api.delete(`/storage/ssh-keys/${name}`),
  installSSHKey: (
    name: string,
    data: { host: string; port: number; username: string; password: string; method?: string },
  ) => api.post(`/storage/ssh-keys/${name}/install`, data),
  downloadSSHPublicKeyURL: (name: string) => `/api/v1/storage/ssh-keys/${name}/public`,
}

export interface SSHKeyInfo {
  name: string
  private_path: string
  public_path: string
  public_key: string
  fingerprint?: string | null
  created_at: number
}

// Komodo API
export const komodoApi = {
  getStatus: () => api.get('/komodo/status'),
  test: () => api.post('/komodo/test'),
  containerAction: (container_name: string, action: string, reason?: string) =>
    api.post('/komodo/container-action', { container_name, action, reason }),
  getContainerStatus: (container_name: string) => 
    api.get(`/komodo/container/${container_name}`),
}

// Settings API
export interface KomodoSettings {
  enabled: boolean
  api_url: string | null
  has_api_key: boolean
  has_api_secret: boolean
  connected: boolean
}

export interface KomodoTestResult {
  success: boolean
  message: string
  version?: string
}

export interface SystemInfo {
  disk_status?: 'available' | 'unavailable'
  backup_dir: string
  database_path: string
  timezone: string
  disk_total: number
  disk_used: number
  disk_free: number
  db_size: number
  backup_count: number
  target_count: number
  uptime_seconds: number
  app_version: string
}

export interface StorageSettings {
  check_quota_before_upload: boolean
}

export const settingsApi = {
  getKomodo: () => api.get<KomodoSettings>('/settings/komodo'),
  updateKomodo: (data: { enabled: boolean; api_url?: string; api_key?: string; api_secret?: string }) =>
    api.put<KomodoSettings>('/settings/komodo', data),
  testKomodo: () => api.post<KomodoTestResult>('/settings/komodo/test'),
  getSystemInfo: () => api.get<SystemInfo>('/settings/system-info'),
  getStorageSettings: () => api.get<StorageSettings>('/settings/storage'),
  updateStorageSettings: (data: StorageSettings) =>
    api.put<StorageSettings>('/settings/storage', data),
}

export default api
