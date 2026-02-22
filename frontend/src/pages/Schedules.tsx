import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Clock, Plus, Edit2, Trash2, HelpCircle, Calendar, Target, PanelRightClose, PanelRightOpen } from 'lucide-react'
import { schedulesApi, targetsApi, ScheduleEntity, ScheduleCreate, ScheduleUpdate, BackupTarget } from '../api'
import { formatDistanceToNow } from 'date-fns'
import toast from 'react-hot-toast'
import { useState } from 'react'
import { clsx } from 'clsx'
import ConfirmDialog from '../components/ConfirmDialog'
import EmptyState from '../components/EmptyState'
import LoadingSkeleton from '../components/LoadingSkeleton'

interface ScheduleFormData {
  name: string
  cron_expression: string
  description: string
  enabled: boolean
}

const defaultFormData: ScheduleFormData = {
  name: '',
  cron_expression: '0 2 * * *',
  description: '',
  enabled: true,
}

function ScheduleForm({
  initialData,
  onSubmit,
  onCancel,
  isLoading,
}: {
  initialData?: ScheduleFormData
  onSubmit: (data: ScheduleFormData) => void
  onCancel: () => void
  isLoading: boolean
}) {
  const [formData, setFormData] = useState<ScheduleFormData>(initialData || defaultFormData)

  return (
    <div className="space-y-4">
      <div>
        <label className="block text-sm font-medium text-dark-300 mb-1">Name</label>
        <input
          type="text"
          value={formData.name}
          onChange={(e) => setFormData({ ...formData, name: e.target.value })}
          className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
          placeholder="Daily Backup"
        />
      </div>

      <div>
        <label className="block text-sm font-medium text-dark-300 mb-1">Cron Expression</label>
        <input
          type="text"
          value={formData.cron_expression}
          onChange={(e) => setFormData({ ...formData, cron_expression: e.target.value })}
          className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100 font-mono"
          placeholder="0 2 * * *"
        />
        <p className="text-xs text-dark-500 mt-1">minute hour day month weekday</p>
      </div>

      <div>
        <label className="block text-sm font-medium text-dark-300 mb-1">Description</label>
        <input
          type="text"
          value={formData.description}
          onChange={(e) => setFormData({ ...formData, description: e.target.value })}
          className="w-full px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-dark-100"
          placeholder="Every day at 2:00 AM"
        />
      </div>

      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          id="enabled"
          checked={formData.enabled}
          onChange={(e) => setFormData({ ...formData, enabled: e.target.checked })}
          className="w-4 h-4 rounded border-dark-600 text-primary-500 focus:ring-primary-500 bg-dark-700"
        />
        <label htmlFor="enabled" className="text-sm text-dark-300">
          Enabled
        </label>
      </div>

      <div className="flex gap-2 pt-2">
        <button
          onClick={() => onSubmit(formData)}
          disabled={isLoading || !formData.name || !formData.cron_expression}
          className="flex-1 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
          {isLoading ? 'Saving...' : 'Save'}
        </button>
        <button
          onClick={onCancel}
          className="px-4 py-2 bg-dark-600 text-dark-300 rounded-lg hover:bg-dark-500 transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

function ScheduleCard({
  schedule,
  onEdit,
  targets,
}: {
  schedule: ScheduleEntity
  onEdit: () => void
  targets: BackupTarget[]
}) {
  const queryClient = useQueryClient()
  const [showConfirm, setShowConfirm] = useState(false)

  // SC1: Find targets that use this schedule
  const usedByTargets = targets.filter((t) => t.schedule_id === schedule.id)

  const deleteMutation = useMutation({
    mutationFn: () => schedulesApi.delete(schedule.id),
    onSuccess: () => {
      toast.success('Schedule deleted')
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
    },
    onError: () => toast.error('Failed to delete schedule'),
  })

  const toggleMutation = useMutation({
    mutationFn: () => schedulesApi.update(schedule.id, { enabled: !schedule.enabled }),
    onSuccess: () => {
      toast.success(`Schedule ${schedule.enabled ? 'disabled' : 'enabled'}`)
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
    },
    onError: () => toast.error('Failed to update schedule'),
  })

  return (
    <div className={`bg-dark-800 rounded-xl border border-dark-700 p-6 ${!schedule.enabled ? 'opacity-60' : ''}`}>
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-orange-500/10 rounded-lg flex items-center justify-center">
            <Calendar className="w-5 h-5 text-orange-400" />
          </div>
          <div>
            <h3 className="text-lg font-semibold text-dark-100">{schedule.name}</h3>
            {schedule.description && (
              <p className="text-sm text-dark-400">{schedule.description}</p>
            )}
          </div>
        </div>
        <button
          onClick={() => toggleMutation.mutate()}
          className={`px-2 py-1 rounded-full text-xs font-medium ${
            schedule.enabled
              ? 'bg-green-500/10 text-green-400'
              : 'bg-dark-600 text-dark-400'
          }`}
        >
          {schedule.enabled ? 'Active' : 'Inactive'}
        </button>
      </div>

      {/* Cron Expression */}
      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-1">Cron Expression:</p>
        <code className="px-3 py-2 bg-dark-700 rounded-lg text-sm font-mono text-primary-400 block">
          {schedule.cron_expression}
        </code>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-2 gap-4 mb-4 text-sm">
        <div>
          <p className="text-dark-400">Next run:</p>
          <p className="text-dark-200">
            {schedule.next_run
              ? formatDistanceToNow(new Date(schedule.next_run), { addSuffix: true })
              : '-'}
          </p>
        </div>
        <div>
          <p className="text-dark-400">Targets:</p>
          <div className="flex items-center gap-1 text-dark-200">
            <Target className="w-3 h-3" />
            {schedule.target_count}
          </div>
          {/* SC1: Show target names */}
          {usedByTargets.length > 0 && (
            <div className="mt-1 space-y-0.5">
              {usedByTargets.slice(0, 3).map((t) => (
                <p key={t.id} className="text-xs text-dark-500 truncate" title={t.name}>
                  {t.name}
                </p>
              ))}
              {usedByTargets.length > 3 && (
                <p className="text-xs text-dark-600">+{usedByTargets.length - 3} more</p>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Actions */}
      <div className="flex gap-2">
        <button
          onClick={onEdit}
          className="flex items-center justify-center gap-2 flex-1 px-3 py-2 bg-dark-700 text-dark-300 rounded-lg hover:bg-dark-600 transition-colors text-sm"
        >
          <Edit2 className="w-4 h-4" />
          Edit
        </button>
        <button
          onClick={() => setShowConfirm(true)}
          disabled={deleteMutation.isPending}
          className="flex items-center justify-center gap-2 px-3 py-2 bg-red-500/10 text-red-400 rounded-lg hover:bg-red-500/20 transition-colors text-sm"
        >
          <Trash2 className="w-4 h-4" />
        </button>
      </div>

      <ConfirmDialog
        isOpen={showConfirm}
        onClose={() => setShowConfirm(false)}
        onConfirm={() => {
          deleteMutation.mutate()
          setShowConfirm(false)
        }}
        title="Delete Schedule"
        message={`Delete schedule "${schedule.name}"? Targets using it will be unlinked.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deleteMutation.isPending}
      />
    </div>
  )
}

function CronHelp() {
  const { data: help } = useQuery({
    queryKey: ['cron-help'],
    queryFn: () => schedulesApi.getCronHelp().then((r) => r.data),
  })

  if (!help) return null

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-center gap-2 mb-4">
        <HelpCircle className="w-5 h-5 text-primary-400" />
        <h2 className="text-lg font-semibold text-dark-100">Cron Help</h2>
      </div>

      <p className="text-sm text-dark-400 mb-4">Format: {help.format}</p>

      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-2">Examples:</p>
        <div className="space-y-2">
          {help.examples.map((ex: { expression: string; description: string }, i: number) => (
            <div key={i} className="flex items-center gap-4 text-sm">
              <code className="px-2 py-1 bg-dark-700 rounded text-primary-400 font-mono">
                {ex.expression}
              </code>
              <span className="text-dark-300">{ex.description}</span>
            </div>
          ))}
        </div>
      </div>

      <div>
        <p className="text-xs text-dark-400 mb-2">Special characters:</p>
        <div className="grid grid-cols-2 gap-2 text-sm">
          {Object.entries(help.special).map(([symbol, desc]) => (
            <div key={symbol} className="flex items-center gap-2">
              <code className="px-2 py-1 bg-dark-700 rounded text-primary-400 font-mono">
                {symbol}
              </code>
              <span className="text-dark-400">{desc as string}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

export default function Schedules() {
  const queryClient = useQueryClient()
  const [showForm, setShowForm] = useState(false)
  const [editingSchedule, setEditingSchedule] = useState<ScheduleEntity | null>(null)
  const [showCronHelp, setShowCronHelp] = useState(false)

  const { data: schedules, isLoading } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })

  // SC1: Fetch targets to show which targets use each schedule
  const { data: targets = [] } = useQuery({
    queryKey: ['targets'],
    queryFn: () => targetsApi.list().then((r) => r.data),
  })

  const createMutation = useMutation({
    mutationFn: (data: ScheduleCreate) => schedulesApi.create(data),
    onSuccess: () => {
      toast.success('Schedule created')
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      setShowForm(false)
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to create schedule')
    },
  })

  const updateMutation = useMutation({
    mutationFn: ({ id, data }: { id: number; data: ScheduleUpdate }) =>
      schedulesApi.update(id, data),
    onSuccess: () => {
      toast.success('Schedule updated')
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      setEditingSchedule(null)
    },
    onError: (err: Error & { response?: { data?: { detail?: string } } }) => {
      toast.error(err.response?.data?.detail || 'Failed to update schedule')
    },
  })

  const handleSubmit = (data: ScheduleFormData) => {
    if (editingSchedule) {
      updateMutation.mutate({
        id: editingSchedule.id,
        data: {
          name: data.name,
          cron_expression: data.cron_expression,
          description: data.description || undefined,
          enabled: data.enabled,
        },
      })
    } else {
      createMutation.mutate({
        name: data.name,
        cron_expression: data.cron_expression,
        description: data.description || undefined,
        enabled: data.enabled,
      })
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-100">Schedules</h1>
          <p className="text-dark-400 mt-1">Reusable backup schedules for targets</p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => setShowCronHelp(!showCronHelp)}
            className={clsx(
              'flex items-center gap-2 px-3 py-2 rounded-lg transition-colors border',
              showCronHelp
                ? 'bg-primary-500/10 border-primary-500/50 text-primary-400'
                : 'bg-dark-800 border-dark-700 text-dark-400 hover:text-dark-300'
            )}
            title={showCronHelp ? 'Hide cron help' : 'Show cron help'}
          >
            {showCronHelp ? (
              <PanelRightClose className="w-4 h-4" />
            ) : (
              <PanelRightOpen className="w-4 h-4" />
            )}
            <HelpCircle className="w-4 h-4" />
          </button>
          <button
            onClick={() => {
              setEditingSchedule(null)
              setShowForm(true)
            }}
            className="flex items-center gap-2 px-4 py-2 bg-primary-500 text-white rounded-lg hover:bg-primary-600 transition-colors"
          >
            <Plus className="w-4 h-4" />
            New Schedule
          </button>
        </div>
      </div>

      <div className={clsx('grid gap-6', showCronHelp ? 'grid-cols-1 lg:grid-cols-3' : 'grid-cols-1')}>
        <div className={clsx(showCronHelp ? 'lg:col-span-2' : '', 'space-y-6')}>
          {/* Schedule List */}
          {isLoading ? (
            <LoadingSkeleton count={4} layout="grid-2" />
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              {[...(schedules ?? [])].sort((a, b) => {
                // SC4: Active first, then inactive
                if (a.enabled !== b.enabled) return a.enabled ? -1 : 1
                return a.name.localeCompare(b.name)
              }).map((schedule) => (
                <ScheduleCard
                  key={schedule.id}
                  schedule={schedule}
                  targets={targets}
                  onEdit={() => {
                    setShowForm(false)
                    setEditingSchedule(schedule)
                  }}
                />
              ))}
            </div>
          )}

          {!isLoading && schedules?.length === 0 && !showForm && (
            <EmptyState
              icon={Clock}
              title="No schedules configured"
              description="Create a schedule and assign it to backup targets"
              action={{ label: 'Create Schedule', onClick: () => setShowForm(true), icon: Plus }}
            />
          )}
        </div>

        {showCronHelp && (
          <div>
            <CronHelp />
          </div>
        )}
      </div>

      {/* Schedule Form Modal */}
      {(showForm || editingSchedule) && (
        <div className="fixed inset-0 z-50 flex items-center justify-center">
          <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={() => { setShowForm(false); setEditingSchedule(null) }} />
          <div className="relative bg-dark-800 rounded-2xl border border-dark-700 shadow-2xl w-full max-w-lg p-6">
            <h2 className="text-lg font-semibold text-dark-100 mb-4">
              {editingSchedule ? 'Edit Schedule' : 'New Schedule'}
            </h2>
            <ScheduleForm
              initialData={
                editingSchedule
                  ? {
                      name: editingSchedule.name,
                      cron_expression: editingSchedule.cron_expression,
                      description: editingSchedule.description || '',
                      enabled: editingSchedule.enabled,
                    }
                  : undefined
              }
              onSubmit={handleSubmit}
              onCancel={() => {
                setShowForm(false)
                setEditingSchedule(null)
              }}
              isLoading={createMutation.isPending || updateMutation.isPending}
            />
          </div>
        </div>
      )}
    </div>
  )
}
