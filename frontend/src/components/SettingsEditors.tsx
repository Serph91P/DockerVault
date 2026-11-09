import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { settingsApi, type KomodoSettings, type StorageSettings } from '../api'
import { queryKeys } from '../api/queryKeys'
import { operationOutcome, type Outcome } from '../api/outcomes'
import { ownsSession, sessionGeneration } from '../store/session'
import { useValidationFeedback } from './ValidationFeedback'
import { QueryFeedback } from './QueryFeedback'
import { Button } from './ui/button'

const card = 'space-y-4 rounded-xl border border-dark-700 bg-dark-800 p-4 sm:p-6 min-w-0 break-words'
const input = 'w-full rounded-lg border border-dark-500 bg-dark-900 px-3 py-2 text-dark-100'
type Props = { discardRevision: number; onDirty: (dirty: boolean) => void }

// Drafts (including credentials) never enter the Query/Mutation caches.
export function StorageSettingsCard({ discardRevision, onDirty }: Props) {
  const client = useQueryClient()
  const query = useQuery({ queryKey: queryKeys.settings.storage, queryFn: () => settingsApi.getStorageSettings().then(r => r.data) })
  const [draft, storeDraft] = useState<StorageSettings | null>(null)
  const [base, setBase] = useState<StorageSettings | null>(null)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const [result, setResult] = useState('')
  const revision = useRef(0), active = useRef(true), submitting = useRef(false)
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(base)
  useEffect(() => { active.current = true; return () => { active.current = false } }, [])
  useEffect(() => { onDirty(dirty) }, [dirty, onDirty])
  const change = (value: StorageSettings) => { revision.current++; storeDraft(value); setResult(''); setError(null) }
  const begin = (value: StorageSettings) => { revision.current++; storeDraft(value); setBase(value); setResult(''); setError(null) }
  const cancel = () => { revision.current++; storeDraft(null); setBase(null); setError(null); setResult('') }
  const save = async () => {
    if (draft === null || !dirty || submitting.current) return
    const owner = sessionGeneration(), current = revision.current, submitted = draft
    submitting.current = true; setPending(true); setError(null); setResult('')
    let accepted = false
    try {
      await settingsApi.updateStorageSettings(submitted); accepted = true
      if (!active.current || !ownsSession(owner)) return
      const canonical = (await settingsApi.getStorageSettings()).data
      if (!active.current || !ownsSession(owner)) return
      await client.cancelQueries({ queryKey: queryKeys.settings.storage })
      if (!active.current || !ownsSession(owner)) return
      client.setQueryData(queryKeys.settings.storage, canonical)
      if (revision.current !== current) return
      storeDraft(null); setBase(null)
      setResult('Storage settings saved; canonical values confirmed.')
    } catch (failure) {
      if (!active.current || !ownsSession(owner) || revision.current !== current) return
      setError(failure)
      if (accepted) setResult('Write accepted, but canonical readback is unconfirmed. Review or retry the read before resubmitting; the server may already have saved these values.')
    } finally {
      submitting.current = false
      if (active.current && ownsSession(owner)) setPending(false)
    }
  }
  const reset = useRef(discardRevision)
  useEffect(() => { if (reset.current !== discardRevision) { reset.current = discardRevision; cancel() } }, [discardRevision])
  const value = draft ?? query.data
  const validation = useValidationFeedback(error)
  return <div className={card}><h2 className="text-lg font-semibold">Storage & Upload</h2><QueryFeedback name="Storage settings" query={query} />
    {validation.summary}<p role="status" aria-label="Storage save result">{pending ? 'Saving storage settings; awaiting canonical confirmation…' : result || (dirty ? 'Unsaved storage draft' : 'No unsaved storage changes')}</p>
    {value && <><div className="flex items-center justify-between gap-4"><label id="quota-label">Check quota before upload</label><button type="button" role="switch" aria-labelledby="quota-label" aria-describedby="quota-help" aria-checked={value.check_quota_before_upload} className="min-h-11 rounded-lg border border-dark-500 px-4" onClick={() => { if (!draft) begin(value); change({ check_quota_before_upload: !value.check_quota_before_upload }) }}>{value.check_quota_before_upload ? 'On' : 'Off'}</button></div>
      <p id="quota-help">Verify remote capacity before uploading. Disable for servers without quota queries. This safeguard does not reserve capacity or guarantee an upload will succeed.</p>
      {draft && <div className="flex flex-wrap gap-2"><Button disabled={!dirty || pending} onClick={() => void save()}>Save</Button><Button variant="outline" onClick={cancel}>Cancel</Button></div>}
    </>}
  </div>
}

