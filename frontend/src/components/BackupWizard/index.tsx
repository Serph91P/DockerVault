import { useState, useEffect, useCallback } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { X, ChevronLeft, ChevronRight, Check, Loader2 } from 'lucide-react'
import {
  dockerApi,
  schedulesApi,
  retentionApi,
  storageApi,
  targetsApi,
} from '../../api'
import toast from 'react-hot-toast'

import StepTargetSelect from './StepTargetSelect'
import StepDependencies from './StepDependencies'
import StepSchedule from './StepSchedule'
import StepStorage from './StepStorage'
import StepRetention from './StepRetention'
import StepOptions from './StepOptions'
import StepSummary from './StepSummary'
import StepVolumeConfig from './StepVolumeConfig'

export interface WizardData {
  // Step 1: Target
  targetType: 'container' | 'volume' | 'path' | 'stack' | null
  targetName: string
  containerName: string
  volumeName: string
  hostPath: string
  stackName: string

  // Step 2: Volume Configuration (for container/stack)
  selectedVolumes: string[]  // Empty = all volumes
  includePaths: string[]     // Include only these paths (empty = all)
  excludePaths: string[]     // Exclude these paths/patterns
  perVolumeRules: Record<string, { includePaths: string[]; excludePaths: string[] }>

  // Step 3: Dependencies
  dependencies: string[]
  stopContainer: boolean

  // Step 4: Schedule
  scheduleId: number | null
  newSchedule: {
    name: string
    cronExpression: string
    description: string
  } | null

  // Step 5: Storage
  remoteStorageIds: number[]

  // Step 6: Retention
  retentionPolicyId: number | null
  newRetentionPolicy: {
    name: string
    keepLast: number
    keepDaily: number
    keepWeekly: number
    keepMonthly: number
  } | null

  // Step 7: Options
  compression: 'none' | 'gzip' | 'zstd'
  preCommand: string
  postCommand: string
  enabled: boolean
}

const initialData: WizardData = {
  targetType: null,
  targetName: '',
  containerName: '',
  volumeName: '',
  hostPath: '',
  stackName: '',
  selectedVolumes: [],
  includePaths: [],
  excludePaths: [],
  perVolumeRules: {},
  dependencies: [],
  stopContainer: true,
  scheduleId: null,
  newSchedule: null,
  remoteStorageIds: [],
  retentionPolicyId: null,
  newRetentionPolicy: null,
  compression: 'gzip',
  preCommand: '',
  postCommand: '',
  enabled: true,
}

const STEPS = [
  { id: 1, name: 'Target', description: 'What to backup' },
  { id: 2, name: 'Volumes', description: 'Which volumes/paths' },
  { id: 3, name: 'Dependencies', description: 'Container dependencies' },
  { id: 4, name: 'Schedule', description: 'When to backup' },
  { id: 5, name: 'Storage', description: 'Where to store' },
  { id: 6, name: 'Retention', description: 'How long to keep' },
  { id: 7, name: 'Options', description: 'Advanced settings' },
  { id: 8, name: 'Summary', description: 'Review & create' },
]

// W2: LocalStorage draft persistence
const DRAFT_KEY = 'dockervault-wizard-draft'

interface WizardDraft {
  data: WizardData
  currentStep: number
  savedAt: number
}

function saveDraft(data: WizardData, currentStep: number) {
  try {
    const draft: WizardDraft = { data, currentStep, savedAt: Date.now() }
    localStorage.setItem(DRAFT_KEY, JSON.stringify(draft))
  } catch { /* localStorage full or unavailable */ }
}

function loadDraft(): WizardDraft | null {
  try {
    const raw = localStorage.getItem(DRAFT_KEY)
    if (!raw) return null
    const draft: WizardDraft = JSON.parse(raw)
    // Discard drafts older than 24 hours
    if (Date.now() - draft.savedAt > 24 * 60 * 60 * 1000) {
      localStorage.removeItem(DRAFT_KEY)
      return null
    }
    return draft
  } catch {
    return null
  }
}

function clearDraft() {
  try { localStorage.removeItem(DRAFT_KEY) } catch { /* ignore */ }
}

interface BackupWizardProps {
  isOpen: boolean
  onClose: () => void
  editTarget?: import('../../api').BackupTarget | null
  preselectedType?: 'container' | 'volume' | 'path' | 'stack' | null
  preselectedTarget?: string | null
}

