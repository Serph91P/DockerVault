import { useState, useEffect, useRef } from 'react'
import { Link, Navigate, useLocation, useSearchParams, useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import api, { backupsApi, storageApi, type HistoryRecord } from '../api'
import { queryKeys } from '../api/queryKeys'
import { responseStatus } from '../api/responses'
import { operationOutcome } from '../api/outcomes'
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session'
import { QueryFeedback } from '../components/QueryFeedback'
import BackupBrowser from '../components/BackupBrowser'
import BackupLogViewer from '../components/BackupLogViewer'
import RestoreWizard from '../components/RestoreWizard'
import RemovalDialog from '../components/RemovalDialog'

const filterNames = ['q', 'status', 'source', 'target_id', 'after', 'before', 'location', 'sort', 'direction', 'limit']
const localLabels = { deleted: 'Local copy deleted (confirmed removal)', available: 'Local copy available', missing: 'Local file missing', inaccessible: 'Local copy inaccessible', not_recorded: 'Local copy not retained / no local path recorded' }
function CopyState({ record }: { record: HistoryRecord }) {
  return <div className="space-y-1 text-sm break-words">
    <p>{localLabels[record.local_state]}</p>
    {record.remote_sync_details?.map(copy => <p key={copy.storage_id}>{copy.storage_name}: {copy.availability === 'inaccessible' ? 'inaccessible (storage disabled or unavailable)' : copy.status === 'completed' ? 'synced (catalog confirmed; availability checked on access)' : copy.status === 'failed' ? 'failed sync' : copy.status === 'pending' || copy.status === 'syncing' ? copy.status : 'unknown'}</p>)}
    {!record.remote_sync_details?.length && <p>No remote copy recorded</p>}
    <p>{record.encrypted ? 'Encrypted archive — private key required for contents' : 'Unencrypted archive'}</p>
  </div>
}
function historySearch(params: URLSearchParams, view: string, restart = false) {
  const next = new URLSearchParams(params)
  if (restart) { next.delete('offset'); next.delete('snapshot') }
  next.set('view', view)
  return next.toString()
}
export default function History() {
  const location = useLocation()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const restoreMatch = location.pathname.match(/^\/history\/archives\/([1-9]\d*)\/restore$/)
  if (restoreMatch) return <RestoreWizard key={restoreMatch[1]} backupId={Number(restoreMatch[1])} inline onClose={() => navigate(`/history/${location.state?.restoreFrom === 'runs' ? 'runs' : 'archives'}/${restoreMatch[1]}${location.search}`, {state:{restoreReturnFocus:true}})} />
  const match = location.pathname.match(/^\/history\/(runs|archives)\/([^/]+)$/)
  const legacy = params.get('run') || params.get('archive')
  if (!match && legacy && /^[1-9]\d*$/.test(legacy)) {
    const next = new URLSearchParams(params); const kind = params.has('run') ? 'runs' : 'archives'; next.delete('run'); next.delete('archive')
    return <Navigate replace to={`/history/${kind}/${legacy}?${historySearch(next, kind)}`} />
  }
  if (!match && /^[1-9]\d*$/.test(params.get('plan') ?? '')) {
    const next = new URLSearchParams(params); next.set('target_id', next.get('plan')!); next.delete('plan')
    return <Navigate replace to={`/history?${next}`} />
  }
  if (!match && !['/history', '/history/'].includes(location.pathname)) return <section><h1>Page not found</h1><p>This address does not identify a run or archive.</p><Link to={`/history?${params}`}>Return to History with current filters</Link></section>
  return match ? <HistoryDetail key={`${match[1]}-${match[2]}`} view={match[1]} rawId={match[2]} /> : <HistoryList />
}
function HistoryList() {
  const [params, setParams] = useSearchParams()
  const locations = useQuery({ queryKey: queryKeys.storages.list(), queryFn: () => storageApi.list().then(r => r.data), retry: false })
  const view = params.get('view') === 'archives' ? 'archives' : 'runs'
  const request: Record<string, string | number> = { view, limit: 50, offset: 0 }
  for (const name of [...filterNames, 'offset', 'snapshot']) if (params.get(name)) request[name] = params.get(name)!
  const query = useQuery({ queryKey: queryKeys.backups.history(request), queryFn: ({ signal }) => backupsApi.history(request, signal).then(r => r.data), retry: false })
  const page = query.data
  const changePage = (offset: number) => { const next = new URLSearchParams(params); next.set('offset', String(offset)); if (page) next.set('snapshot', String(page.snapshot)); setParams(next) }
  const reset = () => { const next = new URLSearchParams(params); for (const key of [...filterNames, 'offset', 'snapshot']) next.delete(key); setParams(next) }
  return <section className="space-y-5 min-w-0">
    <h1 className="text-2xl font-semibold">History &amp; Restore</h1>
    <p>Runs are execution results. Archives are recorded recovery artifacts, not a guarantee that their copies remain accessible.</p>
    <nav aria-label="History views" className="flex gap-4">{['runs', 'archives'].map(kind => <Link className="underline" key={kind} aria-current={view === kind ? 'page' : undefined} to={`/history?${historySearch(params, kind, true)}`}>{kind === 'runs' ? 'Runs' : 'Archives'}</Link>)}</nav>
    <form key={params.toString()} className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4" onSubmit={event => {
      event.preventDefault(); const values = new FormData(event.currentTarget); const next = new URLSearchParams(params)
      for (const key of filterNames) { const value = String(values.get(key) ?? '').trim(); if (value) next.set(key, value); else next.delete(key) }
      next.delete('snapshot'); next.delete('offset'); setParams(next)
    }}>
      <label>Search history<input className="input w-full" name="q" defaultValue={params.get('q') ?? ''} maxLength={200} /></label>
      <label>Run status<select className="input w-full" name="status" defaultValue={params.get('status') ?? ''}><option value="">All outcomes</option>{['pending', 'running', 'completed', 'failed', 'cancelled'].map(s => <option key={s}>{s}</option>)}</select></label>
      <label>Source type<select className="input w-full" name="source" defaultValue={params.get('source') ?? ''}><option value="">All sources</option>{['container', 'stack', 'volume', 'path'].map(s => <option key={s}>{s}</option>)}</select></label>
      <label>Recorded location<select className="input w-full" name="location" defaultValue={params.get('location') ?? ''}><option value="">All locations</option><option value="local">Local path recorded</option><option value="remote">Any remote record</option>{locations.data?.map(storage => <option key={storage.id} value={storage.id}>{storage.name}{storage.enabled ? '' : ' (disabled)'}</option>)}{/^[1-9]\d*$/.test(params.get('location') ?? '') && !locations.data?.some(storage => String(storage.id) === params.get('location')) && <option value={params.get('location')!}>Remote #{params.get('location')} (inventory unavailable)</option>}</select></label>
      <label>Created after (UTC)<input className="input w-full" type="datetime-local" name="after" defaultValue={params.get('after')?.replace(/Z$/, '') ?? ''} /></label>
      <label>Created before (UTC)<input className="input w-full" type="datetime-local" name="before" defaultValue={params.get('before')?.replace(/Z$/, '') ?? ''} /></label>
      <label>Sort by<select className="input w-full" name="sort" defaultValue={params.get('sort') ?? 'created'}><option value="created">Created time</option><option value="name">Source name</option><option value="size">Archive size</option></select></label>
      <label>Sort direction<select className="input w-full" name="direction" defaultValue={params.get('direction') ?? 'desc'}><option value="desc">Descending</option><option value="asc">Ascending</option></select></label>
      <label>Plan ID<input className="input w-full" name="target_id" type="number" min="1" defaultValue={params.get('target_id') ?? ''} /></label>
      <label>Results per page<select className="input w-full" name="limit" defaultValue={params.get('limit') ?? '50'}>{[25, 50, 100].map(n => <option key={n}>{n}</option>)}</select></label>
      <button className="btn-primary self-end" type="submit">Apply filters</button><button className="btn-secondary self-end" type="button" onClick={reset}>Reset filters</button>
    </form>
    <p aria-label="Applied filters">Server filters: {filterNames.filter(key => params.has(key)).map(key => `${key}=${params.get(key)}`).join('; ') || 'All catalog records'}</p>
    <QueryFeedback name="History" query={query} />
    {locations.isError && <QueryFeedback name="History locations" query={locations} />}
    {params.has('snapshot') && <button className="btn-secondary" onClick={() => { const next = new URLSearchParams(params); next.delete('snapshot'); next.delete('offset'); setParams(next) }}>Include new runs / restart pagination</button>}
    {page && <>
      <p className="font-semibold" aria-live="polite">{page.total} matching {view}</p><p className="text-sm">{page.dataset}</p>
      {page.items.length === 0 ? <p>{page.total === 0 ? `No matching ${view}` : 'This page is now empty. Return to the first page or refresh the snapshot.'}</p> : <div className="overflow-x-auto"><table aria-label={view === 'runs' ? 'Runs' : 'Archives'} className="w-full text-left text-sm">
        <thead className="max-sm:sr-only"><tr><th scope="col">Identity / source</th><th scope="col">Outcome / time</th><th scope="col">Archive / copies</th></tr></thead>
        <tbody>{page.items.map(record => <tr key={record.id} className="border-t border-dark-600 max-sm:flex max-sm:flex-col max-sm:py-3">
          <td className="p-3 max-sm:p-1 align-top"><Link className="underline font-semibold" to={`/history/${view}/${record.id}?${historySearch(params, view)}`}>{view === 'runs' ? 'Run' : 'Archive'} #{record.id}</Link><p>{record.target_name ?? `Plan #${record.target_id}`} · {record.source_type}</p>{record.plan_removed && <p>Removed plan — history retained</p>}</td>
          <td className="p-3 max-sm:p-1 align-top"><p>Outcome: {operationOutcome(record)}</p><time dateTime={record.created_at}>{new Date(record.created_at).toLocaleString()}</time><p>Duration: {record.duration_seconds == null ? 'Unknown' : `${record.duration_seconds}s`}</p></td>
          <td className="p-3 max-sm:p-1 align-top">{record.archive_state === 'never_created' ? <p>Archive never created</p> : <><p>{record.file_size_human ?? 'Size unknown'}</p><CopyState record={record} /></>}</td>
        </tr>)}</tbody>
      </table></div>}
      <div className="flex flex-wrap items-center gap-3"><button className="btn-secondary" aria-label="Previous page" disabled={query.isFetching || page.offset === 0} onClick={() => changePage(Math.max(0, page.offset - page.limit))}>Previous</button><span>{page.items.length ? page.offset + 1 : 0}–{page.offset + page.items.length} of {page.total}</span><button className="btn-secondary" aria-label="Next page" disabled={query.isFetching || page.offset + page.limit >= page.total} onClick={() => changePage(page.offset + page.limit)}>Next</button></div>
    </>}
  </section>
}
function HistoryDetail({ view, rawId }: { view: string; rawId: string }) {
  const id = /^[1-9]\d*$/.test(rawId) ? Number(rawId) : NaN
  const [params] = useSearchParams()
  const label = `${view === 'runs' ? 'Run' : 'Archive'} #${rawId}`
  const client = useQueryClient()
  const query = useQuery({ queryKey: queryKeys.backups.historyDetail(id), queryFn: ({ signal }) => backupsApi.historyDetail(id, signal).then(r => r.data), enabled: Number.isSafeInteger(id), retry: false })
  const record = query.data
  const location = useLocation()
  const restoreLink = useRef<HTMLAnchorElement>(null)
  const returnedFocus = useRef(false)
  useEffect(() => {
    if (!location.state?.restoreReturnFocus || returnedFocus.current) return
    // The shell focuses main after child effects on route changes.
    const frame = requestAnimationFrame(() => {
      if (restoreLink.current) { restoreLink.current.focus(); returnedFocus.current = true }
    })
    return () => cancelAnimationFrame(frame)
  }, [location.key, location.state, record])
  const [files, setFiles] = useState(view === 'archives')
  const [logs, setLogs] = useState(view === 'runs')
  const [removal, setRemoval] = useState<'archive' | 'plan' | null>(null)
  const [pending, setPending] = useState(false)
  const [result, setResult] = useState('')
  const [downloadError, setDownloadError] = useState('')
  const [downloadSidecar, setDownloadSidecar] = useState(false)
  const download = async (sidecar = false) => {
    const owner = sessionGeneration(), signal = sessionSignal()
    setPending(true); setDownloadError(''); setDownloadSidecar(sidecar)
    try {
      const response = await api.get(`/backups/${id}/download`, { params: { sidecar }, responseType: 'blob', signal })
      if (!ownsSession(owner) || signal.aborted) return
      const header = String(response.headers['content-disposition'] ?? '')
      const encodedName = /filename\*=UTF-8''([^;]+)/i.exec(header)?.[1]
      const filename = encodedName ? decodeURIComponent(encodedName) : /filename="([^"]+)"/i.exec(header)?.[1]
      const url = URL.createObjectURL(response.data); const link = document.createElement('a'); link.href = url; link.download = filename || `archive-${id}${sidecar ? '.key' : record?.encrypted ? '.enc' : '.tar'}`; link.click(); URL.revokeObjectURL(url)
    } catch { if (ownsSession(owner) && !signal.aborted) setDownloadError('Archive download failed. Retry download or refresh copy state.') }
    finally { if (ownsSession(owner)) setPending(false) }
  }
  const sync = async () => {
    const owner = sessionGeneration(); setPending(true); setResult('')
    try {
      const response = await backupsApi.retrySync(id)
      if (!ownsSession(owner)) return
      const data = response.data
      setResult(data && Number.isSafeInteger(data.succeeded) && Number.isSafeInteger(data.failed) ? `Sync result: ${data.succeeded} succeeded; ${data.failed} failed. Inspect refreshed copies.` : 'Sync result unknown. Refresh before retrying.')
      await client.invalidateQueries({ queryKey: queryKeys.backups.all })
    } catch { if (ownsSession(owner)) setResult('Sync result not confirmed. Refresh copy state before retrying.') }
    finally { if (ownsSession(owner)) setPending(false) }
  }
  const absent = !Number.isSafeInteger(id) || responseStatus(query.error) === 404 || (view === 'archives' && record?.archive_state !== 'recorded' && query.isSuccess)
  return <section aria-label={`${label} context`} role={absent || query.isError ? 'alert' : undefined} className="space-y-4 min-w-0">
    <Link className="underline" to={`/history?${historySearch(params, view)}`}>Back to {view}</Link>
    <h1 className="text-2xl font-semibold">{label}</h1>
    <QueryFeedback name={label} query={query} />
    {absent && <div role="status"><h2>{view === 'archives' ? 'Archive' : 'Run'} not found</h2><p>This identifier has no recorded {view === 'archives' ? 'archive' : 'run'}. Your filters are preserved; return to the list or retry.</p></div>}
    {record && !absent && <>
      <h2 className="text-lg font-semibold">{record.target_name ?? `Plan #${record.target_id}`}</h2>
      <p>{record.plan_removed ? 'Removed plan — retained history' : 'Protection plan'} · {record.source_type} · <Link className="underline" to={`/protection/${record.target_id}`}>Open plan</Link></p>
      <p className="break-all">Source identity: {record.source_identity ?? 'Unknown'}</p>
      <p>Outcome: {operationOutcome(record)}; run status: {record.status}; stage: {record.stage ?? 'Unknown / not recorded'}</p>
      <dl className="grid grid-cols-1 sm:grid-cols-2 gap-2"><div><dt>Started</dt><dd>{record.started_at ?? 'Not recorded'}</dd></div><div><dt>Completed</dt><dd>{record.completed_at ?? 'Not final / not recorded'}</dd></div><div><dt>Duration</dt><dd>{record.duration_seconds == null ? 'Unknown' : `${record.duration_seconds}s`}</dd></div><div><dt>Archive size</dt><dd>{record.file_size_human ?? 'Unknown'}</dd></div></dl>
      {record.error_message && <p role="note" className="break-words">Run error: {record.error_message}</p>}
      <p>{record.archive_state === 'never_created' ? 'Archive never created' : record.archive_state === 'unknown' ? 'Archive identity unknown' : `Archive #${record.id} recorded`}</p>
      <p>{record.retention_context}</p><CopyState record={record} />
      {record.encrypted && record.can_restore && <p>Whole-archive export stays encrypted. Keep its encryption sidecar with the archive and your separately held private key. <button className="btn-secondary" disabled={pending} onClick={() => void download(true)}>Download encryption sidecar</button></p>}
      {record.archive_state === 'recorded' && view === 'runs' && <Link className="underline" to={`/history/archives/${id}?${historySearch(params, 'archives')}`}>Open archive #{id}</Link>}
      {record.archive_state === 'recorded' && <Link className="underline" to={`/history/runs/${id}?${historySearch(params, 'runs')}`}>Open originating run #{id}</Link>}
      <div className="flex flex-wrap gap-2">
        {record.can_restore && <><button className="btn-secondary" onClick={() => setFiles(!files)}>Browse archive files</button>{!pending && !query.isError && <Link ref={restoreLink} className="btn-secondary" to={`/history/archives/${id}/restore?${params}`} state={{restoreFrom:view}}>Restore archive</Link>}<button className="btn-secondary" disabled={pending} onClick={() => void download()}>Download whole archive</button></>}
        {record.can_sync && <button className="btn-secondary" disabled={pending || query.isError} onClick={() => void sync()}>{record.remote_sync_details?.some(copy => copy.status === 'failed') ? 'Retry failed sync / sync new destinations' : 'Sync to new remote destinations'}</button>}
        <button className="btn-secondary" onClick={() => setLogs(!logs)}>{logs ? 'Hide run logs' : 'Show run logs'}</button>
        {record.archive_state === 'recorded' && <button className="btn-secondary" disabled={pending || query.isError} onClick={() => setRemoval('archive')}>Review copy removal</button>}
        <button className="btn-secondary" disabled={pending || query.isError} onClick={() => setRemoval('plan')}>Review archive copy deletion for this plan</button>
      </div>
      <p>Plan-wide copy deletion reviews all archives for Plan #{record.target_id}, independently of History filters or pagination. The plan and unselected copies are retained.</p>
      {record.local_storage_path && <Link className="underline block" to={`/storage?storage=local&path=${encodeURIComponent(record.local_storage_path.split('/').slice(0, -1).join('/'))}&returnTo=${encodeURIComponent(`/history/${view}/${id}?${historySearch(params, view)}`)}`}>Open local storage at {record.local_storage_path}</Link>}
      {record.remote_sync_details?.map(copy => <Link key={copy.storage_id} className="underline block break-all" to={`/storage?storage=${copy.storage_id}&path=${encodeURIComponent(copy.remote_path?.split('/').slice(0, -1).join('/') ?? '')}&returnTo=${encodeURIComponent(`/history/${view}/${id}?${historySearch(params, view)}`)}`}>Open storage {copy.storage_name} at {copy.remote_path ?? 'unknown path'}</Link>)}
      {result && <p role="status">{result}</p>}
      {downloadError && <div role="alert"><p>{downloadError}</p><button className="btn-secondary" disabled={pending} onClick={() => void download(downloadSidecar)}>Retry download</button></div>}
      {files && record.can_restore && <BackupBrowser key={id} backup={record} inline onClose={() => setFiles(false)} />}
      {logs && <BackupLogViewer backupId={id} inline onClose={() => setLogs(false)} />}
      {removal && <RemovalDialog intent={removal === 'plan' ? { kind: 'copies', targetId: record.target_id, name: record.target_name ?? `Plan #${record.target_id}` } : { kind: 'copies', backupId: id, name: `Archive #${id}` }} onClose={() => setRemoval(null)} onChanged={() => { void client.invalidateQueries({ queryKey: queryKeys.backups.all }) }} />}
    </>}
  </section>
}
