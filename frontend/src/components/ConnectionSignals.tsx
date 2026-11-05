import { useQuery } from '@tanstack/react-query'
import { settingsApi } from '../api'
import { queryKeys } from '../api/queryKeys'
import { responseStatus } from '../api/responses'

export default function ConnectionSignals() {
  const apiRead = useQuery({ queryKey: queryKeys.settings.storage, queryFn: () => settingsApi.getStorageSettings().then(r => r.data), retry: false })
  const status = responseStatus(apiRead.error)
  return <span className="text-xs text-dark-200" title="Measured using the authenticated storage-settings read; not Docker or provider health">API: {apiRead.isPending ? 'unknown' : apiRead.isError ? status ? `responded (${status}); data unavailable` : 'unavailable' : 'reachable'}{apiRead.isError && apiRead.dataUpdatedAt > 0 ? ' (previous data stale)' : ''}</span>
}
