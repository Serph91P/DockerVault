import axios from 'axios'

const api = axios.create({
  baseURL: '/api/v1',
  headers: {
    'Content-Type': 'application/json',
  },
  // Include credentials (cookies) for authentication
  withCredentials: true,
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
  id: number
  name: string
  target_type: 'container' | 'volume' | 'path' | 'stack'
  container_id?: string
  container_name?: string
  volume_name?: string
  host_path?: string
  stack_name?: string
  schedule_id?: number  // NEW: Reference to Schedule entity
  schedule?: {  // NEW: Embedded schedule info
    id: number
    name: string
    cron_expression: string
  }
  schedule_cron?: string  // DEPRECATED: Keep for backwards compatibility
  enabled: boolean
  retention_policy_id?: number
  dependencies: string[]
  pre_backup_command?: string
  post_backup_command?: string
  stop_container: boolean
  compression_enabled: boolean
  created_at: string
  updated_at: string
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
  created_at: string
}

export interface RetentionPolicy {
  id: number
  name: string
  keep_daily: number
  keep_weekly: number
  keep_monthly: number
  keep_yearly: number
  max_age_days: number
  created_at: string
  updated_at: string
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
  stopContainer: (id: string) => api.post(`/docker/containers/${id}/stop`),
  startContainer: (id: string) => api.post(`/docker/containers/${id}/start`),
  listVolumes: () => api.get<Volume[]>('/docker/volumes'),
  listStacks: () => api.get<Stack[]>('/docker/stacks'),
}

// Targets API
export const targetsApi = {
  list: () => api.get<BackupTarget[]>('/targets'),
  get: (id: number) => api.get<BackupTarget>(`/targets/${id}`),
  create: (data: Partial<BackupTarget>) => api.post<BackupTarget>('/targets', data),
  update: (id: number, data: Partial<BackupTarget>) => api.put<BackupTarget>(`/targets/${id}`, data),
  delete: (id: number) => api.delete(`/targets/${id}`),
}

// Backups API
export const backupsApi = {
  list: (params?: { target_id?: number; status?: string; limit?: number }) => 
    api.get<Backup[]>('/backups', { params }),
  get: (id: number) => api.get<Backup>(`/backups/${id}`),
  create: (target_id: number, backup_type = 'full') => 
    api.post<Backup>('/backups', { target_id, backup_type }),
  restore: (id: number, target_path?: string) => 
    api.post(`/backups/${id}/restore`, { target_path }),
  delete: (id: number) => api.delete(`/backups/${id}`),
  getStats: (id: number) => api.get(`/backups/${id}/stats`),
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
  getJobs: () => api.get('/schedules/jobs'),
  trigger: (targetId: number) => api.post(`/schedules/target/${targetId}/trigger`),
  estimate: (target_id: number, cron_expression: string) =>
    api.post('/schedules/estimate', { target_id, cron_expression }),
  getCronHelp: () => api.get('/schedules/cron-help'),
  
  // Legacy endpoint for backwards compatibility
  listLegacy: () => api.get<Schedule[]>('/schedules/legacy/by-target'),
}

// Retention API
export const retentionApi = {
  listPolicies: () => api.get<RetentionPolicy[]>('/retention'),
  getPolicy: (id: number) => api.get<RetentionPolicy>(`/retention/${id}`),
  createPolicy: (data: Partial<RetentionPolicy>) => api.post<RetentionPolicy>('/retention', data),
  updatePolicy: (id: number, data: Partial<RetentionPolicy>) => 
    api.put<RetentionPolicy>(`/retention/${id}`, data),
  deletePolicy: (id: number) => api.delete(`/retention/${id}`),
  applyRetention: (targetId: number) => api.post(`/retention/${targetId}/apply`),
  getStats: (targetId: number) => api.get(`/retention/${targetId}/stats`),
  cleanupOrphaned: () => api.post('/retention/cleanup-orphaned'),
}

// Remote Storage Types
export interface RemoteStorage {
  id: number
  name: string
  storage_type: 'local' | 'ssh' | 'webdav' | 's3' | 'ftp' | 'rclone'
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
  connected: boolean
}

export interface KomodoTestResult {
  success: boolean
  message: string
  version?: string
}

export const settingsApi = {
  getKomodo: () => api.get<KomodoSettings>('/settings/komodo'),
  updateKomodo: (data: { enabled: boolean; api_url?: string; api_key?: string }) =>
    api.put<KomodoSettings>('/settings/komodo', data),
  testKomodo: () => api.post<KomodoTestResult>('/settings/komodo/test'),
}

export default api
