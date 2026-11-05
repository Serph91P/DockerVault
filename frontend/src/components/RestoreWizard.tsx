import { useState, useEffect, useCallback, useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  X,
  RotateCcw,
  HardDrive,
  FolderOpen,
  Lock,
  AlertTriangle,
  CheckCircle,
  Loader2,
  ChevronRight,
  ChevronLeft,
  Server,
  ArrowRight,
} from 'lucide-react'
import api, { RestoreInfo } from '../api'
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session'
import { queryKeys } from '../api/queryKeys'
import { operationOutcome } from '../api/outcomes'
import { useNotificationStore } from '../store/notifications'
import { clsx } from 'clsx'
import toast from 'react-hot-toast'

interface RestoreWizardProps {
  backupId: number
  onClose: () => void
}

type DestinationType = 'original' | 'volume' | 'path'

interface RestoreState {
  destination: DestinationType
  targetVolume: string
  targetPath: string
  privateKey: string
}

const STEPS = ['Destination', 'Decryption', 'Confirm'] as const
const STEPS_NO_ENCRYPT = ['Destination', 'Confirm'] as const

export default function RestoreWizard({ backupId, onClose }: RestoreWizardProps) {
  const queryClient = useQueryClient()
  const [info, setInfo] = useState<RestoreInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [step, setStep] = useState(0)
  const [restoring, setRestoring] = useState(false)
  const busy = useRef(false)
  const requests = useRef(new AbortController())
  const [state, setState] = useState<RestoreState>({
    destination: 'original',
    targetVolume: '',
    targetPath: '',
    privateKey: '',
  })

  const isEncrypted = info?.backup.encrypted ?? false
  const steps = isEncrypted ? STEPS : STEPS_NO_ENCRYPT
  const isLastStep = step === steps.length - 1

  useEffect(() => {
    let cancelled = false
    const controller = new AbortController()
    requests.current = controller
    async function load() {
      try {
        setLoading(true)
        const res = await api.get<RestoreInfo>(`/backups/${backupId}/restore-info`, { signal: controller.signal })
        if (!cancelled) {
          setInfo(res.data)
          if (res.data.target.target_type === 'volume' && res.data.target.volume_name) {
            setState((s) => ({ ...s, targetVolume: res.data.target.volume_name! }))
          }
          setError(null)
        }
      } catch {
        if (!cancelled) setError('Failed to load backup info')
      } finally {
        if (!cancelled) setLoading(false)
      }
    }
    load()
    return () => { cancelled = true; controller.abort() }
  }, [backupId])

  const restore = async () => {
    if (busy.current) return
    busy.current = true
    const owner = sessionGeneration()
    const signal = AbortSignal.any([requests.current.signal, sessionSignal()])
    setRestoring(true)
    const startedAt = Date.now()
    try {
      const opts: { target_path?: string; private_key?: string } = {}
      if (state.destination === 'path' && state.targetPath) {
        opts.target_path = state.targetPath
      }
      if (state.destination === 'volume' && state.targetVolume && info?.target.volume_name !== state.targetVolume) {
        opts.target_path = `/var/lib/docker/volumes/${state.targetVolume}/_data`
      }
      if (isEncrypted && state.privateKey) {
        opts.private_key = state.privateKey
      }
      const result = await api.post(`/backups/${backupId}/restore`, opts, { signal })
      if (result.data.status !== 'restored') throw new Error('Restore not confirmed')
      if (!ownsSession(owner) || signal.aborted) return
      setState(s => ({ ...s, privateKey: '' }))
      const phase = operationOutcome(result.data)
      if (phase !== 'completed' && phase !== 'partial') throw new Error('Restore outcome unavailable')
      useNotificationStore.getState().addNotification({ type: phase === 'partial' ? 'warning' : 'success', title: 'Restore result', message: phase === 'partial' ? 'Restore returned warnings. Review the archive and actual destination before retrying.' : 'Backend confirmed restoration. Verify the destination content.', target: { kind: 'archive', id: backupId }, phase, startedAt })
      if (phase === 'partial') toast('Restore completed with warnings', { icon: '!' })
      else toast.success('Restore completed successfully')
      queryClient.invalidateQueries({ queryKey: queryKeys.backups.all })
      onClose()
    } catch {
      if (ownsSession(owner) && !signal.aborted) {
        setState(s => ({ ...s, privateKey: '' }))
        setStep(isEncrypted ? 1 : 0)
        useNotificationStore.getState().addNotification({ type: 'error', title: 'Restore result unknown', message: 'No final restore result was confirmed. Inspect the destination before submitting again.', target: { kind: 'archive', id: backupId }, phase: 'unknown', startedAt })
        toast.error('Restore failed — enter your key again to retry; check backup logs for details')
      }
    } finally {
      busy.current = false
      if (ownsSession(owner) && !signal.aborted) setRestoring(false)
    }
  }

  const canProceed = useCallback((): boolean => {
    const currentStepName = steps[step]
    if (currentStepName === 'Destination') {
      if (state.destination === 'path' && !state.targetPath.trim()) return false
      if (state.destination === 'volume' && !state.targetVolume) return false
      return true
    }
    if (currentStepName === 'Decryption') {
      return state.privateKey.trim().length > 0
    }
    return true
  }, [step, steps, state])

  const handleNext = () => {
    if (isLastStep) {
      void restore()
    } else {
      setStep((s) => s + 1)
    }
  }

  const getDestinationLabel = (): string => {
    if (!info) return ''
    if (state.destination === 'original') {
      if (info.target.target_type === 'volume') return `Volume: ${info.target.volume_name}`
      if (info.target.target_type === 'path') return `Path: ${info.target.host_path}`
      return info.target.name
    }
    if (state.destination === 'volume') return `Volume: ${state.targetVolume}`
    return `Path: ${state.targetPath}`
  }

  if (loading) {
    return (
      <Modal title="Restore Backup" onClose={onClose}>
        <div className="bg-dark-800 rounded-xl p-8 flex items-center gap-3">
          <Loader2 className="w-5 h-5 text-primary-500 animate-spin" />
          <span className="text-dark-200">Loading restore info…</span>
        </div>
      </Modal>
    )
  }

  if (error || !info) {
    return (
      <Modal title="Restore Backup" onClose={onClose}>
        <div className="bg-dark-800 rounded-xl p-8 max-w-md">
          <p className="text-red-400 mb-4">{error || 'Could not load backup information'}</p>
          <button onClick={onClose} className="px-4 py-2 bg-dark-600 text-dark-200 rounded-lg hover:bg-dark-500">
            Close
          </button>
        </div>
      </Modal>
    )
  }

  return (
    <Modal title="Restore Backup" onClose={onClose} pending={restoring}>
      <div
        className="bg-dark-800 border border-dark-600 rounded-xl shadow-2xl w-full max-w-lg mx-4"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-dark-600">
          <div className="flex items-center gap-2">
            <RotateCcw className="w-5 h-5 text-primary-500" />
            <h2 className="text-lg font-semibold text-dark-100">Restore Backup</h2>
          </div>
          <button onClick={onClose} disabled={restoring} aria-label="Close restore" className="p-2 text-dark-400 hover:text-dark-200 rounded">
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Step indicator */}
        <div className="flex items-center gap-2 px-6 pt-4">
          {steps.map((label, i) => (
            <div key={label} className="flex items-center gap-2">
              <div
                className={clsx(
                  'w-7 h-7 rounded-full flex items-center justify-center text-xs font-medium',
                  i < step && 'bg-green-600 text-white',
                  i === step && 'bg-primary-600 text-white',
                  i > step && 'bg-dark-600 text-dark-400',
                )}
              >
                {i < step ? <CheckCircle className="w-4 h-4" /> : i + 1}
              </div>
              <span className={clsx('text-sm', i === step ? 'text-dark-200 font-medium' : 'text-dark-400')}>
                {label}
              </span>
              {i < steps.length - 1 && <ArrowRight className="w-3 h-3 text-dark-500" />}
            </div>
          ))}
        </div>

        {/* Content */}
        <div className="px-6 py-5 min-h-[260px]">
          {steps[step] === 'Destination' && (
            <StepDestination info={info} state={state} setState={setState} />
          )}
          {steps[step] === 'Decryption' && (
            <StepDecryption state={state} setState={setState} />
          )}
          {steps[step] === 'Confirm' && (
            <StepConfirm
              info={info}
              destinationLabel={getDestinationLabel()}
              isEncrypted={isEncrypted}
            />
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-6 py-4 border-t border-dark-600">
          <button
            onClick={() => (step === 0 ? onClose() : setStep((s) => s - 1))}
            disabled={restoring}
            className="flex items-center gap-1 px-4 py-2 text-sm text-dark-300 hover:text-dark-100 transition-colors"
          >
            <ChevronLeft className="w-4 h-4" />
            {step === 0 ? 'Cancel' : 'Back'}
          </button>
          <button
            onClick={handleNext}
            disabled={!canProceed() || restoring}
            className={clsx(
              'flex items-center gap-1 px-5 py-2 text-sm font-medium rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed',
              isLastStep
                ? 'bg-orange-600 hover:bg-orange-500 text-white'
                : 'bg-primary-600 hover:bg-primary-500 text-white',
            )}
          >
            {restoring ? (
              <>
                <Loader2 className="w-4 h-4 animate-spin" />
                Restoring…
              </>
            ) : isLastStep ? (
              <>
                <RotateCcw className="w-4 h-4" />
                Restore Now
              </>
            ) : (
              <>
                Next
                <ChevronRight className="w-4 h-4" />
              </>
            )}
          </button>
        </div>
      </div>
    </Modal>
  )
}

// --- Step Components ---
import { Modal } from './ui/dialog'

function StepDestination({
  info,
  state,
  setState,
}: {
  info: RestoreInfo
  state: RestoreState
  setState: React.Dispatch<React.SetStateAction<RestoreState>>
}) {
  const isVolume = info.target.target_type === 'volume'
  const isPath = info.target.target_type === 'path'

  return (
    <div className="space-y-4">
      <p className="text-sm text-dark-300">
        Choose where to restore backup <span className="text-dark-100 font-medium">#{info.backup.id}</span>{' '}
        from <span className="text-dark-100 font-medium">{info.target.name}</span>.
      </p>

      {/* Original location */}
      <label
        className={clsx(
          'flex items-start gap-3 p-3 rounded-lg border cursor-pointer transition-colors',
          state.destination === 'original'
            ? 'border-primary-500 bg-primary-500/10'
            : 'border-dark-600 hover:border-dark-500',
        )}
      >
        <input
          type="radio"
          name="destination"
          checked={state.destination === 'original'}
          onChange={() => setState((s) => ({ ...s, destination: 'original' }))}
          className="mt-1 accent-primary-500"
        />
        <div className="flex-1">
          <div className="flex items-center gap-2 text-dark-200 font-medium text-sm">
            {isVolume ? <HardDrive className="w-4 h-4" /> : <FolderOpen className="w-4 h-4" />}
            Original location
          </div>
          <p className="text-xs text-dark-400 mt-1">
            {isVolume && `Volume: ${info.target.volume_name}`}
            {isPath && `Path: ${info.target.host_path}`}
            {!isVolume && !isPath && info.target.name}
          </p>
        </div>
      </label>

      {/* Alternative volume */}
      {isVolume && info.available_volumes.length > 0 && (
        <label
          className={clsx(
            'flex items-start gap-3 p-3 rounded-lg border cursor-pointer transition-colors',
            state.destination === 'volume'
              ? 'border-primary-500 bg-primary-500/10'
              : 'border-dark-600 hover:border-dark-500',
          )}
        >
          <input
            type="radio"
            name="destination"
            checked={state.destination === 'volume'}
            onChange={() => setState((s) => ({ ...s, destination: 'volume' }))}
            className="mt-1 accent-primary-500"
          />
          <div className="flex-1">
            <div className="flex items-center gap-2 text-dark-200 font-medium text-sm">
              <Server className="w-4 h-4" />
              Different volume
            </div>
            {state.destination === 'volume' && (
              <select
                value={state.targetVolume}
                onChange={(e) => setState((s) => ({ ...s, targetVolume: e.target.value }))}
                className="mt-2 w-full px-3 py-1.5 bg-dark-700 border border-dark-500 rounded text-sm text-dark-200 focus:outline-none focus:ring-1 focus:ring-primary-500"
              >
                <option value="">Select volume…</option>
                {info.available_volumes.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            )}
          </div>
        </label>
      )}

      {/* Custom path */}
      <label
        className={clsx(
          'flex items-start gap-3 p-3 rounded-lg border cursor-pointer transition-colors',
          state.destination === 'path'
            ? 'border-primary-500 bg-primary-500/10'
            : 'border-dark-600 hover:border-dark-500',
        )}
      >
        <input
          type="radio"
          name="destination"
          checked={state.destination === 'path'}
          onChange={() => setState((s) => ({ ...s, destination: 'path' }))}
          className="mt-1 accent-primary-500"
        />
        <div className="flex-1">
          <div className="flex items-center gap-2 text-dark-200 font-medium text-sm">
            <FolderOpen className="w-4 h-4" />
            Custom path
          </div>
          {state.destination === 'path' && (
            <input
              type="text"
              placeholder="/path/to/restore"
              value={state.targetPath}
              onChange={(e) => setState((s) => ({ ...s, targetPath: e.target.value }))}
              className="mt-2 w-full px-3 py-1.5 bg-dark-700 border border-dark-500 rounded text-sm text-dark-200 placeholder-dark-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
            />
          )}
        </div>
      </label>

      {!info.backup.local_available && (
        <div className="flex items-center gap-2 p-3 bg-yellow-900/20 border border-yellow-700/30 rounded-lg text-yellow-400 text-sm">
          <AlertTriangle className="w-4 h-4 shrink-0" />
          Backup file is not available locally. Restore may fail.
        </div>
      )}
    </div>
  )
}

function StepDecryption({
  state,
  setState,
}: {
  state: RestoreState
  setState: React.Dispatch<React.SetStateAction<RestoreState>>
}) {
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2 text-dark-200">
        <Lock className="w-5 h-5 text-yellow-500" />
        <span className="font-medium">Encrypted Backup</span>
      </div>
      <p className="text-sm text-dark-300">
        This backup is encrypted. Provide the private key (age secret key) to decrypt it before restoring.
      </p>
      <textarea
        rows={6}
        placeholder="AGE-SECRET-KEY-1..."
        value={state.privateKey}
        onChange={(e) => setState((s) => ({ ...s, privateKey: e.target.value }))}
        className="w-full px-3 py-2 bg-dark-700 border border-dark-500 rounded-lg text-sm text-dark-200 placeholder-dark-500 font-mono focus:outline-none focus:ring-1 focus:ring-primary-500 resize-none"
        spellCheck={false}
      />
      <p className="text-xs text-dark-400">
        The key is only sent to the server for this restore operation and is not stored.
      </p>
    </div>
  )
}

function StepConfirm({
  info,
  destinationLabel,
  isEncrypted,
}: {
  info: RestoreInfo
  destinationLabel: string
  isEncrypted: boolean
}) {
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2 p-3 bg-orange-900/20 border border-orange-700/30 rounded-lg text-orange-400 text-sm">
        <AlertTriangle className="w-4 h-4 shrink-0" />
        <span>
          Restoring will <strong>overwrite existing data</strong> at the destination. This cannot be undone.
        </span>
      </div>

      <div className="space-y-3 bg-dark-750 rounded-lg p-4">
        <div className="flex justify-between text-sm">
          <span className="text-dark-400">Backup</span>
          <span className="text-dark-200">#{info.backup.id} — {info.backup.file_size_human || 'unknown size'}</span>
        </div>
        <div className="flex justify-between text-sm">
          <span className="text-dark-400">Source Target</span>
          <span className="text-dark-200">{info.target.name}</span>
        </div>
        <div className="flex justify-between text-sm">
          <span className="text-dark-400">Restore to</span>
          <span className="text-dark-200">{destinationLabel}</span>
        </div>
        {isEncrypted && (
          <div className="flex justify-between text-sm">
            <span className="text-dark-400">Encryption</span>
            <span className="text-yellow-400 flex items-center gap-1">
              <Lock className="w-3 h-3" /> Will be decrypted
            </span>
          </div>
        )}
        {info.containers_to_stop.length > 0 && (
          <div className="flex justify-between text-sm">
            <span className="text-dark-400">Containers to stop</span>
            <span className="text-dark-200">{info.containers_to_stop.join(', ')}</span>
          </div>
        )}
      </div>

      {info.containers_to_stop.length > 0 && (
        <p className="text-xs text-dark-400">
          The listed containers will be stopped before restore and restarted after.
        </p>
      )}
    </div>
  )
}
