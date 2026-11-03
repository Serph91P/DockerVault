import type { AxiosRequestConfig, AxiosResponse, InternalAxiosRequestConfig } from 'axios'

export interface PrivateTargetConfig extends AxiosRequestConfig { privateTargetHooks?: boolean }
export function isTargetRequest(config?: AxiosRequestConfig) {
  return /^\/targets(?:\/|$)/.test(config?.url ?? '')
}

// Omitted is deliberately different from null: redaction must never clear stored hooks.
function withoutHooks(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(withoutHooks)
  if (!value || typeof value !== 'object') return value
  return Object.fromEntries(Object.entries(value).filter(([key]) => key !== 'pre_backup_command' && key !== 'post_backup_command').map(([key, entry]) => [key, withoutHooks(entry)]))
}

export function protectTargetResponse(response: AxiosResponse) {
  if (!isTargetRequest(response.config)) return response
  const config = response.config as InternalAxiosRequestConfig & PrivateTargetConfig
  if (!config.privateTargetHooks) response.data = withoutHooks(response.data)
  // Axios retains serialized request payloads even after successful requests.
  config.data = undefined
  // XHR retains the unredacted responseText even when response.data is safe.
  response.request = undefined
  return response
}
