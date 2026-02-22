import {
  Check,
  Target,
  Link2,
  Calendar,
  Cloud,
  Archive,
  Settings,
  AlertCircle,
  Database,
  Pencil,
} from 'lucide-react'
import { WizardData } from './index'
import { ScheduleEntity, RemoteStorage, RetentionPolicy } from '../../api'

interface Props {
  data: WizardData
  schedules: ScheduleEntity[]
  storages: RemoteStorage[]
  policies: RetentionPolicy[]
  isCreating: boolean
  onGoToStep?: (step: number) => void
  availableVolumes?: string[]
}

// Section component for summary display
function SummarySection({
  icon: Icon,
  title,
  children,
  status = 'configured',
  onEdit,
}: {
  icon: React.ElementType
  title: string
  children: React.ReactNode
  status?: 'configured' | 'optional' | 'warning'
  onEdit?: () => void
}) {
  const statusColors = {
    configured: 'text-green-400',
    optional: 'text-dark-400',
    warning: 'text-yellow-400',
  }

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-4">
      <div className="flex items-start gap-3">
        <div className="p-2 bg-dark-700 rounded-lg">
          <Icon className={`w-5 h-5 ${statusColors[status]}`} />
        </div>
        <div className="flex-1 min-w-0">
          <h4 className="text-dark-100 font-medium">{title}</h4>
          <div className="mt-2 text-sm">{children}</div>
        </div>
        <div className="flex items-center gap-2">
          {onEdit && (
            <button
              onClick={onEdit}
              className="p-1.5 text-dark-500 hover:text-primary-400 hover:bg-dark-700 rounded-lg transition-colors"
              title={`Edit ${title}`}
            >
              <Pencil className="w-3.5 h-3.5" />
            </button>
          )}
          {status === 'configured' && <Check className="w-5 h-5 text-green-400" />}
          {status === 'warning' && <AlertCircle className="w-5 h-5 text-yellow-400" />}
        </div>
      </div>
    </div>
  )
}