type KomodoDraft = { enabled: boolean; api_url: string; api_key: string; api_secret: string; clear_key: boolean; clear_secret: boolean }
const draftFrom = (value: KomodoSettings): KomodoDraft => ({ enabled: value.enabled, api_url: value.api_url ?? '', api_key: '', api_secret: '', clear_key: false, clear_secret: false })

export function KomodoSettingsCard({ discardRevision, onDirty }: Props) {
  const client = useQueryClient()
  // The editable model is private; readback/cache holds only the redacted server DTO.
  const query = useQuery({ queryKey: queryKeys.settings.komodo, queryFn: () => settingsApi.getKomodo().then(r => r.data) })
  const readRevision = client.getQueryState(queryKeys.settings.komodo)?.dataUpdateCount
  const [draft, setDraft] = useState<KomodoDraft | null>(null), [base, setBase] = useState<KomodoSettings | null>(null)
  const [pending, setPending] = useState(false), [error, setError] = useState<unknown>(null), [result, setResult] = useState('')
  const [visible, setVisible] = useState({ key: false, secret: false })
  const [testing, setTesting] = useState(false), [test, setTest] = useState<{ outcome: Outcome; at: number; snapshot: string; readRevision: number | undefined } | null>(null)
  const revision = useRef(0), active = useRef(true), submit = useRef(false), testSubmit = useRef(false)
  const dirty = draft !== null && base !== null && JSON.stringify(draft) !== JSON.stringify(draftFrom(base))
  useEffect(() => { active.current = true; return () => { active.current = false } }, [])
  useEffect(() => { onDirty(dirty) }, [dirty, onDirty])
  const cancel = () => { revision.current++; setDraft(null); setBase(null); setVisible({ key: false, secret: false }); setError(null); setResult(''); setTest(null) }
  const reset = useRef(discardRevision)
  useEffect(() => { if (reset.current !== discardRevision) { reset.current = discardRevision; cancel() } }, [discardRevision])
  const change = (changes: Partial<KomodoDraft>) => { revision.current++; setDraft(d => d && { ...d, ...changes }); setTest(null); setResult(''); setError(null) }
  const begin = () => { if (!query.data || query.isError) return; revision.current++; setBase(query.data); setDraft(draftFrom(query.data)); setResult(''); setError(null); setTest(null) }
  const save = async () => {
    if (!draft || !base || !dirty || submit.current) return
    const owner = sessionGeneration(), current = revision.current
    const payload = { enabled: draft.enabled,
      ...(draft.api_url !== (base.api_url ?? '') ? { api_url: draft.api_url } : {}),
      ...(draft.clear_key ? { api_key: '' } : draft.api_key ? { api_key: draft.api_key } : {}),
      ...(draft.clear_secret ? { api_secret: '' } : draft.api_secret ? { api_secret: draft.api_secret } : {}),
    }
    submit.current = true; setPending(true); setError(null); setTest(null); setResult('')
    let accepted = false
    try {
      await settingsApi.updateKomodo(payload); accepted = true
      if (!active.current || !ownsSession(owner)) return
      const canonical = (await settingsApi.getKomodo()).data
      if (!active.current || !ownsSession(owner)) return
      await client.cancelQueries({ queryKey: queryKeys.settings.komodo })
      if (!active.current || !ownsSession(owner)) return
      client.setQueryData(queryKeys.settings.komodo, canonical)
      if (revision.current !== current) return
      setDraft(null); setBase(null); setVisible({ key: false, secret: false }); setResult('Komodo settings saved; canonical values confirmed.')
    } catch (failure) {
      if (!active.current || !ownsSession(owner) || revision.current !== current) return
      setError(failure)
      if (accepted) setResult('Write accepted, but canonical readback is unconfirmed. Refresh saved configuration before resubmitting; the server may already have saved these values.')
    } finally { submit.current = false; if (active.current && ownsSession(owner)) setPending(false) }
  }
  const testSaved = async () => {
    if (dirty || pending || testSubmit.current || !query.data || query.isError) return
    const owner = sessionGeneration(), current = revision.current, snapshot = JSON.stringify(query.data)
    testSubmit.current = true; setTesting(true); setTest(null)
    try {
      const response = await settingsApi.testKomodo()
      if (active.current && ownsSession(owner) && revision.current === current && client.getQueryState(queryKeys.settings.komodo)?.dataUpdateCount === readRevision && JSON.stringify(client.getQueryData(queryKeys.settings.komodo)) === snapshot) setTest({ outcome: operationOutcome(response.data), at: Date.now(), snapshot, readRevision })
    } catch { if (active.current && ownsSession(owner) && revision.current === current) setTest({ outcome: 'unknown', at: Date.now(), snapshot, readRevision }) }
    finally { testSubmit.current = false; if (active.current && ownsSession(owner)) setTesting(false) }
  }
  const validation = useValidationFeedback(error), saved = query.data
  const changed = base && saved && JSON.stringify(base) !== JSON.stringify(saved)
  return <div className={card}><h2 className="text-lg font-semibold">Komodo Integration</h2><QueryFeedback name="Komodo settings" query={query} />
    <p>Effective configuration uses database overrides, then environment values, then defaults. Blank masked credentials keep the effective value. Clear explicitly stores an empty override; it does not restore environment inheritance.</p>
    {saved && <div aria-label="Saved effective Komodo configuration"><p>Integration: {saved.enabled ? 'Enabled' : 'Disabled'}. Connectivity: {query.isError ? 'unknown (stale observation)' : saved.connected ? 'Connected' : 'not connected'}.</p><p>API URL: <span className="break-all">{saved.api_url || 'Not set'}</span></p><p>API Key: {saved.has_api_key ? 'Configured' : 'Not set'} · API Secret: {saved.has_api_secret ? 'Configured' : 'Not set'}</p></div>}
    <p>Discovery is separate from connectivity. Negotiated lifecycle capabilities are unavailable until backend dependency #144; a version or successful connection does not authorize stop, start or restart.</p>
    {changed && <p role="status">Saved configuration changed while editing. Your draft is retained; Cancel uses the latest server values. Saving applies the draft enablement and changed endpoint or credentials.</p>}
    {validation.summary}<p role="status" aria-label="Komodo save result">{pending ? 'Saving Komodo settings; awaiting canonical confirmation…' : result || (dirty ? 'Unsaved Komodo draft' : 'No unsaved Komodo changes')}</p>
    {draft ? <div className="space-y-4"><div className="flex items-center gap-4"><span id="komodo-enable-label">Enable Komodo integration</span><button type="button" role="switch" aria-labelledby="komodo-enable-label" aria-checked={draft.enabled} className="min-h-11 rounded-lg border border-dark-500 px-4" onClick={() => change({ enabled: !draft.enabled })}>{draft.enabled ? 'On' : 'Off'}</button></div>
      <div><label htmlFor="komodo-url">Komodo API URL</label><input id="komodo-url" type="text" className={input} value={draft.api_url} {...validation.field('api_url')} onChange={e => change({ api_url: e.target.value })} autoComplete="off" /></div>
      {(['key', 'secret'] as const).map(kind => { const field = kind === 'key' ? 'api_key' : 'api_secret', clear = kind === 'key' ? 'clear_key' : 'clear_secret'; return <div key={kind} className="space-y-2"><label htmlFor={`komodo-${kind}`}>Komodo API {kind}</label><input id={`komodo-${kind}`} className={input} type={visible[kind] ? 'text' : 'password'} autoComplete="off" aria-describedby="komodo-secret-help" {...validation.field(field)} disabled={draft[clear]} value={draft[field]} onChange={e => change({ [field]: e.target.value })} /><div className="flex flex-wrap items-center gap-4"><Button variant="outline" onClick={() => setVisible(v => ({ ...v, [kind]: !v[kind] }))}>{visible[kind] ? 'Hide' : 'Show'} API {kind}</Button><label className="flex min-h-11 items-center gap-2"><input type="checkbox" checked={draft[clear]} onChange={e => change({ [clear]: e.target.checked, [field]: '' })} />Clear API {kind}</label></div></div> })}
      <p id="komodo-secret-help">Credentials stay only in this page's private memory. Cancel, leaving, logout or session expiry clears them. Failed requests retain this draft for correction; nothing is stored locally.</p>
      <div className="flex flex-wrap gap-2"><Button onClick={() => void save()} disabled={!dirty || pending}>Save</Button><Button variant="outline" onClick={cancel}>Cancel</Button></div>
    </div> : <Button variant="outline" onClick={begin} disabled={!saved || query.isError}>Edit Settings</Button>}
    <Button variant="outline" onClick={() => void testSaved()} disabled={!saved || !saved.enabled || dirty || pending || testing || query.isError}>Test saved connection</Button>
    <p>Test uses only the saved effective server configuration, never a draft. Save or Cancel unsaved changes before testing. It does not save credentials.</p>
    {test && !query.isError && test.readRevision === readRevision && test.snapshot === JSON.stringify(saved) && <p role="status">Komodo test: {test.outcome}; measured {new Date(test.at).toLocaleString()} (UI session only).</p>}
  </div>
}
