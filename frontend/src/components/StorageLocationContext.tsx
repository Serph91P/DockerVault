import { Link, useSearchParams } from 'react-router-dom'
import type { RemoteStorage } from '../api'
import StorageBrowser from './StorageBrowser'

export default function StorageLocationContext({ storages }: { storages: RemoteStorage[] | undefined }) {
  const [params, setParams] = useSearchParams()
  const raw = params.get('storage')
  const returnTo = params.get('returnTo')
  const back = returnTo && /^\/history(?:\/|\?|$)/.test(returnTo) ? <Link className="underline" to={returnTo}>Return to archive context</Link> : null
  if (!raw || params.has('operation')) return back
  const path = params.get('path') ?? ''
  const safe = !path.startsWith('/') && !path.includes('\\') && !path.split('/').some(part => part === '..' || part === '.')
  const storage = raw === 'local' ? { id: 'local' as const, name: 'Local Backups', storage_type: 'local', base_path: '' } : storages?.find(item => String(item.id) === raw)
  const close = () => { const next = new URLSearchParams(params); next.delete('storage'); next.delete('path'); setParams(next) }
  return <section aria-label="Archive storage context" className="space-y-3">
    {back}
    <p>Storage {raw}, relative path: {path || '/'}.</p>
    {!safe ? <p role="alert">This archive path cannot be opened as a relative storage location. Use the storage root.</p> : storage ? <StorageBrowser storage={storage} initialPath={path} onClose={close} /> : <p role="status">{storages ? 'Storage location not found or unavailable.' : 'Loading storage location…'}</p>}
  </section>
}
