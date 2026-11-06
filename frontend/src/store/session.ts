import { QueryClient } from '@tanstack/react-query'
import { CanceledError, type InternalAxiosRequestConfig } from 'axios'
import toast from 'react-hot-toast'
import api from '../api'
import { protectTargetResponse } from '../api/targetPrivacy'
import { safeRequestError } from '../api/responses'
import { resetNotificationOwner } from './notifications'
import { cleanupWebSocket } from './websocket'
import { clearPlanDrafts } from '../components/BackupWizard/planEditorDraft'

export const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 10000, refetchOnWindowFocus: false } },
})

let generation = 0
let controller = new AbortController()
let expire: () => void = () => undefined
const owners = new WeakMap<InternalAxiosRequestConfig, number>()

export const sessionGeneration = () => generation
export const sessionSignal = () => controller.signal
export const ownsSession = (owner: number) => owner === generation
export function onSessionExpired(handler: () => void) { expire = handler }

export function clearSession(preserveDraftFor?: number, preserveNotificationOwner?: number) {
  ++generation
  controller.abort()
  controller = new AbortController()
  cleanupWebSocket()
  void queryClient.cancelQueries()
  // clear() removes records, not pending mutation callbacks. Revoke those first.
  for (const mutation of queryClient.getMutationCache().getAll()) {
    mutation.setOptions({ ...mutation.options, mutationFn: async () => { throw new CanceledError('Session changed') }, onSuccess: undefined, onError: undefined, onSettled: undefined })
  }
  queryClient.clear()
  resetNotificationOwner(preserveNotificationOwner)
  toast.remove()
  try {
    clearPlanDrafts(preserveDraftFor)
    let preserveDraft = false
    try { preserveDraft = Boolean(preserveDraftFor && JSON.parse(localStorage.getItem('dockervault-wizard-draft') ?? 'null')?.ownerId === preserveDraftFor) } catch { /* Discard malformed legacy drafts. */ }
    if (!preserveDraft) localStorage.removeItem('dockervault-wizard-draft')
    localStorage.removeItem('dockervault_notifications')
    localStorage.removeItem('storage-test-results')
  } catch { /* Storage may be unavailable. */ }
}

// Reuse the default cookie client; also cover legacy callers without a query signal.
api.interceptors.request.use((config) => {
  owners.set(config, generation)
  config.signal = config.signal
    ? AbortSignal.any([config.signal as AbortSignal, controller.signal])
    : controller.signal
  return config
}, undefined, { synchronous: true })
api.interceptors.response.use((response) => {
  if (owners.get(response.config) !== generation) throw new CanceledError('Session changed')
  return protectTargetResponse(response)
}, (error) => {
  if (error.config && owners.get(error.config) !== generation) {
    return Promise.reject(new CanceledError('Session changed'))
  }
  if (error.response?.status === 401 && !error.config?.url?.startsWith('/auth/')) expire()
  // Validation errors can echo command bodies; never expose their Axios context.
  return Promise.reject(safeRequestError(error))
})
