import { QueryState } from './ui/workspace'
import { Button } from './ui/button'

type QueryStatus = {
  isPending: boolean; isError: boolean; isFetching: boolean; error: unknown;
  dataUpdatedAt: number; refetch: () => unknown
}
export function QueryFeedback({ name, query, stale }: { name: string; query: QueryStatus; stale?: boolean }) {
  const staleData = stale ?? (query.isError && query.dataUpdatedAt > 0)
  const denied = (query.error as { status?: number; response?: { status?: number } } | null)?.status === 403 || (query.error as { response?: { status?: number } } | null)?.response?.status === 403
  return <div aria-label={`${name} status`} role={query.isError ? 'alert' : undefined}>
    {query.isPending ? <QueryState state="loading" title={`Loading ${name}`} /> : query.isError ?
      <QueryState state="error" announce={false} title={`${name}: ${staleData ? 'stale data' : denied ? 'access denied' : 'unavailable'}`} description={staleData ? 'Refresh failed. Previously confirmed data remains visible.' : denied ? 'You do not have access to this resource.' : 'The response could not be confirmed. Retry without leaving this page.'} /> : null}
    {query.dataUpdatedAt > 0 && <p className="text-xs text-dark-300">{staleData ? 'Last successful refresh' : `${name} updated`}: <time dateTime={new Date(query.dataUpdatedAt).toISOString()}>{new Date(query.dataUpdatedAt).toLocaleString()}</time></p>}
    <Button variant="outline" focusableWhenDisabled disabled={query.isFetching} onClick={() => { if (!query.isFetching) void query.refetch() }}>{query.isError ? 'Retry' : 'Refresh'} {name}</Button>
  </div>
}
