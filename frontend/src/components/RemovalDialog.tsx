import { useEffect, useId, useRef, useState } from 'react'
import { backupsApi, targetsApi, RemovalPreview, RemovalResult, RemovalScope } from '../api'

export type RemovalIntent = { kind: 'plan' | 'copies'; name: string; targetId?: number; backupId?: number }

export default function RemovalDialog({ intent, onClose, onChanged }: { intent: RemovalIntent; onClose: () => void; onChanged: () => void }) {
  const [scope, setScope] = useState<RemovalScope>('all')
  const [preview, setPreview] = useState<RemovalPreview | null>(null)
  const [result, setResult] = useState<RemovalResult | null>(null)
  const [planRemoved, setPlanRemoved] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const pending = useRef(false)
  const panel = useRef<HTMLDivElement>(null)
  const titleId = useId()
  const plan = intent.kind === 'plan'

  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null
    panel.current?.querySelector<HTMLButtonElement>('button')?.focus()
    return () => { if (trigger?.isConnected) trigger.focus() }
  }, [])

  const run = async (action: () => Promise<void>) => {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try { await action() } catch {
      setError('The server could not confirm this operation. No success is assumed. Review again or retry the same confirmed scope.')
    } finally { pending.current = false; setBusy(false) }
  }
  const review = () => run(async () => {
    const response = await backupsApi.previewRemoval({ scope: plan ? 'all' : scope, ...(intent.backupId ? { backup_ids: [intent.backupId] } : { target_id: intent.targetId }) })
    setPreview(response.data)
  })
  const execute = (token?: string) => run(async () => {
    if (plan) {
      const response = await targetsApi.delete(intent.targetId!)
      if (response.data.copies !== 'retained') throw new Error('Unconfirmed plan removal')
      setPlanRemoved(true)
    } else {
      if (!token) return
      const response = await backupsApi.executeRemoval(token)
      setResult(response.data)
    }
    onChanged()
  })

  return <div className="fixed inset-0 z-[60] bg-black/60 flex items-center justify-center p-4">
    <div ref={panel} role="dialog" aria-modal="true" aria-labelledby={titleId} className="bg-dark-800 border border-dark-700 rounded-xl p-6 w-full max-w-2xl max-h-[90dvh] overflow-y-auto" onKeyDown={(event) => {
      if (event.key === 'Escape' && !pending.current) { event.stopPropagation(); onClose() }
      if (event.key === 'Tab') {
        const controls = panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)')
        if (!controls?.length) return
        const first = controls[0], last = controls[controls.length - 1]
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus() }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
      }
    }}>
      <h2 id={titleId} className="text-xl font-semibold">{plan ? 'Remove protection plan' : 'Delete archive copies'}</h2>
      <p className="my-3">{intent.name}{intent.targetId ? ` · Plan #${intent.targetId}` : ` · Archive #${intent.backupId}`}</p>
      <p>{plan ? 'Stop future automation and retention. All existing archives, keys, logs and local/remote recovery copies will be kept. Restore and browse remain available in retained history.' : 'Permanent deletion cannot be undone. Only the reviewed copies will be removed; later archives are never included.'}</p>
      {!preview && !plan && <fieldset disabled={busy} className="my-4 space-y-2"><legend>Copy scope</legend>
        {([['local', 'Local copies only'], ['remote', 'Remote copies only'], ['all', 'All selected local and remote copies']] as const).map(([value, label]) => <label key={value} className="block"><input type="radio" name="copy-scope" checked={scope === value} onChange={() => setScope(value)} /> {label}</label>)}
      </fieldset>}
      {preview && !planRemoved && !result && <section className="my-4" aria-label="Confirmed scope">
        <p>{preview.archive_count} archives · {preview.copy_count} selected copies · {plan ? 'all retained' : `${preview.scope} scope`}</p>
        <p>{plan ? 'No archive or copy will be deleted.' : `${preview.retained.length} unselected copies retained. Required local keys are kept while remote copies remain.`}</p>
        <ul className="max-h-48 overflow-auto">{preview.items.map((item, index) => <li key={index}>{item.target_name} · Archive #{item.backup_id} · {item.copy} · {item.label} — {plan ? 'retained' : 'selected for permanent removal'}</li>)}</ul>
      </section>}
      <div role="status" aria-live="polite">
        {planRemoved && <p>Protection plan removed. Existing recovery points retained.</p>}
        {result && <><h3>{result.status === 'completed' && !result.warning ? 'Selected copies removed' : 'Removal incomplete'}</h3><p>{result.succeeded} succeeded · {result.failed} failed · {result.skipped} skipped</p>{result.warning && <p>{result.warning}</p>}<ul className="max-h-48 overflow-auto">{result.items.map((item, index) => <li key={index}>{item.target_name} · Archive #{item.backup_id} · {item.copy} · {item.label}: {item.outcome} {item.message}</li>)}</ul></>}
      </div>
      {error && <p role="alert" className="my-3 text-red-400">{error}</p>}
      <div className="flex flex-wrap gap-3 mt-5">
        <button disabled={busy} onClick={onClose} className="btn-secondary">{result || planRemoved ? 'Close' : 'Cancel'}</button>
        {!preview && <button disabled={busy} onClick={review} className="btn-primary">Review exact scope</button>}
        {preview && !result && !planRemoved && <button disabled={busy || (!plan && !preview.copy_count)} onClick={() => execute(preview.token)} className="btn-primary">{plan ? 'Remove plan only — keep recovery points' : 'Permanently delete confirmed copies'}</button>}
        {result?.retry_token && <button disabled={busy} onClick={() => execute(result.retry_token!)} className="btn-primary">Retry failed confirmed copies only</button>}
      </div>
      {busy && <p role="status">Waiting for authoritative server result…</p>}
    </div>
  </div>
}