function targetToWizardData(target: import('../../api').BackupTarget): WizardData {
  return {
    targetType: target.target_type,
    targetName: target.name,
    containerName: target.container_name || '',
    volumeName: target.volume_name || '',
    hostPath: target.host_path || '',
    stackName: target.stack_name || '',
    selectedVolumes: target.selected_volumes || [],
    includePaths: target.include_paths || [],
    excludePaths: target.exclude_paths || [],
    perVolumeRules: Object.fromEntries(
      Object.entries(target.per_volume_rules || {}).map(([k, v]) => [
        k,
        { includePaths: v.include_paths || [], excludePaths: v.exclude_paths || [] },
      ])
    ),
    dependencies: target.dependencies || [],
    stopContainer: target.stop_container,
    scheduleId: target.schedule_id || null,
    newSchedule: null,
    remoteStorageIds: target.remote_storage_ids || [],
    retentionPolicyId: target.retention_policy_id || null,
    newRetentionPolicy: null,
    compression: target.compression_enabled ? 'gzip' : 'none',
    preCommand: target.pre_backup_command || '',
    postCommand: target.post_backup_command || '',
    enabled: target.enabled,
  }
}

export default function BackupWizard({ isOpen, onClose, editTarget, preselectedType, preselectedTarget }: BackupWizardProps) {
  const queryClient = useQueryClient()
  const isEditing = !!editTarget
  const [currentStep, setCurrentStep] = useState(1)
  const [data, setData] = useState<WizardData>(initialData)
  const [showDraftBanner, setShowDraftBanner] = useState(false)

  // W2: Check for saved draft when wizard opens (non-edit, no preselection)
  const [draftChecked, setDraftChecked] = useState(false)
  if (isOpen && !isEditing && !preselectedType && !draftChecked) {
    setDraftChecked(true)
    const draft = loadDraft()
    if (draft && draft.data.targetType) {
      setShowDraftBanner(true)
    }
  }
  if (!isOpen && draftChecked) {
    setDraftChecked(false)
  }

  // Pre-populate data when editing
  const [lastEditId, setLastEditId] = useState<number | null>(null)
  if (editTarget && editTarget.id !== lastEditId) {
    setData(targetToWizardData(editTarget))
    setLastEditId(editTarget.id)
  }

  // T1: Handle preselection - auto-select type and target, advance to step 2
  const [lastPreselection, setLastPreselection] = useState<string | null>(null)
  const preselectionKey = preselectedType && preselectedTarget
    ? `${preselectedType}:${preselectedTarget}`
    : null
  if (isOpen && !isEditing && preselectionKey && preselectionKey !== lastPreselection) {
    setLastPreselection(preselectionKey)

    const updates: Partial<WizardData> = {
      targetType: preselectedType ?? null,
    }
    // Set the appropriate target field and auto-generate name
    switch (preselectedType) {
      case 'container':
        updates.containerName = preselectedTarget ?? ''
        updates.targetName = `${preselectedTarget} Backup`
        break
      case 'volume':
        updates.volumeName = preselectedTarget ?? ''
        updates.targetName = `${preselectedTarget} Backup`
        break
      case 'stack':
        updates.stackName = preselectedTarget ?? ''
        updates.targetName = `${preselectedTarget} Stack Backup`
        break
      case 'path':
        updates.hostPath = preselectedTarget ?? ''
        updates.targetName = `${preselectedTarget?.split('/').pop() || preselectedTarget} Backup`
        break
    }
    setData((prev) => ({ ...prev, ...updates }))
    // Skip to step 2 (or 4 for volume/path which skip steps 2+3)
    if (preselectedType === 'volume' || preselectedType === 'path') {
      setCurrentStep(4)
    } else {
      setCurrentStep(2)
    }
  }

  // Fetch existing targets for T3: "Already configured" badge
  const { data: existingTargets = [] } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
    enabled: isOpen,
  })

  // Fetch data for all steps
  const { data: containers = [] } = useQuery({
    queryKey: ['containers'],
    queryFn: () => dockerApi.listContainers().then((r) => r.data),
    enabled: isOpen,
  })

  const { data: volumes = [] } = useQuery({
    queryKey: ['volumes'],
    queryFn: () => dockerApi.listVolumes().then((r) => r.data),
    enabled: isOpen,
  })

  const { data: stacks = [] } = useQuery({
    queryKey: ['stacks'],
    queryFn: () => dockerApi.listStacks().then((r) => r.data),
    enabled: isOpen,
  })

  const { data: schedules = [] } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
    enabled: isOpen,
  })

  const { data: retentionPolicies = [] } = useQuery({
    queryKey: ['retention-policies'],
    queryFn: () => retentionApi.listPolicies().then((r) => r.data),
    enabled: isOpen,
  })

  const { data: remoteStorages = [] } = useQuery({
    queryKey: ['remote-storages'],
    queryFn: () => storageApi.list().then((r) => r.data),
    enabled: isOpen,
  })

  // Create schedule mutation (if new schedule needed)
  const createScheduleMutation = useMutation({
    mutationFn: (scheduleData: { name: string; cron_expression: string; description?: string }) =>
      schedulesApi.create(scheduleData),
  })

  // Create retention policy mutation (if new policy needed)
  const createRetentionPolicyMutation = useMutation({
    mutationFn: (policyData: Partial<import('../../api').RetentionPolicy>) =>
      retentionApi.createPolicy(policyData),
  })

  // Create target mutation
  const createTargetMutation = useMutation({
    mutationFn: (targetData: Parameters<typeof targetsApi.create>[0]) =>
      targetsApi.create(targetData),
    onSuccess: () => {
      toast.success('Backup target created successfully!')
      clearDraft()
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      handleClose()
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to create backup target')
    },
  })

  // Update target mutation
  const updateTargetMutation = useMutation({
    mutationFn: (targetData: Parameters<typeof targetsApi.update>[1]) =>
      targetsApi.update(editTarget!.id, targetData),
    onSuccess: () => {
      toast.success('Backup target updated successfully!')
      clearDraft()
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      handleClose()
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to update backup target')
    },
  })

  const handleClose = () => {
    // W2: Save draft if user has made progress (has selected a target type)
    if (!isEditing && data.targetType) {
      saveDraft(data, currentStep)
    }
    setCurrentStep(1)
    setData(initialData)
    setLastEditId(null)
    setLastPreselection(null)
    setShowDraftBanner(false)
    onClose()
  }

  const handleDiscardDraft = () => {
    clearDraft()
    setShowDraftBanner(false)
  }

  const handleRestoreDraft = () => {
    const draft = loadDraft()
    if (draft) {
      setData(draft.data)
      setCurrentStep(draft.currentStep)
      clearDraft()
    }
    setShowDraftBanner(false)
  }

  const updateData = (updates: Partial<WizardData>) => {
    setData((prev) => {
      const next = { ...prev, ...updates }
      // W2: Auto-save draft to localStorage (only for new targets)
      if (!isEditing) {
        saveDraft(next, currentStep)
      }
      return next
    })
  }

  const canProceed = (): boolean => {
    switch (currentStep) {
      case 1: // Target
        if (!data.targetType) return false
        if (data.targetType === 'container' && !data.containerName) return false
        if (data.targetType === 'volume' && !data.volumeName) return false
        if (data.targetType === 'path' && !data.hostPath) return false
        if (data.targetType === 'stack' && !data.stackName) return false
        if (!data.targetName) return false
        return true
      case 2: // Volumes
        return true
      case 3: // Dependencies
        return true
      case 4: // Schedule
        if (data.newSchedule) {
          return !!data.newSchedule.name && !!data.newSchedule.cronExpression
        }
        return true
      case 5: // Storage
        return true
      case 6: // Retention
        if (data.newRetentionPolicy) {
          return !!data.newRetentionPolicy.name
        }
        return true
      case 7: // Options
        return true
      case 8: // Summary
        return true
      default:
        return false
    }
  }

  // Steps 2 (Volumes) and 3 (Dependencies) are irrelevant for volume/path targets
  const shouldSkipStep = useCallback((stepId: number): boolean => {
    if (stepId === 2 || stepId === 3) {
      return data.targetType === 'volume' || data.targetType === 'path'
    }
    return false
  }, [data.targetType])

  const getNextStep = useCallback((from: number): number => {
    let next = from + 1
    while (next <= STEPS.length && shouldSkipStep(next)) next++
    return next
  }, [shouldSkipStep])

  const getPrevStep = useCallback((from: number): number => {
    let prev = from - 1
    while (prev >= 1 && shouldSkipStep(prev)) prev--
    return prev
  }, [shouldSkipStep])

  const handleNext = () => {
    if (currentStep < STEPS.length && canProceed()) {
      setCurrentStep(getNextStep(currentStep))
    }
  }

  const handlePrev = () => {
    if (currentStep > 1) {
      setCurrentStep(getPrevStep(currentStep))
    }
  }

  const isLoading = createScheduleMutation.isPending || createRetentionPolicyMutation.isPending || createTargetMutation.isPending || updateTargetMutation.isPending

  const handleSubmit = async () => {
    try {
      let scheduleId = data.scheduleId

      // Create new schedule if specified
      if (data.newSchedule) {
        const newSchedule = await createScheduleMutation.mutateAsync({
          name: data.newSchedule.name,
          cron_expression: data.newSchedule.cronExpression,
          description: data.newSchedule.description || undefined,
        })
        scheduleId = newSchedule.data.id
      }

      // Create new retention policy if specified
      let retentionPolicyId = data.retentionPolicyId
      if (data.newRetentionPolicy) {
        const newPolicy = await createRetentionPolicyMutation.mutateAsync({
          name: data.newRetentionPolicy.name,
          keep_last: data.newRetentionPolicy.keepLast,
          keep_daily: data.newRetentionPolicy.keepDaily,
          keep_weekly: data.newRetentionPolicy.keepWeekly,
          keep_monthly: data.newRetentionPolicy.keepMonthly,
        })
        retentionPolicyId = newPolicy.data.id
      }

      // Build target data
      const targetData: Parameters<typeof targetsApi.create>[0] = {
        name: data.targetName,
        target_type: data.targetType!,
        container_name: data.targetType === 'container' ? data.containerName : undefined,
        volume_name: data.targetType === 'volume' ? data.volumeName : undefined,
        host_path: data.targetType === 'path' ? data.hostPath : undefined,
        stack_name: data.targetType === 'stack' ? data.stackName : undefined,
        schedule_id: scheduleId || undefined,
        enabled: data.enabled,
        dependencies: data.dependencies,
        // Volume selection and path filtering
        selected_volumes: data.selectedVolumes,
        include_paths: data.includePaths,
        exclude_paths: data.excludePaths,
        per_volume_rules: Object.fromEntries(
          Object.entries(data.perVolumeRules)
            .filter(([, v]) => v.includePaths.length > 0 || v.excludePaths.length > 0)
            .map(([k, v]) => [
              k,
              { include_paths: v.includePaths, exclude_paths: v.excludePaths },
            ])
        ),
        stop_container: data.stopContainer,
        compression_enabled: data.compression !== 'none',
        pre_backup_command: data.preCommand || undefined,
        post_backup_command: data.postCommand || undefined,
        retention_policy_id: retentionPolicyId || undefined,
        remote_storage_ids: data.remoteStorageIds,
      }

      if (isEditing) {
        await updateTargetMutation.mutateAsync(targetData)
      } else {
        await createTargetMutation.mutateAsync(targetData)
      }
    } catch {
      // Error handled in mutation
    }
  }

  // Keyboard navigation
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        handleClose()
        return
      }
      if (e.key === 'Enter') {
        const tag = (e.target as HTMLElement)?.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
        e.preventDefault()
        if (currentStep === STEPS.length) {
          if (canProceed() && !isLoading) handleSubmit()
        } else {
          handleNext()
        }
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  })

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={handleClose} />

      {/* Modal */}
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 w-full max-w-5xl max-h-[90vh] overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-dark-700">
          <div>
            <h2 className="text-xl font-bold text-dark-100">{isEditing ? 'Edit Backup Target' : 'Create Backup Target'}</h2>
            <p className="text-sm text-dark-400">
              Step {currentStep} of {STEPS.length}: {STEPS[currentStep - 1].name}
            </p>
          </div>
          <button
            onClick={handleClose}
            className="p-2 text-dark-400 hover:text-dark-200 hover:bg-dark-700 rounded-lg transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Progress Steps */}
        <div className="px-6 py-4 border-b border-dark-700">
          <div className="flex items-center justify-between">
            {STEPS.map((step, index) => {
              const skipped = shouldSkipStep(step.id)
              const isVisited = isEditing || step.id < currentStep
              const canClick = isEditing ? !skipped : (step.id < currentStep && !skipped)
              return (
                <div key={step.id} className="flex items-center">
                  <div className="flex flex-col items-center">
                    <button
                      onClick={() => canClick && setCurrentStep(step.id)}
                      disabled={!canClick && step.id !== currentStep}
                      title={skipped ? `${step.name} (skipped)` : step.name}
                      className={`flex items-center justify-center rounded-full font-medium transition-colors ${
                        skipped
                          ? 'w-5 h-5 text-[10px] bg-dark-700/50 text-dark-500 cursor-default'
                          : step.id === currentStep
                          ? 'w-8 h-8 text-sm bg-primary-500 text-white'
                          : isVisited
                          ? 'w-8 h-8 text-sm bg-green-500 text-white cursor-pointer'
                          : 'w-8 h-8 text-sm bg-dark-700 text-dark-400'
                      }`}
                    >
                      {skipped ? '·' : isVisited && step.id !== currentStep ? <Check className="w-4 h-4" /> : step.id}
                    </button>
                    <span className={`text-[10px] mt-1 ${
                      skipped ? 'text-dark-600 text-[9px]' : step.id === currentStep ? 'text-primary-400' : 'text-dark-500'
                    }`}>
                      {step.name.length > 6 ? step.name.slice(0, 5) + '.' : step.name}
                    </span>
                  </div>
                  {index < STEPS.length - 1 && (
                    <div
                      className={`w-8 h-0.5 mx-1 ${
                        (isEditing || step.id < currentStep) ? 'bg-green-500' : 'bg-dark-700'
                      }`}
                    />
                  )}
                </div>
              )
            })}
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-6">
          {/* W2: Draft restoration banner */}
          {showDraftBanner && (
            <div className="mb-4 flex items-center gap-3 p-3 bg-primary-500/10 border border-primary-500/30 rounded-lg">
              <div className="flex-1">
                <p className="text-sm font-medium text-primary-300">Continue where you left off?</p>
                <p className="text-xs text-primary-400/70">You have an unsaved wizard draft</p>
              </div>
              <button
                onClick={handleRestoreDraft}
                className="px-3 py-1.5 text-xs bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
              >
                Restore
              </button>
              <button
                onClick={handleDiscardDraft}
                className="px-3 py-1.5 text-xs bg-dark-600 text-dark-300 rounded-lg hover:bg-dark-500 transition-colors"
              >
                Discard
              </button>
            </div>
          )}

          {currentStep === 1 && (
            <StepTargetSelect
              data={data}
              updateData={updateData}
              containers={containers}
              volumes={volumes}
              stacks={stacks}
              existingTargets={existingTargets}
            />
          )}
          {currentStep === 2 && (
            <StepVolumeConfig
              data={data}
              updateData={updateData}
              containers={containers}
              stacks={stacks}
            />
          )}
          {currentStep === 3 && (
            <StepDependencies
              data={data}
              updateData={updateData}
              containers={containers}
              stacks={stacks}
            />
          )}
          {currentStep === 4 && (
            <StepSchedule
              data={data}
              updateData={updateData}
              schedules={schedules}
              isLoadingSchedules={false}
            />
          )}
          {currentStep === 5 && (
            <StepStorage
              data={data}
              updateData={updateData}
              storages={remoteStorages}
              isLoadingStorages={false}
            />
          )}
          {currentStep === 6 && (
            <StepRetention
              data={data}
              updateData={updateData}
              policies={retentionPolicies}
              isLoadingPolicies={false}
            />
          )}
          {currentStep === 7 && (
            <StepOptions data={data} updateData={updateData} />
          )}
          {currentStep === 8 && (
            <StepSummary
              data={data}
              schedules={schedules}
              policies={retentionPolicies}
              storages={remoteStorages}
              isCreating={isLoading}
              onGoToStep={setCurrentStep}
              availableVolumes={(() => {
                if (data.targetType === 'container' && data.containerName) {
                  const c = containers.find((ct) => ct.name === data.containerName)
                  return c?.mounts?.filter((m) => m.type === 'volume').map((m) => m.name || '').filter(Boolean) || []
                }
                if (data.targetType === 'stack' && data.stackName) {
                  return stacks.find((s) => s.name === data.stackName)?.volumes || []
                }
                return []
              })()}
            />
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-6 py-4 border-t border-dark-700">
          <button
            onClick={handlePrev}
            disabled={currentStep === 1}
            className="flex items-center gap-2 px-4 py-2 text-dark-300 hover:text-dark-100 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          >
            <ChevronLeft className="w-4 h-4" />
            Previous
          </button>

          <div className="flex gap-2">
            <button
              onClick={handleClose}
              className="px-4 py-2 text-dark-300 hover:text-dark-100 transition-colors"
            >
              Cancel
            </button>
            {currentStep === STEPS.length ? (
              <button
                onClick={handleSubmit}
                disabled={!canProceed() || isLoading}
                className="flex items-center gap-2 px-6 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              >
                {isLoading ? (
                  <>
                    <Loader2 className="w-4 h-4 animate-spin" />
                    {isEditing ? 'Saving...' : 'Creating...'}
                  </>
                ) : (
                  <>
                    <Check className="w-4 h-4" />
                    {isEditing ? 'Save Changes' : 'Create Target'}
                  </>
                )}
              </button>
            ) : (
              <button
                onClick={handleNext}
                disabled={!canProceed()}
                className="flex items-center gap-2 px-6 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              >
                Next
                <ChevronRight className="w-4 h-4" />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
