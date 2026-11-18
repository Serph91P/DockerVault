// Metadata may carry legacy URL credentials. Keep them out of shared consumers.
export function storageEndpoint(value: string): string {
  try {
    const url = new URL(value)
    return `${url.protocol}//${url.host}${url.pathname}`
  } catch { return value.split(/[?#]/,1)[0].split('@').pop() ?? '' }
}
export function protectStorageMetadata(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(protectStorageMetadata)
  if (!value || typeof value !== 'object') return value
  const row = {...value} as Record<string,unknown>
  for (const key of ['webdav_url','s3_endpoint_url','host']) if (typeof row[key] === 'string') row[key] = storageEndpoint(row[key])
  return row
}
