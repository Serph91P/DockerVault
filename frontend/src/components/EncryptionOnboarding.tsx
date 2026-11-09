import { useRef, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import api from '../api'
import EncryptionSetup from './EncryptionSetup'
import { useNavigate } from 'react-router-dom'
import { useAuthStore } from '../store/auth'
import { ownsSession, sessionGeneration } from '../store/session'

interface Status {
  setup_completed: boolean
  encryption_enabled: boolean
  onboarding_deferred: boolean
}

export default function EncryptionOnboarding({ children }: { children: ReactNode }) {
  const { data, isError, refetch } = useQuery<Status>({
    queryKey: ['encryption-status'],
    queryFn: async ({ signal }) => (await api.get('/encryption/status', { signal })).data,
    retry: false,
  })
  if (isError) return <main className="p-6" role="alert">Unable to verify backup encryption. <button onClick={() => void refetch()}>Retry</button><button onClick={() => void useAuthStore.getState().logout()}>Sign out</button></main>
  if (!data) return <main className="p-6" role="status">Checking backup encryption…</main>
  return <OnboardingJourney status={data} refetch={refetch}>{children}</OnboardingJourney>
}

function OnboardingJourney({ status, refetch, children }: { status: Status; refetch: () => Promise<unknown>; children: ReactNode }) {
  const [onboarding, setOnboarding] = useState(!status.setup_completed && !status.onboarding_deferred)
  const navigate = useNavigate()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const busy = useRef(false)
  const ready = status.setup_completed || status.onboarding_deferred
  const defer = async () => {
    if (busy.current) return
    busy.current = true
    const owner = sessionGeneration()
    setPending(true)
    setError('')
    try {
      await api.post('/encryption/defer')
      if (ownsSession(owner)) await refetch()
    } catch { if (ownsSession(owner)) setError('Unable to save your choice. Setup remains incomplete. Retry.') }
    finally { busy.current = false; if (ownsSession(owner)) setPending(false) }
  }
  if (!onboarding) return children
  return <main className="min-h-screen bg-dark-900 p-4 text-dark-100">
    <div className="max-w-3xl mx-auto space-y-6">
      <h1 className="text-2xl font-bold">Account created — backup protection</h1>
      <p>Your account password signs in. The age public key encrypts archives; the private key restores them. They are not interchangeable.</p>
      <p className="text-sm text-dark-400">System-recovery bundles use their own password in Settings or first-run restore. A bundle does not contain archives or external archive private identities and does not replace private-key custody.</p>
      <EncryptionSetup />
      {error && <p role="alert">{error}</p>}
      {!ready && <section className="space-y-3">
        <p>Setup is incomplete until you confirm custody or explicitly choose unencrypted use. Losing a private key cannot be fixed by your account password.</p>
        <button className="px-4 py-3 bg-dark-700 hover:bg-dark-600 disabled:opacity-50 rounded-lg text-dark-100" disabled={pending} onClick={() => void defer()}>{pending ? 'Saving choice…' : 'Continue without encryption — new backups will be unencrypted'}</button>
      </section>}
      {ready && <section className="space-y-4">
        <h2 className="text-xl">Next: create your first backup</h2>
        <p>{status.setup_completed ? 'Key custody confirmed.' : 'You deliberately deferred encryption. New backups are unencrypted; enable encryption later in Settings.'} No backup has been created by setup.</p>
        <button className="px-4 py-3 bg-indigo-600 hover:bg-indigo-700 rounded-lg text-white" onClick={() => { setOnboarding(false); navigate('/protection/new') }}>Create first backup plan</button>
        <button className="px-4 py-3 bg-dark-700 hover:bg-dark-600 disabled:opacity-50 rounded-lg text-dark-100" onClick={() => setOnboarding(false)}>Continue to DockerVault</button>
      </section>}
      <button className="px-4 py-3 bg-dark-700 hover:bg-dark-600 disabled:opacity-50 rounded-lg text-dark-100" onClick={() => void useAuthStore.getState().logout()}>Sign out</button>
    </div>
  </main>
}
