import { useQuery } from '@tanstack/react-query'
import { useContext, useEffect, useState, useSyncExternalStore } from 'react'
import { Link, UNSAFE_DataRouterContext, useBlocker, useLocation } from 'react-router-dom'
import { dockerApi, settingsApi } from '../api'
import { queryKeys } from '../api/queryKeys'
import { QueryFeedback } from '../components/QueryFeedback'
import EncryptionSetup from '../components/EncryptionSetup'
import SSHKeyManager from '../components/SSHKeyManager'
import ConfirmDialog from '../components/ConfirmDialog'
import { sessionGeneration, sessionSignal } from '../store/session'
import { KomodoSettingsCard, StorageSettingsCard } from '../components/SettingsEditors'

const sections = [
  ['system', 'System'], ['connection', 'Connections'], ['encryption', 'Encryption & SSH keys'],
  ['storage-safeguards', 'Storage safeguards'], ['system-recovery', 'System recovery'],
] as const
const card = 'space-y-4 rounded-xl border border-dark-700 bg-dark-800 p-4 sm:p-6 min-w-0 break-words'
const subscribeSession = (listener: () => void) => { const signal = sessionSignal(); signal.addEventListener('abort', listener); return () => signal.removeEventListener('abort', listener) }

function NavigationGuard({ dirty, discard }: { dirty: boolean; discard: () => void }) {
  const blocker = useBlocker(dirty)
  return <ConfirmDialog isOpen={blocker.state === 'blocked'} title="Discard unsaved settings?" message="Unsaved settings and private credential inputs will be discarded. Cancel to keep editing. An already submitted write may still complete on the server; leaving does not roll it back." confirmLabel="Discard and leave" onConfirm={() => { discard(); blocker.proceed?.() }} onClose={() => blocker.reset?.()} />
}

function bytes(value: number) { return `${(value / 1024 ** 3).toFixed(2)} GiB` }
function SystemInfoCard() {
  const query = useQuery({ queryKey: queryKeys.systemInfo, queryFn: () => settingsApi.getSystemInfo().then(r => r.data), refetchInterval: 30000 })
  const info = query.data
  return <div className={card}>
    <h2 className="text-lg font-semibold">System Information</h2>
    <QueryFeedback name="System information" query={query} />
    <p>Runtime values are read-only and controlled by the deployment environment. Docker socket, backup paths, concurrency and timeouts cannot be edited here.</p>
    {info && <>
      <h3 className="font-semibold">Disk Usage</h3>
      {info.disk_status === 'available' ? <><p>{bytes(info.disk_used)} used of {bytes(info.disk_total)} · {bytes(info.disk_free)} free</p><meter className="w-full" aria-label="Filesystem bytes used" min={0} max={Math.max(1, info.disk_total)} value={info.disk_used} /></> : <p>Disk capacity unavailable; fallback numbers are not measured usage.</p>}
      <dl className="grid gap-2 sm:grid-cols-2">
        <dt>Backups</dt><dd>{info.backup_count}</dd><dt>Targets</dt><dd>{info.target_count}</dd>
        <dt>Uptime</dt><dd>{Math.floor(info.uptime_seconds / 60)} minutes</dd><dt>Version</dt><dd>{info.app_version}</dd>
        <dt>Backup Directory</dt><dd>{info.backup_dir}</dd><dt>Database</dt><dd>{info.database_path} ({bytes(info.db_size)})</dd><dt>Timezone</dt><dd>{info.timezone}</dd>
      </dl>
    </>}
    <p>CPU and RAM metrics are not supplied by this settings API. No guessed measurements are shown.</p>
    <p>Configure reusable <Link className="underline text-primary-300" to="/protection/retention">Retention</Link> policies in Protection.</p>
  </div>
}

