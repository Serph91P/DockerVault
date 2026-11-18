import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { settingsApi, type StorageSettings } from '../api'
import { queryKeys } from '../api/queryKeys'
import { ownsSession, sessionGeneration } from '../store/session'
import { useValidationFeedback } from './ValidationFeedback'
import { QueryFeedback } from './QueryFeedback'
import { Button } from './ui/button'

const card = 'space-y-4 rounded-xl border border-dark-700 bg-dark-800 p-4 sm:p-6 min-w-0 break-words'
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
      if (revision.current !== current) { setBase(canonical); return }
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
