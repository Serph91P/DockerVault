export function planId(raw?: string | null): number | null {
  if (!raw || !/^[1-9]\d*$/.test(raw)) return null
  const id = Number(raw)
  return Number.isSafeInteger(id) ? id : null
}
export function protectionSearch(params: URLSearchParams): string {
  const safe = new URLSearchParams()
  const q = params.get('q')?.slice(0, 255)
  if (q) safe.set('q', q)
  for (const [key, values] of Object.entries({
    type: ['container', 'stack', 'volume', 'path'],
    status: ['active', 'disabled', 'completed', 'failed', 'missing', 'unknown', 'never'],
    sort: ['name', 'type', 'last-backup', 'created'],
  })) {
    const value = params.get(key)
    if (value && values.includes(value)) safe.set(key, value)
  }
  const text = safe.toString()
  return text ? `?${text}` : ''
}
export function policySearch(params: URLSearchParams): string {
  const safe = new URLSearchParams()
  const q = params.get('q')?.slice(0, 255)
  if (q) safe.set('q', q)
  for (const [key, values] of Object.entries({ status: ['enabled', 'disabled', 'default'], usage: ['used', 'unused'] })) {
    const value = params.get(key)
    if (value && values.includes(value)) safe.set(key, value)
  }
  return safe.size ? `?${safe}` : ''
}
export function returnPath(raw?: string | null): string {
  if (raw) {
    const [path, search = ''] = raw.split('?')
    const policy = /^\/protection\/(schedules|retention)(?:\/([1-9]\d*)(?:\/edit)?)?$/.exec(path)
    if (policy && (!policy[2] || planId(policy[2]))) return `${path}${policySearch(new URLSearchParams(search))}`
  }
  if (!raw || !raw.startsWith('/protection') || (raw !== '/protection' && !raw.startsWith('/protection?')))
    return '/protection'
  return `/protection${protectionSearch(new URLSearchParams(raw.split('?')[1] || ''))}`
}
export const contextSearch = (back: string) => `?returnTo=${encodeURIComponent(returnPath(back))}`