export default function Settings() {
  const generation = useSyncExternalStore(subscribeSession, sessionGeneration)
  return <SettingsWorkspace key={generation} />
}
function SettingsWorkspace() {
  const location = useLocation()
  const dataRouter = useContext(UNSAFE_DataRouterContext)
  const hash = location.hash.slice(1)
  const current = hash === 'ssh-keys' ? 'encryption' : sections.some(([id]) => id === hash) ? hash : 'system'
  const [dirty, setDirty] = useState({ komodo: false, storage: false })
  const [discardRevision, setDiscardRevision] = useState(0)
  const hasDraft = dirty.komodo || dirty.storage
  const discard = () => { setDiscardRevision(r => r + 1); setDirty({ komodo: false, storage: false }) }
  const candidate = new URLSearchParams(location.search).get('storageReturn')
  let storageReturn: string | null = null
  if (candidate?.startsWith('/storage?')) {
    const source = new URLSearchParams(candidate.slice('/storage?'.length)), safe = new URLSearchParams()
    for (const key of ['storage', 'tab', 'path', 'q', 'kind', 'sort', 'direction', 'operation', 'returnTo']) if (source.has(key)) safe.set(key, source.get(key)!)
    storageReturn = `/storage?${safe}`
  }
  useEffect(() => { document.getElementById(hash === 'ssh-keys' ? hash : current)?.focus() }, [current, hash])
  useEffect(() => {
    if (!hasDraft) return
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = '' }
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [hasDraft])
  const docker = useQuery({ queryKey: ['docker-health'], queryFn: () => dockerApi.getHealth().then(r => r.data.status === 'healthy'), refetchInterval: 30000 })
  return <div className="space-y-6">
    <header><h1 className="text-2xl font-bold">Settings</h1><p className="text-dark-300">Saved effective values, private drafts and separate test observations.</p></header>
    <nav aria-label="Settings sections" className="flex flex-wrap gap-2">
      {sections.map(([id, label]) => <Link key={id} to={{ pathname: location.pathname, search: location.search, hash: `#${id}` }} aria-current={current === id ? 'location' : undefined} className={`min-h-11 rounded-lg border px-3 py-2 ${current === id ? 'border-primary-400 text-primary-200' : 'border-dark-600 text-dark-200'}`}>{label}</Link>)}
    </nav>
    {dataRouter && <NavigationGuard dirty={hasDraft} discard={discard} />}
    <section id="system" tabIndex={-1} aria-label="System settings" hidden={current !== 'system'}><SystemInfoCard /></section>
    <section id="connection" tabIndex={-1} aria-label="Docker connection settings" hidden={current !== 'connection'} className="space-y-4">
      <div className={card}><h2 className="text-lg font-semibold">Docker Connection</h2><QueryFeedback name="Docker" query={docker} />
        <p>{docker.data === undefined ? 'Unknown' : docker.isError ? 'Previous Docker observation is stale; current connection unknown.' : docker.data ? 'Connected' : 'Not connected'}</p>
        <p>Socket path is environment-controlled; this API does not expose its configured value. A health observation does not prove discovery results or lifecycle authority.</p>
      </div>
      <KomodoSettingsCard discardRevision={discardRevision} onDirty={value => setDirty(prev => prev.komodo === value ? prev : { ...prev, komodo: value })} />
    </section>
    <div hidden={current !== 'encryption'} className="space-y-4">
      <section id="encryption" tabIndex={-1} aria-label="Encryption settings"><EncryptionSetup /></section>
      <section id="ssh-keys" tabIndex={-1} aria-label="SSH key security" className={card}><h2 className="text-lg font-semibold">Security / SSH keys</h2>
        {storageReturn && <Link className="underline text-primary-300" to={storageReturn}>Return to storage connection</Link>}
        <SSHKeyManager />
      </section>
    </div>
    <section id="storage-safeguards" tabIndex={-1} aria-label="Storage safeguards" hidden={current !== 'storage-safeguards'}><StorageSettingsCard discardRevision={discardRevision} onDirty={value => setDirty(prev => prev.storage === value ? prev : { ...prev, storage: value })} /></section>
    <section id="system-recovery" tabIndex={-1} aria-label="System recovery" hidden={current !== 'system-recovery'} className={card}><h2 className="text-lg font-semibold">System recovery</h2><p>System export, import and bootstrap require backend dependency #143, which is not implemented in this version. No recovery-bundle actions are available.</p><p>Archive encryption keys are not login passwords or system-recovery bundle passwords. Existing archive restore remains available in <Link className="underline text-primary-300" to="/history">History & Restore</Link>.</p></section>
  </div>
}
