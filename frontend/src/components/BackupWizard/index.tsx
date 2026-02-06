import { useState } from 'react'
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

interface BackupWizardProps {
  isOpen: boolean
  onClose: () => void
}

export default function BackupWizard({ isOpen, onClose }: BackupWizardProps) {
  const queryClient = useQueryClient()
  const [currentStep, setCurrentStep] = useState(1)
  const [data, setData] = useState<WizardData>(initialData)

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

  // Create target mutation
  const createTargetMutation = useMutation({
    mutationFn: (targetData: Parameters<typeof targetsApi.create>[0]) =>
      targetsApi.create(targetData),
    onSuccess: () => {
      toast.success('Backup target created successfully!')
      queryClient.invalidateQueries({ queryKey: ['targets'] })
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      handleClose()
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to create backup target')
    },
  })

  const handleClose = () => {
    setCurrentStep(1)
    setData(initialData)
    onClose()
  }

  const updateData = (updates: Partial<WizardData>) => {
    setData((prev) => ({ ...prev, ...updates }))
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
      case 2: // Volumes - Optional for container/stack, auto-skip for volume/path
        return true
      case 3: // Dependencies
        return true // Optional step
      case 4: // Schedule
        return data.scheduleId !== null || data.newSchedule !== null || true // Schedule is optional
      case 5: // Storage
        return true // Optional step
      case 6: // Retention
        return true // Optional step
      case 7: // Options
        return true // Optional step
      case 8: // Summary
        return true
      default:
        return false
    }
  }

  const handleNext = () => {
    if (currentStep < STEPS.length && canProceed()) {
      setCurrentStep((prev) => prev + 1)
    }
  }

  const handlePrev = () => {
    if (currentStep > 1) {
      setCurrentStep((prev) => prev - 1)
    }
  }

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

      // TODO: Create new retention policy if specified
      const retentionPolicyId = data.retentionPolicyId
      // if (data.newRetentionPolicy) {
      //   const newPolicy = await createRetentionPolicyMutation.mutateAsync(...)
      //   retentionPolicyId = newPolicy.data.id
      // }

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
        stop_container: data.stopContainer,
        compression_enabled: data.compression !== 'none',
        pre_backup_command: data.preCommand || undefined,
        post_backup_command: data.postCommand || undefined,
        retention_policy_id: retentionPolicyId || undefined,
      }

      await createTargetMutation.mutateAsync(targetData)
    } catch {
      // Error handled in mutation
    }
  }

  if (!isOpen) return null

  const isLoading = createScheduleMutation.isPending || createTargetMutation.isPending

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={handleClose} />

      {/* Modal */}
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 w-full max-w-4xl max-h-[90vh] overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-dark-700">
          <div>
            <h2 className="text-xl font-bold text-dark-100">Create Backup Target</h2>
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
            {STEPS.map((step, index) => (
              <div key={step.id} className="flex items-center">
                <button
                  onClick={() => step.id < currentStep && setCurrentStep(step.id)}
                  disabled={step.id > currentStep}
                  className={`flex items-center justify-center w-8 h-8 rounded-full text-sm font-medium transition-colors ${
                    step.id === currentStep
                      ? 'bg-primary-500 text-white'
                      : step.id < currentStep
                      ? 'bg-green-500 text-white cursor-pointer'
                      : 'bg-dark-700 text-dark-400'
                  }`}
                >
                  {step.id < currentStep ? <Check className="w-4 h-4" /> : step.id}
                </button>
                {index < STEPS.length - 1 && (
                  <div
                    className={`w-12 h-0.5 mx-2 ${
                      step.id < currentStep ? 'bg-green-500' : 'bg-dark-700'
                    }`}
                  />
                )}
              </div>
            ))}
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-6">
          {currentStep === 1 && (
            <StepTargetSelect
              data={data}
              updateData={updateData}
              containers={containers}
              volumes={volumes}
              stacks={stacks}
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
                    Creating...
                  </>
                ) : (
                  <>
                    <Check className="w-4 h-4" />
                    Create Target
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
