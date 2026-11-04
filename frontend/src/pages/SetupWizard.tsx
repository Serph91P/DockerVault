import { useRef, useState } from 'react'
import { Shield } from 'lucide-react'
import { useAuthStore } from '../store/auth'

export default function SetupWizard() {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [confirmation, setConfirmation] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const busy = useRef(false)
  const setup = useAuthStore(s => s.setup)
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (busy.current) return
    if (username.trim().length < 3 || password.length < 8 || confirmation !== password) {
      setError('Use at least 3 username characters and 8 password characters. Both account passwords must match.')
      return
    }
    busy.current = true
    setPending(true)
    setError('')
    try {
      await setup(username.trim(), password)
    } catch {
      setError('Account creation could not be confirmed. Check status before retrying; an existing account must sign in.')
    } finally {
      setPassword('')
      setConfirmation('')
      setPending(false)
      busy.current = false
    }
  }
  return <main className="min-h-screen bg-dark-900 flex items-center justify-center p-4">
    <div className="w-full max-w-lg space-y-6">
      <Shield className="w-12 h-12 text-indigo-400" />
      <h1 className="text-3xl font-bold text-dark-100">Welcome to DockerVault</h1>
      <p className="text-dark-300">Step 1: Create your administrator account. Next: backup encryption and private-key custody.</p>
      <form onSubmit={submit} className="bg-dark-800 rounded-xl border border-dark-700 p-6 space-y-4">
        {error && <p role="alert" className="text-red-400">{error} <button type="button" onClick={() => void useAuthStore.getState().checkAuthStatus()}>Check status</button></p>}
        <label htmlFor="setup-username" className="block text-dark-200">Username</label>
        <input id="setup-username" className="w-full px-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:ring-2 focus:ring-indigo-500" autoComplete="username" minLength={3} required value={username} onChange={e => setUsername(e.target.value)} />
        <label htmlFor="setup-password" className="block text-dark-200">Account password</label>
        <input id="setup-password" className="w-full px-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:ring-2 focus:ring-indigo-500" type="password" autoComplete="new-password" minLength={8} required value={password} onChange={e => setPassword(e.target.value)} />
        <label htmlFor="setup-confirm" className="block text-dark-200">Confirm account password</label>
        <input id="setup-confirm" className="w-full px-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 focus:outline-none focus:ring-2 focus:ring-indigo-500" type="password" autoComplete="new-password" minLength={8} required value={confirmation} onChange={e => setConfirmation(e.target.value)} />
        <p className="text-sm text-dark-400">This password signs in to DockerVault. It does not decrypt age archives. Keep the separate private recovery key exported in the next step.</p>
        <button type="submit" className="w-full px-4 py-3 bg-indigo-600 hover:bg-indigo-700 disabled:opacity-50 rounded-lg text-white" disabled={pending}>{pending ? 'Creating account…' : 'Create account'}</button>
      </form>
    </div>
  </main>
}
