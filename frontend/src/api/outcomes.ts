export type Outcome = 'accepted' | 'running' | 'completed' | 'partial' | 'failed' | 'cancelled' | 'unknown'
export function operationOutcome(data: unknown): Outcome {
  if (!data || typeof data !== 'object' || Array.isArray(data)) return 'unknown'
  const result = data as Record<string, unknown>
  const status = result.status ?? result.state
  if (result.success === false || status === 'failed') return 'failed'
  if (['accepted', 'pending', 'queued'].includes(String(status))) return 'accepted'
  if (['running', 'cancelling'].includes(String(status))) return 'running'
  if (status === 'cancelled') return 'cancelled'
  if (status !== undefined && !['completed', 'restored', 'partial', 'incomplete', 'interrupted'].includes(String(status))) return 'unknown'
  const counters = ['succeeded', 'failed', 'total'] as const
  if (counters.some(key => result[key] !== undefined)) {
    if (!counters.every(key => Number.isSafeInteger(result[key]) && Number(result[key]) >= 0)) return 'unknown'
    if (['skipped', 'unknown'].some(key => result[key] !== undefined && (!Number.isSafeInteger(result[key]) || Number(result[key]) < 0))) return 'unknown'
    if (Number(result.succeeded) + Number(result.failed) + Number(result.skipped ?? 0) + Number(result.unknown ?? 0) !== result.total) return 'unknown'
  }
  const items = result.items ?? result.remote_sync_details
  if (items !== undefined && (!Array.isArray(items) || items.some(item => !item || typeof item !== 'object'))) return 'unknown'
  const itemStatus = (item: Record<string, unknown>) => item.outcome ?? item.status ?? item.state
  if (Array.isArray(items) && items.some(item => !(result.remote_sync_details !== undefined ? ['completed', 'failed', 'pending'] : ['succeeded', 'completed', 'failed', 'skipped', 'unknown', 'pending', 'running', 'retained']).includes(String(itemStatus(item))))) return 'unknown'
  const warnings = result.warnings
  if (warnings !== undefined && !Array.isArray(warnings)) return 'unknown'
  const partial = (Array.isArray(warnings) && warnings.length > 0) || Boolean(result.warning) || ['partial', 'incomplete', 'interrupted'].includes(String(status)) || Number(result.failed) > 0 || Number(result.skipped) > 0 || Number(result.unknown) > 0 || (Array.isArray(items) && items.some(item => ['failed', 'skipped', 'unknown', 'pending', 'running', 'retained'].includes(String(itemStatus(item)))))
  if (partial) return 'partial'
  if (result.success === true || ['completed', 'restored'].includes(String(status))) return 'completed'
  return 'unknown'
}
