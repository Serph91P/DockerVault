import { operationActive, type StorageOperation } from '../api/operations'

export default function StorageOperationResults({ operations, stale, pending, error, onCancel, onRetry, onRefresh, onFolder }: {
  operations: StorageOperation[]; stale: boolean; pending: boolean; error: boolean
  onCancel: (id: string) => void; onRetry: (id: string) => void; onRefresh: () => void; onFolder: (path: string) => void
}) {
  return <section aria-label="Storage operations" className="border-t border-dark-700 p-3 space-y-3 max-h-64 overflow-y-auto shrink-0">
    <div className="flex items-center justify-between gap-2 flex-wrap">
      <h3 className="font-medium">Storage operations</h3>
      <button type="button" className="btn-secondary" onClick={onRefresh}>Refresh operations</button>
    </div>
    {stale && <p role="alert">Operation status unavailable. Displayed results are stale; completion is not confirmed. Retry refresh.</p>}
    {error && <p role="alert">Operation request failed or its outcome is unknown. Refresh to inspect server state before another action.</p>}
    {operations.map(operation => <article key={operation.id} aria-label={`Operation ${operation.id}`} className="border border-dark-700 rounded p-2 space-y-2 break-words">
      <p className="text-xs break-all">{operation.id}</p>
      <p role="status">{stale ? 'Stale snapshot: ' : ''}{operation.state} - {operation.completed}/{operation.total} entries</p>
      <p>{operation.succeeded} succeeded, {operation.failed} failed, {operation.skipped} skipped, {operation.unknown} unknown</p>
      <progress aria-label="Completed entries" value={operation.completed} max={operation.total} className="w-full" />
      {operationActive(operation) && <><progress aria-label="Current deletion (indeterminate)" className="w-full" /><p className="text-sm">Recursive work is indeterminate; counts measure completed outer entries only.</p></>}
      {operation.cancel_requested && <p role="status">Cancellation requested; wait for current deletion to settle. This is not rollback.</p>}
      {operation.unknown > 0 && <p role="alert">Interrupted effects are unknown. Inspect actual files before taking further action; unknown items are not retried.</p>}
      <div className="flex flex-wrap gap-2">
        <button className="btn-secondary" onClick={() => onFolder(operation.path)}>Open operation folder</button>
        {operationActive(operation) && <button className="btn-secondary" disabled={pending || stale || operation.cancel_requested} onClick={() => onCancel(operation.id)}>Cancel operation</button>}
        {!operationActive(operation) && operation.failed > 0 && !operations.some(child => child.parent_id === operation.id) && <button className="btn-secondary" disabled={pending || stale} onClick={() => onRetry(operation.id)}>Retry failed items</button>}
      </div>
      <details open={operation.failed > 0 || operation.unknown > 0}><summary>Per-item results</summary><ul>{operation.items.map((item, index) => <li key={index} className="text-sm break-all">{item.path}: {item.state}{item.error && <> - <span>{item.error}</span></>}</li>)}</ul></details>
    </article>)}
    {!operations.length && !stale && <p className="text-sm text-dark-400">No recent storage operations</p>}
  </section>
}
