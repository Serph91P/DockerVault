import { useEffect, useRef, useState } from 'react'
import api, { type RestoreInfo, type RestoreSelection, type RestoreImpact } from '../api'
import { responseStatus } from '../api/responses'
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session'
import { Modal } from './ui/dialog'
import { Button } from './ui/button'

const tracking = 'Restore has no durable progress tracking. Leaving or closing this view does not cancel the backend. After reload, reconnect or a lost response the outcome is unknown: inspect the destination and run logs before explicitly retrying. Never replay automatically.'
function validInfo(v: RestoreInfo, id: number): boolean {
  return !!v?.backup && v.backup.id === id && typeof v.backup.encrypted === 'boolean' && typeof v.backup.local_available === 'boolean' && typeof v.backup.created_at === 'string' && !!v.target && typeof v.target.name === 'string' && typeof v.original_supported === 'boolean' && ['available','unavailable'].includes(v.volume_discovery) && Array.isArray(v.available_volumes) && v.available_volumes.every(n=>typeof n==='string') && Array.isArray(v.remote_copies) && v.remote_copies.every(c=>!!c && Number.isSafeInteger(c.storage_id) && typeof c.storage_name==='string' && typeof c.status==='string')
}
function validImpact(value: unknown): value is RestoreImpact {
  const v = value as RestoreImpact | null
  return !!v && typeof v.fingerprint === 'string' && !!v.destination && ['original','volume','path'].includes(v.destination.type) && typeof v.destination.path === 'string' && (v.destination.volume === null || typeof v.destination.volume === 'string') && typeof v.layout === 'string' && typeof v.overwrite === 'string' && typeof v.key_required === 'boolean' && [v.containers_to_stop,v.containers_to_restart].every(a => Array.isArray(a) && a.every(n => typeof n === 'string'))
}
function Effects({ impact }: { impact: RestoreImpact }) {
  return <dl className="space-y-3 break-words">
    <div><dt className="font-semibold">Confirmed destination</dt><dd>{impact.destination.type}: {impact.destination.volume && `${impact.destination.volume} — `}{impact.destination.path}</dd></div>
    <div><dt className="font-semibold">Overwrite and recovery limits</dt><dd>{impact.overwrite}</dd></div>
    <div><dt className="font-semibold">Archive layout</dt><dd>{impact.layout}</dd></div>
    <div><dt className="font-semibold">Containers to stop</dt><dd>{impact.containers_to_stop.join(', ') || 'None'}</dd></div>
    <div><dt className="font-semibold">Containers to restart (order)</dt><dd>{impact.containers_to_restart.join(', ') || 'None'}</dd></div>
    <div><dt className="font-semibold">Recovery key</dt><dd>{impact.key_required ? 'Required; decryption authenticates before workload stop' : 'Not required'}</dd></div>
  </dl>
}
export default function RestoreWizard({ backupId, onClose, inline = false }: { backupId: number; onClose: () => void; inline?: boolean }) {
  const [info, setInfo] = useState<RestoreInfo | null>(null)
  const [selection, setSelection] = useState<RestoreSelection>({destination_type:'original'})
  const [privateKey, setPrivateKey] = useState('')
  const [impact, setImpact] = useState<RestoreImpact | null>(null)
  const [result, setResult] = useState<RestoreImpact | null>(null)
  const [step, setStep] = useState<'source'|'destination'|'review'|'pending'|'result'>('source')
  const [error, setError] = useState('')
  const [acknowledged, setAcknowledged] = useState(false)
  const [busy, setBusy] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [revoked, setRevoked] = useState(false)
  const lock = useRef(false)
  const requests = useRef(new AbortController())
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    const controller = new AbortController()
    const session = sessionSignal()
    const owner = sessionGeneration()
    const signal = AbortSignal.any([controller.signal,session])
    requests.current = controller
    const revoke = () => { setPrivateKey(''); setImpact(null); setResult(null); setInfo(null); setRevoked(true) }
    session.addEventListener('abort', revoke, {once:true})
    void api.get<RestoreInfo>(`/backups/${backupId}/restore-info`, {signal}).then(r => {
      if (!signal.aborted && ownsSession(owner)) {
        if (!validInfo(r.data, backupId)) throw new Error('Unavailable info')
        setInfo(r.data); setError(''); setSelection({destination_type:r.data.original_supported ? 'original':'path'})
      }
    }).catch(e => { if (!signal.aborted) setError(responseStatus(e) === 404 ? 'Archive not found or no longer available.' : responseStatus(e) === 403 ? 'Access denied to this archive.' : 'Restore source information unavailable. Retry loading.') })
    return () => { controller.abort(); session.removeEventListener('abort',revoke) }
  }, [backupId,attempt])
  useEffect(() => { heading.current?.focus() }, [step, result, error])
  const close = () => { requests.current.abort(); setPrivateKey(''); setImpact(null); setResult(null); onClose() }
  const ownerRequest = () => ({ owner: sessionGeneration(), signal: AbortSignal.any([requests.current.signal,sessionSignal()]) })
  const review = async () => {
    if (lock.current) return
    lock.current = true; setBusy(true); setError(''); setImpact(null); setAcknowledged(false)
    const {owner,signal} = ownerRequest()
    try {
      const r = await api.post(`/backups/${backupId}/restore-review`, selection, {signal})
      if (!signal.aborted && ownsSession(owner)) {
        if (!validImpact(r.data)) throw new Error('Invalid review')
        setImpact(r.data); setStep('review')
      }
    } catch(e) { if (!signal.aborted && ownsSession(owner)) { setPrivateKey(''); setError(responseStatus(e) === 400 ? 'Destination rejected or unsupported. Correct the path or volume and review again.' : 'Destination or configured workload discovery unavailable. Retry review; nothing has been restored.') } }
    finally { lock.current = false; if (!signal.aborted && ownsSession(owner)) setBusy(false) }
  }
  const restore = async () => {
    if (lock.current || !impact || !acknowledged) return
    lock.current = true; setBusy(true); setError(''); setStep('pending')
    const {owner,signal} = ownerRequest()
    try {
      const r = await api.post(`/backups/${backupId}/restore`, {...selection,review_fingerprint:impact.fingerprint,...(info?.backup.encrypted ? {private_key:privateKey}: {})}, {signal})
      if (!signal.aborted && ownsSession(owner)) {
        if (r.data.status !== 'restored' || !validImpact(r.data) || r.data.fingerprint !== impact.fingerprint || JSON.stringify(r.data.destination) !== JSON.stringify(impact.destination) || ('warnings' in r.data && (!Array.isArray(r.data.warnings) || r.data.warnings.length > 0))) throw new Error('Final result not confirmed')
        setResult(r.data); setStep('result')
      }
    } catch(e) {
      if (!signal.aborted && ownsSession(owner)) {
        const status = responseStatus(e)
        setError(status === 409 ? 'Restore impact changed. No destination change was accepted. Return and review again.' : status === 400 ? 'Restore not confirmed: recovery key missing, incorrect or sidecar unavailable, or destination rejected. Inspect logs and destination before retrying.' : status === 404 ? 'Archive copy unavailable locally or remotely. Check storage availability before retrying.' : 'Restore result unknown / not confirmed. The backend may have written files or failed during restart. Inspect the destination and run logs before retrying.')
        setImpact(null); setAcknowledged(false); setStep('destination')
      }
    } finally {
      lock.current = false
      if (!signal.aborted && ownsSession(owner)) { setPrivateKey(''); setBusy(false) }
    }
  }
  const content = <section className="space-y-5 min-w-0 p-4 sm:p-6 max-w-3xl" aria-busy={busy}>
    <div className="flex flex-wrap justify-between gap-3"><h1 className="text-2xl font-semibold">Restore Backup</h1><Button variant="outline" onClick={close} aria-label="Close restore">{step==='pending' ? 'Leave view (not cancel)' : 'Close restore'}</Button></div>
    <p className="text-sm text-dark-300">{tracking}</p>
    <h2 ref={heading} tabIndex={-1} className="text-xl font-semibold">{revoked ? 'Session changed' : step==='source' ? 'Source and availability' : step==='destination' ? 'Destination and effects' : step==='review' ? 'Review restore impact' : step==='pending' ? 'Restoring — awaiting final backend result' : 'Restore completed'}</h2>
    {error && <p role="alert">{error}</p>}
    {revoked ? <p>Private restore view cleared. Open the archive in your current session.</p> : !info ? <>{!error && <p role="status">Loading restore information…</p>}{error && <Button onClick={()=>{setError('');setAttempt(a=>a+1)}}>Retry loading</Button>}</> : <>
      <p>Archive #{info.backup.id} — {info.target.name} — {info.backup.completed_at || info.backup.created_at} — {info.backup.file_size_human || 'Size unknown'}</p>
      {step==='source' && <>
        <p>{info.backup.local_available ? 'Local archive available (observed now).' : 'Local copy missing. Completed remote copies will be materialized on confirmation.'}</p>
        {info.remote_copies.map(copy=><p key={copy.storage_id}>{copy.storage_name}: {copy.status} — provider access and encrypted sidecar availability checked only during restore.</p>)}
        {!info.backup.local_available && !info.remote_copies.some(c=>c.status==='completed') && <p role="alert">No usable copy is recorded. Restore cannot proceed; recover an archive copy first.</p>}
        <p>{info.backup.encrypted ? 'Encrypted archive: private recovery key required. It stays only in this open view.' : 'Unencrypted archive; no key needed.'}</p>
        <Button disabled={!info.backup.local_available && !info.remote_copies.some(c=>c.status==='completed')} onClick={()=>setStep('destination')}>Next</Button>
      </>}
      {step==='destination' && <fieldset disabled={busy} className="space-y-4">
        <legend>Destination</legend>
        <label className="block"><input type="radio" name="destination" checked={selection.destination_type==='original'} disabled={!info.original_supported} onChange={()=>{setSelection({destination_type:'original'});setImpact(null)}} /> Original location</label>
        {!info.original_supported && <p>Original container/stack mapping is unsupported. Use an explicit custom path or volume; archive roots are preserved.</p>}
        <label className="block"><input type="radio" name="destination" checked={selection.destination_type==='path'} onChange={()=>{setSelection({destination_type:'path',target_path:''});setImpact(null)}} /> Custom path</label>
        {selection.destination_type==='path' && <label className="block">Destination path<input className="input w-full" value={selection.target_path || ''} onChange={e=>setSelection({destination_type:'path',target_path:e.target.value})} /></label>}
        <label className="block"><input type="radio" name="destination" disabled={info.volume_discovery!=='available' || !info.available_volumes.length} checked={selection.destination_type==='volume'} onChange={()=>{setSelection({destination_type:'volume',target_volume:''});setImpact(null)}} /> Different volume</label>
        {info.volume_discovery!=='available' && <p>Volume discovery unavailable, not a confirmed empty inventory. Retry loading to discover volumes.</p>}
        {info.volume_discovery==='available' && !info.available_volumes.length && <p>No alternative volumes found.</p>}
        {selection.destination_type==='volume' && <label className="block">Destination volume<select aria-label="Destination volume" className="input w-full" value={selection.target_volume || ''} onChange={e=>setSelection({destination_type:'volume',target_volume:e.target.value})}><option value="">Select volume…</option>{info.available_volumes.map(v=><option key={v}>{v}</option>)}</select></label>}
      </fieldset>}
      {step==='destination' && <>
        {info.backup.encrypted && <label className="block">Private recovery key<textarea className="input w-full" placeholder="AGE-SECRET-KEY-1..." autoComplete="off" spellCheck={false} value={privateKey} onChange={e=>setPrivateKey(e.target.value)} /></label>}
        <p>Review validates destination identity and configured workload state, not archive contents, key correctness or remote provider access. Only configured, originally-running workloads are stopped/restarted, not every volume user.</p>
        <div className="flex flex-wrap gap-3"><Button variant="outline" disabled={busy} onClick={()=>{setStep('source');setPrivateKey('')}}>Back</Button><Button disabled={busy || (selection.destination_type==='path' && !selection.target_path?.trim()) || (selection.destination_type==='volume' && !selection.target_volume) || (info.backup.encrypted && !privateKey.trim())} onClick={()=>void review()}>{busy ? 'Loading review…':'Review effects'}</Button><Button variant="outline" disabled={busy} onClick={()=>{setPrivateKey('');setAttempt(a=>a+1)}}>Reload source and volumes</Button></div>
      </>}
      {step==='review' && impact && <><Effects impact={impact}/><label className="block"><input type="checkbox" checked={acknowledged} onChange={e=>setAcknowledged(e.target.checked)} /> I reviewed the destination, overwrite and lifecycle effects and explicitly authorize this restore.</label><div className="flex flex-wrap gap-3"><Button variant="outline" onClick={()=>{setImpact(null);setAcknowledged(false);setStep('destination')}}>Back</Button><Button disabled={!acknowledged || busy} onKeyDown={e=>{if(e.key==='Enter')e.preventDefault()}} onClick={()=>void restore()}>Confirm restore</Button></div><p className="text-sm">Keyboard: activate Confirm restore with Space after checking the confirmation. Enter never submits this workspace.</p></>}
      {step==='pending' && <p role="status">Restore is in progress; waiting for extraction and workload recovery. No progress percentage or cancellation is available.</p>}
      {step==='result' && result && <><p role="status">Backend confirmed restoration and configured workload recovery. Verify destination file bytes and application health separately.</p><Effects impact={result}/></>}
    </>}
  </section>
  return inline ? content : <Modal title="Restore Backup" onClose={close}>{content}</Modal>
}
