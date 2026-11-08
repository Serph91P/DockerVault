import { Link, useSearchParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import api, { type RemoteStorage } from '../api'
import StorageBrowser from './StorageBrowser'
import { QueryFeedback } from './QueryFeedback'

function safeStorageReturn(value: string | null): string | null {
  if (!value || value.includes('\\') || [...value].some(char => char.charCodeAt(0) < 32)) return null
  return /^\/(history(?:\/archives\/[1-9]\d*|\/runs\/[1-9]\d*)?|backups|protection)(?:\?[^#]*)?$/.test(value) ? value : null
}

export default function StorageLocationContext({ storages, onEdit, onTest, onRemove }: { storages: RemoteStorage[] | undefined; onEdit?: (storage: RemoteStorage) => void; onTest?: (id: number) => void; onRemove?: (storage: RemoteStorage) => void }) {
  const [params, setParams] = useSearchParams()
  const raw = params.get('storage'), returnTo = safeStorageReturn(params.get('returnTo'))
  const back = returnTo ? <Link className="underline" to={returnTo}>Return to archive context</Link> : null
  const path = params.get('path') ?? '', tab = params.get('tab') ?? 'files'
  const safe = !path.startsWith('/') && !path.includes('\\') && ![...path].some(char => char.charCodeAt(0) < 32) && !path.split('/').some(part => part === '..' || part === '.')
  const storage = raw === 'local' ? { id: 'local' as const, name: 'Local Backups', storage_type: 'local', base_path: '' } : storages?.find(item => String(item.id) === raw)
  const system = useQuery({ queryKey: ['system-info'], queryFn: async () => (await api.get('/settings/system-info')).data, enabled: raw === 'local' && tab === 'usage' })
  if (!raw) return back
  const change = (key: string, value: string) => { const next = new URLSearchParams(params); if (value) next.set(key, value); else next.delete(key); setParams(next) }
  const close = () => { const next = new URLSearchParams(params); ['storage','path','tab','q','kind','sort','direction','operation'].forEach(key => next.delete(key)); setParams(next) }
  const tabHref = (value: string) => { const next = new URLSearchParams(params); next.set('tab',value); return `/storage?${next}` }
  return <section aria-label="Archive storage context" className="space-y-3 min-w-0">
    {back}<button className="btn-secondary" onClick={close}>All locations</button>
    <h2 className="text-xl font-semibold break-words">{storage?.name ?? `Storage ${raw}`}</h2>
    <p>Storage {raw}, relative path: {path || '/'}.</p>
    <nav aria-label="Location contexts" className="flex flex-wrap gap-4">{['files','connection','usage'].map(value => <Link key={value} aria-current={tab === value ? 'page' : undefined} className="underline capitalize" to={tabHref(value)}>{value[0].toUpperCase() + value.slice(1)}</Link>)}</nav>
    {!safe ? <p role="alert">This archive path cannot be opened as a relative storage location. Use the storage root.</p> : !storage ? <p role="status">{storages ? 'Storage location not found or unavailable.' : 'Loading storage location…'}</p> : !['files','connection','usage'].includes(tab) ? <p role="alert">Unknown storage context.</p> : <>
      <div hidden={tab !== 'files'}><StorageBrowser storage={storage} initialPath={path} inline onPathChange={value => change('path',value)} filter={params.get('q') ?? ''} kind={params.get('kind') ?? 'all'} sort={params.get('sort') ?? 'name'} direction={params.get('direction') ?? 'asc'} onViewChange={change} onClose={close} /></div>
      {tab === 'connection' && <div className="space-y-3"><p>Provider: {storage.storage_type}. Enabled is a configuration flag, not a health check.</p>{storage.id === 'local' ? <p>System-Local uses the server-configured local backup directory. It has no editable location record.</p> : <><p>Connection tests are optional observations, not continuous health monitoring. Local tests may create the directory and perform an exclusive write probe.</p><button className="btn-secondary" onClick={() => onEdit?.(storage as RemoteStorage)}>Edit connection</button><button className="btn-secondary" onClick={() => onTest?.(storage.id as number)}>Test saved connection</button><button className="btn-danger" onClick={() => onRemove?.(storage as RemoteStorage)}>Remove configuration (not files)</button></>}<Link className="underline block" to={`/settings?storageReturn=${encodeURIComponent(`/storage?${params}`)}#ssh-keys`}>Manage SSH keys in Settings / Security</Link><p>Credentials are never kept for navigation. Re-enter sensitive draft fields when returning.</p></div>}
      {tab === 'usage' && <div>{storage.id === 'local' ? <><h3>Filesystem capacity of the configured local backup directory</h3><QueryFeedback name="Filesystem capacity" query={system} />{!system.isError && system.data?.disk_status === 'available' ? <dl><dt>Total bytes</dt><dd>{system.data.disk_total}</dd><dt>Used bytes (whole filesystem)</dt><dd>{system.data.disk_used}</dd><dt>Free bytes</dt><dd>{system.data.disk_free}</dd></dl> : <p>Filesystem capacity unavailable; fallback numbers are not measurements.</p>}</> : <p>Location quota / capacity unavailable: this provider does not supply a measured usage contract. Directory sizes in Files are separate observations.</p>}</div>}
    </>}
  </section>
}
