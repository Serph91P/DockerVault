import api from './index'

export type StorageId = number | 'local'
export interface StorageOperation {
  id: string
  action: 'storage_delete'
  storage_id: StorageId
  path: string
  state: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled'
  stage: string
  total: number
  completed: number
  succeeded: number
  failed: number
  skipped: number
  unknown: number
  cancel_requested: boolean
  created_at: string
  updated_at: string
  finished_at: string | null
  parent_id: string | null
  items: { path: string; state: 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped' | 'unknown'; error: string | null }[]
}
export interface CreateStorageOperation {
  action: 'storage_delete'
  storage_id: StorageId
  path: string
  paths: string[]
  request_id: string
}
export const operationsApi = {
  create: (request: CreateStorageOperation) => api.post<{ operation_id: string }>('/operations', request),
  list: (storage_id: StorageId, signal?: AbortSignal) => api.get<StorageOperation[]>('/operations', { params: { storage_id, limit: 100 }, signal }),
  get: (id: string, signal?: AbortSignal) => api.get<StorageOperation>(`/operations/${id}`, { signal }),
  cancel: (id: string) => api.post<StorageOperation>(`/operations/${id}/cancel`),
  retry: (id: string, request_id: string) => api.post<{ operation_id: string }>(`/operations/${id}/retry`, { request_id }),
}
export const operationActive = (operation: StorageOperation) => operation.state === 'queued' || operation.state === 'running'