export default function StepSummary({ data, schedules, storages, policies, isCreating, onGoToStep, availableVolumes = [] }: Props) {
  // Get referenced entities
  const selectedSchedule = data.scheduleId
    ? schedules.find((s) => s.id === data.scheduleId)
    : null
  const selectedStorages = storages.filter((s) => data.remoteStorageIds.includes(s.id))
  const selectedPolicy = data.retentionPolicyId
    ? policies.find((p) => p.id === data.retentionPolicyId)
    : null

  // Format target type display
  const formatTargetType = () => {
    switch (data.targetType) {
      case 'container':
        return `Container: ${data.containerName || 'Not selected'}`
      case 'volume':
        return `Volume: ${data.volumeName || 'Not selected'}`
      case 'stack':
        return `Stack: ${data.stackName || 'Not selected'}`
      case 'path':
        return `Path: ${data.hostPath || 'Not specified'}`
      default:
        return 'Not configured'
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold text-dark-100 mb-2">Review & Create</h3>
        <p className="text-sm text-dark-400">
          Review your backup configuration before creating
        </p>
      </div>

      {/* Loading overlay */}
      {isCreating && (
        <div className="absolute inset-0 bg-dark-900/80 flex items-center justify-center z-10 rounded-2xl">
          <div className="text-center">
            <div className="animate-spin rounded-full h-12 w-12 border-2 border-primary-500 border-t-transparent mx-auto mb-4" />
            <p className="text-dark-300">Creating backup target...</p>
          </div>
        </div>
      )}

      {/* Target Info */}
      <SummarySection icon={Target} title="Backup Target" status="configured" onEdit={onGoToStep ? () => onGoToStep(1) : undefined}>
        <p className="text-dark-300">
          <span className="font-medium text-dark-100">{data.targetName || 'Unnamed'}</span>
        </p>
        <p className="text-dark-400 mt-1">{formatTargetType()}</p>
      </SummarySection>

      {/* Volumes */}
      {(data.targetType === 'container' || data.targetType === 'stack') && (
        <SummarySection
          icon={Database}
          title="Volume Configuration"
          status="configured"
          onEdit={onGoToStep ? () => onGoToStep(2) : undefined}
        >
          {(() => {
            const totalVolumes = availableVolumes.length
            const selectedCount = data.selectedVolumes.length === 0 ? totalVolumes : data.selectedVolumes.length
            const customRuleCount = Object.values(data.perVolumeRules).filter(
              (r) => r.includePaths.length > 0 || r.excludePaths.length > 0
            ).length
            return (
              <div className="text-dark-300">
                <p>
                  Volumes: <span className="text-dark-100">{selectedCount}/{totalVolumes}</span> selected
                  {customRuleCount > 0 && (
                    <span className="text-purple-400 ml-2">
                      ({customRuleCount} with custom rules)
                    </span>
                  )}
                </p>
                {(data.includePaths.length > 0 || data.excludePaths.length > 0) && (
                  <p className="text-dark-400 mt-1">
                    {data.includePaths.length > 0 && `${data.includePaths.length} include filter(s)`}
                    {data.includePaths.length > 0 && data.excludePaths.length > 0 && ', '}
                    {data.excludePaths.length > 0 && `${data.excludePaths.length} exclude filter(s)`}
                  </p>
                )}
              </div>
            )
          })()}
        </SummarySection>
      )}

      {/* Dependencies */}
      <SummarySection
        icon={Link2}
        title="Container Dependencies"
        status={data.dependencies.length > 0 ? 'configured' : 'optional'}
        onEdit={onGoToStep ? () => onGoToStep(3) : undefined}
      >
        {data.stopContainer ? (
          <div className="text-dark-300">
            <p>Stop containers during backup: <span className="text-green-400">Yes</span></p>
            {data.dependencies.length > 0 && (
              <p className="mt-1">
                Dependencies: <span className="text-dark-100">{data.dependencies.join(', ')}</span>
              </p>
            )}
          </div>
        ) : (
          <p className="text-dark-400">Containers will not be stopped</p>
        )}
      </SummarySection>

      {/* Schedule */}
      <SummarySection
        icon={Calendar}
        title="Schedule"
        status={selectedSchedule || data.newSchedule ? 'configured' : 'warning'}
        onEdit={onGoToStep ? () => onGoToStep(4) : undefined}
      >
        {selectedSchedule ? (
          <div className="text-dark-300">
            <p className="text-dark-100">{selectedSchedule.name}</p>
            <p className="text-dark-400 font-mono">{selectedSchedule.cron_expression}</p>
          </div>
        ) : data.newSchedule ? (
          <div className="text-dark-300">
            <p className="text-dark-100">{data.newSchedule.name || 'New Schedule'}</p>
            <p className="text-dark-400 font-mono">{data.newSchedule.cronExpression}</p>
          </div>
        ) : (
          <p className="text-yellow-400">No schedule - manual backups only</p>
        )}
      </SummarySection>

      {/* Remote Storage */}
      <SummarySection
        icon={Cloud}
        title="Remote Storage"
        status={selectedStorages.length > 0 ? 'configured' : 'optional'}
        onEdit={onGoToStep ? () => onGoToStep(5) : undefined}
      >
        {selectedStorages.length > 0 ? (
          <ul className="space-y-1">
            {selectedStorages.map((storage) => (
              <li key={storage.id} className="text-dark-300">
                • {storage.name}{' '}
                <span className="text-dark-500">({storage.storage_type})</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-dark-400">Local storage only</p>
        )}
      </SummarySection>

      {/* Retention */}
      <SummarySection
        icon={Archive}
        title="Retention Policy"
        status={selectedPolicy || data.newRetentionPolicy ? 'configured' : 'warning'}
        onEdit={onGoToStep ? () => onGoToStep(6) : undefined}
      >
        {selectedPolicy ? (
          <div className="text-dark-300">
            <p className="text-dark-100">{selectedPolicy.name}</p>
            <div className="flex gap-4 mt-1 text-dark-400">
              {selectedPolicy.keep_last > 0 && <span>Last: {selectedPolicy.keep_last}</span>}
              {selectedPolicy.keep_daily > 0 && <span>Daily: {selectedPolicy.keep_daily}</span>}
              {selectedPolicy.keep_weekly > 0 && <span>Weekly: {selectedPolicy.keep_weekly}</span>}
              {selectedPolicy.keep_monthly > 0 && <span>Monthly: {selectedPolicy.keep_monthly}</span>}
              {selectedPolicy.keep_yearly > 0 && <span>Yearly: {selectedPolicy.keep_yearly}</span>}
            </div>
          </div>
        ) : data.newRetentionPolicy ? (
          <div className="text-dark-300">
            <p className="text-dark-100">{data.newRetentionPolicy.name || 'New Policy'}</p>
            <div className="flex gap-4 mt-1 text-dark-400">
              {data.newRetentionPolicy.keepLast > 0 && (
                <span>Last: {data.newRetentionPolicy.keepLast}</span>
              )}
              {data.newRetentionPolicy.keepDaily > 0 && (
                <span>Daily: {data.newRetentionPolicy.keepDaily}</span>
              )}
              {data.newRetentionPolicy.keepWeekly > 0 && (
                <span>Weekly: {data.newRetentionPolicy.keepWeekly}</span>
              )}
              {data.newRetentionPolicy.keepMonthly > 0 && (
                <span>Monthly: {data.newRetentionPolicy.keepMonthly}</span>
              )}
            </div>
          </div>
        ) : (
          <p className="text-yellow-400">No retention policy - keeping all backups</p>
        )}
      </SummarySection>

      {/* Options */}
      <SummarySection icon={Settings} title="Options" status="configured" onEdit={onGoToStep ? () => onGoToStep(7) : undefined}>
        <div className="grid grid-cols-2 gap-2 text-dark-300">
          <div>
            Compression:{' '}
            <span className="text-dark-100 capitalize">{data.compression}</span>
          </div>
          <div>
            Enabled:{' '}
            <span className={data.enabled ? 'text-green-400' : 'text-dark-500'}>
              {data.enabled ? 'Yes' : 'No'}
            </span>
          </div>
          {data.preCommand && (
            <div className="col-span-2 truncate">
              Pre-command: <span className="text-dark-400 font-mono">{data.preCommand}</span>
            </div>
          )}
          {data.postCommand && (
            <div className="col-span-2 truncate">
              Post-command: <span className="text-dark-400 font-mono">{data.postCommand}</span>
            </div>
          )}
        </div>
      </SummarySection>
    </div>
  )
}
