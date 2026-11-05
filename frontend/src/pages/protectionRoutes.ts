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
export function returnPath(raw?: string | null): string {
  if (!raw || !raw.startsWith('/protection') || (raw !== '/protection' && !raw.startsWith('/protection?')))
    return '/protection'
  return `/protection${protectionSearch(new URLSearchParams(raw.split('?')[1] || ''))}`
}
export const contextSearch = (back: string) => `?returnTo=${encodeURIComponent(returnPath(back))}`
