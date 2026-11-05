import { useEffect, useRef, useState } from 'react'
import api, { type BackupTarget } from '../api'
import type { PrivateTargetConfig } from '../api/targetPrivacy'
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session'
import { ResponseError } from '../api/responses'

// Component-private lifecycle, never a Query/Mutation record or persisted draft.
export function usePrivateTargetEditor(targetId?: number) {
  const [hooks, setHooks] = useState({ pre: '', post: '' })
  const [loaded, setLoaded] = useState(!targetId)
  const [loading, setLoading] = useState(!!targetId)
  const [loadError, setLoadError] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [pending, setPending] = useState(false)
  const [saveError, setSaveError] = useState(false)
  const [saveFailure, setSaveFailure] = useState<ResponseError | null>(null)
  const busy = useRef(false)
  const context = useRef<AbortController | null>(null)
  const close = () => {
    context.current?.abort()
    setHooks({ pre: '', post: '' })
    setLoaded(false)
    setPending(false)
    setSaveFailure(null)
  }

  useEffect(() => {
    const requests = new AbortController()
    context.current = requests
    const owner = sessionGeneration()
    const session = sessionSignal()
    const current = () => !requests.signal.aborted && ownsSession(owner)
    const clear = () => { requests.abort(); setHooks({ pre: '', post: '' }); setLoaded(false); setLoading(false); setPending(false); setSaveFailure(null) }
    session.addEventListener('abort', clear, { once: true })
    if (targetId) {
      setLoading(true)
      setLoadError(false)
      const config: PrivateTargetConfig = { signal: AbortSignal.any([requests.signal, session]), privateTargetHooks: true }
      void api.get<BackupTarget>(`/targets/${targetId}`, config).then(response => {
        if (!current()) return
        setHooks({ pre: response.data.pre_backup_command ?? '', post: response.data.post_backup_command ?? '' })
        setLoaded(true)
      }).catch(() => { if (current()) setLoadError(true) }).finally(() => { if (current()) setLoading(false) })
    }
    return () => { requests.abort(); session.removeEventListener('abort', clear) }
  }, [targetId, attempt])

  const run = async (action: (signal: AbortSignal, current: () => boolean) => Promise<void>, success: () => void) => {
    const requests = context.current
    if (busy.current || !requests || requests.signal.aborted) return
    busy.current = true
    const owner = sessionGeneration()
    const current = () => !requests.signal.aborted && ownsSession(owner)
    setPending(true)
    setSaveError(false)
    setSaveFailure(null)
    try {
      await action(AbortSignal.any([requests.signal, sessionSignal()]), current)
      if (current()) success()
    } catch (error) { if (current()) { setSaveError(true); setSaveFailure(error instanceof ResponseError ? error : new ResponseError()) } }
    finally { busy.current = false; if (current()) setPending(false) }
  }
  return { hooks, setHooks, loaded, loading, loadError, retry: () => setAttempt(value => value + 1), pending, saveError, saveFailure, run, close }
}
