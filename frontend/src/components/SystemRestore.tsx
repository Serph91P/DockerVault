import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Button } from './ui/button'
import { RecoveryCountsView, RecoveryResultView } from './SystemRecovery'
import { recoveryApi, type RecoveryPreview, type RecoveryResult } from '../api/recovery'
import { ownsSession, sessionGeneration } from '../store/session'
import { useAuthStore } from '../store/auth'

const input = 'w-full min-w-0 rounded-lg border border-dark-600 bg-dark-900 px-3 py-3 text-dark-100 focus:outline-none focus:ring-2 focus:ring-primary-400'
const action = 'h-auto min-h-11 whitespace-normal text-left'
export default function SystemRestore({ back }: { back: () => void }) {
  const [bundle, setBundle] = useState(''), [password, setPassword] = useState('')
  const [username, setUsername] = useState(''), [adminPassword, setAdminPassword] = useState(''), [confirmation, setConfirmation] = useState('')
  const [preview, setPreview] = useState<RecoveryPreview | null>(null), [result, setResult] = useState<RecoveryResult | null>(null)
  const [pending, setPending] = useState<'reading' | 'preview' | 'restore' | null>(null), [error, setError] = useState('')
  const busy = useRef(false), revision = useRef(0), live = useRef(true), owner = useRef(sessionGeneration()).current
  const controller = useRef<AbortController | null>(null), heading = useRef<HTMLHeadingElement>(null)
  const navigate = useNavigate()
  useEffect(() => { live.current = true; const abort = new AbortController(); controller.current = abort; return () => { live.current = false; abort.abort() } }, [])
  useEffect(() => { if (preview || result) heading.current?.focus() }, [preview, result])
  const active = () => live.current && ownsSession(owner)
  const read = (file?: File) => {
    const version = ++revision.current
    setBundle(''); setPassword(''); setPreview(null); setError('')
    if (!file) return
    if (!file.size || file.size > 32 * 1024 * 1024) { setError('Choose a nonempty .dockervault bundle no larger than 32 MiB.'); return }
    setPending('reading')
    const reader = new FileReader()
    reader.onload = () => { if (active() && version === revision.current) { setBundle(String(reader.result)); setPending(null) } }
    reader.onerror = () => { if (active() && version === revision.current) { setError('Unable to read the selected file.'); setPending(null) } }
    reader.readAsText(file)
  }
  const validate = async (event: React.FormEvent) => {
    event.preventDefault()
    if (busy.current || !bundle) return
    busy.current = true; setPending('preview'); setError('')
    try {
      const value = await recoveryApi.preview(bundle, password, controller.current!.signal)
      if (active()) setPreview(value)
    } catch { if (active()) { setPassword(''); setError('Bundle validation failed. Check the dedicated password, file integrity and compatibility. No configuration was activated.') } }
    finally { busy.current = false; if (active()) setPending(null) }
  }
  const restore = async (event: React.FormEvent) => {
    event.preventDefault()
    if (busy.current || !preview) return
    if (username.trim().length < 3 || Array.from(adminPassword).length < 8 || adminPassword !== confirmation) { setError('Use at least 3 username characters and 8 account-password characters; both new account passwords must match.'); return }
    busy.current = true; setPending('restore'); setError('')
    try {
      const value = await recoveryApi.restore(bundle, password, username.trim(), adminPassword, confirmation, controller.current!.signal)
      if (active()) { setResult(value); setBundle(''); setPreview(null) }
    } catch { if (active()) setError('Activation was not confirmed. Do not assume success or retry blindly. Check setup status: a committed account must sign in; an uncommitted setup may be retried from the bundle.') }
    finally { busy.current = false; if (active()) { setPassword(''); setAdminPassword(''); setConfirmation(''); setPending(null); if (!result) setPreview(null) } }
  }
  return <section aria-label="Restore system backup" className="space-y-4 rounded-xl border border-dark-700 bg-dark-800 p-4 sm:p-6">
    <h2 ref={heading} tabIndex={-1} className="text-xl font-semibold">{result ? 'Configuration restored' : preview ? 'Validated preview — not restored' : 'Restore system backup'}</h2>
    <p>Only for a fresh installation. Archives, volume data, private SSH keys and external archive age private keys are not included. Mount existing archive locations separately; keep your original archive private identities. A bundle password cannot decrypt age archives.</p>
    {error && <div role="alert" className="space-y-2 text-red-300"><p>{error}</p><Button variant="outline" onClick={() => void useAuthStore.getState().checkAuthStatus()}>Check setup status</Button></div>}
    {pending && <p role="status">{pending === 'reading' ? 'Reading encrypted bundle locally…' : pending === 'preview' ? 'Decrypting and validating in isolation. No live configuration is being activated…' : 'Waiting for backend validation, staging, read-only connection/catalog observations and atomic activation. No completion percentage or cancellation is reported.'}</p>}
    {result ? <>
      <p role="status">The backend confirmed the durable configuration, catalog and new administrator session. Connection failures below are not a successful provider recovery.</p>
      <RecoveryResultView result={result} />
      <Button className={action} onClick={() => { navigate('/settings#system-recovery', { replace: true }); void useAuthStore.getState().checkAuthStatus() }}>Continue to DockerVault</Button>
    </> : preview ? <>
      <p>Exported: <time dateTime={preview.exported_at}>{new Date(preview.exported_at).toLocaleString()}</time>. Compatibility, references and paths validated in an isolated staging database. This is not an import or connection test.</p>
      <RecoveryCountsView counts={preview.counts} />
      <ul>{preview.storages.map(storage => <li key={storage.id}>Storage {storage.id} ({storage.type}): {storage.state === 'credentials_required' ? 'Credentials required' : 'Not tested'}</li>)}</ul>
      <form className="space-y-3" onSubmit={restore}>
        <fieldset className="space-y-3" disabled={pending !== null}><legend className="font-semibold">Create a new administrator as part of activation</legend>
          <label className="block space-y-2"><span>New administrator username</span><input className={input} autoComplete="username" minLength={3} maxLength={50} required value={username} onChange={event => setUsername(event.target.value)} /></label>
          <label className="block space-y-2"><span>New account password</span><input className={input} type="password" autoComplete="new-password" minLength={8} maxLength={1024} required value={adminPassword} onChange={event => setAdminPassword(event.target.value)} /></label>
          <label className="block space-y-2"><span>Confirm new account password</span><input className={input} type="password" autoComplete="new-password" minLength={8} maxLength={1024} required value={confirmation} onChange={event => setConfirmation(event.target.value)} /></label>
          <p>Old accounts and sessions are not imported. The account password is separate from the bundle password and archive private keys. Activation also restores configured automation; review restored schedules and retention policies.</p>
          <Button className={action} type="submit">Restore configuration and create administrator</Button>
          <Button variant="outline" onClick={() => { setPreview(null); setPassword(''); setAdminPassword(''); setConfirmation('') }}>Back to bundle</Button>
        </fieldset>
      </form>
    </> : <form className="space-y-3" onSubmit={validate}>
      <label className="block space-y-2"><span>System backup file</span><input className={input} type="file" accept=".dockervault" disabled={pending !== null} onChange={event => read(event.target.files?.[0])} /></label>
      <label className="block space-y-2"><span>System-backup password</span><input className={input} type="password" autoComplete="off" maxLength={1024} required disabled={pending !== null} value={password} onChange={event => setPassword(event.target.value)} /></label>
      <p>The password stays only in this open view and submitted requests. It is never saved in browser storage or URLs. Losing it makes the bundle unrecoverable.</p>
      <Button className={action} type="submit" disabled={!bundle || pending !== null}>Validate and preview</Button>
      <Button className={action} variant="outline" onClick={back} disabled={pending !== null}>Back to setup choice</Button>
    </form>}
  </section>
}
