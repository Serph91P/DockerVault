import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button } from './ui/button'
import { recoveryApi, recoveryTables, type RecoveryCounts, type RecoveryResult } from '../api/recovery'
import { ownsSession, sessionGeneration } from '../store/session'
import { useAuthStore } from '../store/auth'

export function RecoveryCountsView({ counts }: { counts: RecoveryCounts }) {
  return <dl className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-4 gap-y-2">{(Object.keys(recoveryTables) as (keyof RecoveryCounts)[]).map(key => <div className="contents" key={key}><dt>{recoveryTables[key]}</dt><dd>{counts[key]}</dd></div>)}</dl>
}
const labels = { connected: 'Connected (read observation)', offline: 'Offline', credentials_required: 'Credentials required', not_tested: 'Not tested', disabled: 'Disabled', available: 'Available (presence observed)', unavailable: 'Unavailable' }
export function RecoveryResultView({ result }: { result: RecoveryResult }) {
  const [page, setPage] = useState(0)
  return <div className="space-y-4">
    <h3 className="text-lg font-semibold">Backend connection and catalog observations</h3>
    <p>These are import-time observations, not current health, write-permission or archive-decryption guarantees. Missing credentials and inaccessible copies require attention. No archive or sidecar was modified, adopted, synchronized or deleted.</p>
    <RecoveryCountsView counts={result.counts} />
    <ul className="space-y-2">{result.storages.map(storage => <li key={storage.id}>Storage {storage.id} ({storage.type}): {labels[storage.state]}</li>)}</ul>
    <p>Komodo: {labels[result.komodo.state]}</p>
    {result.ssh_public_keys.length > 0 && <section aria-label="SSH public reconnect references" className="space-y-3"><h4 className="font-semibold">SSH public reconnect references</h4><p>Public references only: private keys were not restored. Supply your original private keys through the permitted deployment SSH paths. Generating a replacement key does not recover existing server authorization.</p>{result.ssh_public_keys.map(key => <div className="min-w-0 space-y-2" key={key.name}><h5 className="font-semibold">{key.name}</h5><pre aria-label={`SSH public reference ${key.name}`} tabIndex={0} className="max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-dark-900 p-3 text-sm text-dark-100">{key.public_key}</pre></div>)}</section>}
    <h4 className="font-semibold">Catalog copy observations ({result.catalog.length})</h4>
    <ul className="space-y-2">{result.catalog.slice(page * 50, (page + 1) * 50).map(entry => <li key={entry.id}>Archive {entry.id}: local — {labels[entry.local]}{entry.copies.map(copy => <span className="block pl-3" key={copy.storage_id}>Storage {copy.storage_id}: {labels[copy.state]}</span>)}</li>)}</ul>
    {result.catalog.length > 50 && <div className="flex gap-2"><Button variant="outline" disabled={page === 0} onClick={() => setPage(value => value - 1)}>Previous observations</Button><Button variant="outline" disabled={(page + 1) * 50 >= result.catalog.length} onClick={() => setPage(value => value + 1)}>Next observations</Button></div>}
  </div>
}
export const recoveryInput = 'w-full min-w-0 rounded-lg border border-dark-600 bg-dark-900 px-3 py-3 text-dark-100 focus:outline-none focus:ring-2 focus:ring-primary-400'
export function SystemRecovery() {
  const admin = useAuthStore(state => state.user?.is_admin === true)
  const [password, setPassword] = useState(''), [confirmation, setConfirmation] = useState('')
  const [pending, setPending] = useState(false), [error, setError] = useState(''), [notice, setNotice] = useState('')
  const [result, setResult] = useState<RecoveryResult | null>(null), [statusError, setStatusError] = useState(false)
  const owner = useRef(sessionGeneration()).current, live = useRef(true), busy = useRef(false)
  const controller = useRef<AbortController | null>(null)
  useEffect(() => {
    live.current = true
    const abort = new AbortController()
    controller.current = abort
    if (admin) void recoveryApi.status(abort.signal).then(value => { if (live.current && ownsSession(owner)) setResult(value) }).catch(() => { if (live.current && ownsSession(owner)) setStatusError(true) })
    return () => { live.current = false; abort.abort() }
  }, [admin, owner])
  const download = async (event: React.FormEvent) => {
    event.preventDefault()
    if (busy.current) return
    setNotice(''); setError('')
    if (Array.from(password).length < 8 || new TextEncoder().encode(password).length > 1024 || password !== confirmation) { setError('Use at least 8 characters and confirm the same dedicated system-backup password.'); return }
    busy.current = true; setPending(true)
    try {
      const response = await recoveryApi.export(password, confirmation, controller.current!.signal)
      if (!live.current || !ownsSession(owner)) return
      const url = URL.createObjectURL(response.data)
      try { const link = document.createElement('a'); link.href = url; link.download = 'dockervault-system-backup.dockervault'; link.click() }
      finally { URL.revokeObjectURL(url) }
      setNotice('Download initiated. Verify that the .dockervault file was saved securely; this page cannot confirm file custody.')
    } catch { if (live.current && ownsSession(owner)) setError('System-backup export could not be confirmed. No usable download is reported. Check the connection and retry.') }
    finally { busy.current = false; if (live.current && ownsSession(owner)) { setPassword(''); setConfirmation(''); setPending(false) } }
  }
  return <div className="space-y-4">
    <p>The encrypted .dockervault bundle contains portable configuration, catalog and included provider credentials. Restore it only before creating an administrator on a fresh installation.</p>
    <p className="rounded-lg border border-amber-700 p-3 text-amber-200">Archives and volume data are NOT included. Private SSH keys and external archive age private identities are NOT included. Keep these separately. Losing the bundle password makes the system backup unrecoverable; neither an account password nor an archive private key resets it.</p>
    {admin ? <form onSubmit={download} className="space-y-3">
      <label className="block space-y-2"><span>System-backup password</span><input className={recoveryInput} type="password" autoComplete="new-password" minLength={8} maxLength={1024} required disabled={pending} value={password} onChange={event => setPassword(event.target.value)} /></label>
      <label className="block space-y-2"><span>Confirm system-backup password</span><input className={recoveryInput} type="password" autoComplete="new-password" minLength={8} maxLength={1024} required disabled={pending} value={confirmation} onChange={event => setConfirmation(event.target.value)} /></label>
      <p>Passwords remain only in this open view and the submitted request, never in URLs or browser storage. Leaving this section clears them.</p>
      {error && <p role="alert" className="text-red-300">{error}</p>}
      {notice && <p role="status">{notice}</p>}
      <Button type="submit" disabled={pending}>{pending ? 'Encrypting system backup…' : 'Download encrypted system backup'}</Button>
    </form> : <p>Only an authenticated administrator can export system backups.</p>}
    <p>Archive encryption keys are separate from account and bundle passwords. Manage existing archives in <Link className="underline text-primary-300" to="/history">History & Restore</Link>.</p>
    {statusError && <p role="alert">Previous recovery observations could not be loaded. No successful recovery is inferred.</p>}
    {result && <section aria-label="Previous system recovery" className="space-y-4"><h3 className="text-xl font-semibold">Previous system recovery</h3><RecoveryResultView result={result} /></section>}
  </div>
}
