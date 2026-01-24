import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { Clock, Play, HelpCircle } from 'lucide-react'
import { schedulesApi, Schedule } from '../api'
import { formatDistanceToNow } from 'date-fns'
import { de } from 'date-fns/locale'
import toast from 'react-hot-toast'
import { useState } from 'react'

function ScheduleCard({ schedule }: { schedule: Schedule }) {
  const queryClient = useQueryClient()
  const [editCron, setEditCron] = useState(false)
  const [cronValue, setCronValue] = useState(schedule.cron_expression)

  const triggerMutation = useMutation({
    mutationFn: () => schedulesApi.trigger(schedule.target_id),
    onSuccess: () => {
      toast.success('Backup gestartet')
      queryClient.invalidateQueries({ queryKey: ['backups'] })
    },
    onError: () => toast.error('Fehler beim Starten'),
  })

  const updateMutation = useMutation({
    mutationFn: (data: { cron_expression?: string; enabled?: boolean }) =>
      schedulesApi.update(schedule.target_id, data),
    onSuccess: () => {
      toast.success('Schedule aktualisiert')
      queryClient.invalidateQueries({ queryKey: ['schedules'] })
      setEditCron(false)
    },
    onError: () => toast.error('Fehler beim Aktualisieren'),
  })

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-orange-500/10 rounded-lg flex items-center justify-center">
            <Clock className="w-5 h-5 text-orange-400" />
          </div>
          <div>
            <h3 className="text-lg font-semibold text-dark-100">{schedule.target_name}</h3>
            <p className="text-sm text-dark-400">Target #{schedule.target_id}</p>
          </div>
        </div>
        <button
          onClick={() => updateMutation.mutate({ enabled: !schedule.enabled })}
          className={`px-2 py-1 rounded-full text-xs font-medium ${
            schedule.enabled
              ? 'bg-green-500/10 text-green-400'
              : 'bg-dark-600 text-dark-400'
          }`}
        >
          {schedule.enabled ? 'Aktiv' : 'Inaktiv'}
        </button>
      </div>

      {/* Cron Expression */}
      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-2">Cron Expression:</p>
        {editCron ? (
          <div className="flex gap-2">
            <input
              type="text"
              value={cronValue}
              onChange={(e) => setCronValue(e.target.value)}
              className="flex-1 px-3 py-2 bg-dark-700 border border-dark-600 rounded-lg text-sm text-dark-100 font-mono"
              placeholder="0 2 * * *"
            />
            <button
              onClick={() => updateMutation.mutate({ cron_expression: cronValue })}
              className="px-3 py-2 bg-primary-500 text-white rounded-lg text-sm"
            >
              Speichern
            </button>
            <button
              onClick={() => {
                setEditCron(false)
                setCronValue(schedule.cron_expression)
              }}
              className="px-3 py-2 bg-dark-600 text-dark-300 rounded-lg text-sm"
            >
              Abbrechen
            </button>
          </div>
        ) : (
          <button
            onClick={() => setEditCron(true)}
            className="px-3 py-2 bg-dark-700 rounded-lg text-sm font-mono text-dark-200 hover:bg-dark-600 transition-colors"
          >
            {schedule.cron_expression}
          </button>
        )}
      </div>

      {/* Next/Last Run */}
      <div className="grid grid-cols-2 gap-4 mb-4 text-sm">
        <div>
          <p className="text-dark-400">Nächste Ausführung:</p>
          <p className="text-dark-200">
            {schedule.next_run
              ? formatDistanceToNow(new Date(schedule.next_run), {
                  addSuffix: true,
                  locale: de,
                })
              : '-'}
          </p>
        </div>
        <div>
          <p className="text-dark-400">Letzte Ausführung:</p>
          <p className="text-dark-200">
            {schedule.last_run
              ? formatDistanceToNow(new Date(schedule.last_run), {
                  addSuffix: true,
                  locale: de,
                })
              : 'Noch nie'}
          </p>
        </div>
      </div>

      {/* Actions */}
      <button
        onClick={() => triggerMutation.mutate()}
        disabled={triggerMutation.isPending}
        className="flex items-center justify-center gap-2 w-full px-3 py-2 bg-primary-500/10 text-primary-400 rounded-lg hover:bg-primary-500/20 transition-colors text-sm"
      >
        <Play className="w-4 h-4" />
        Jetzt ausführen
      </button>
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
        <h2 className="text-lg font-semibold text-dark-100">Cron Hilfe</h2>
      </div>

      <p className="text-sm text-dark-400 mb-4">Format: {help.format}</p>

      <div className="mb-4">
        <p className="text-xs text-dark-400 mb-2">Beispiele:</p>
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
        <p className="text-xs text-dark-400 mb-2">Spezielle Zeichen:</p>
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
  const { data: schedules, isLoading } = useQuery({
    queryKey: ['schedules'],
    queryFn: () => schedulesApi.list().then((r) => r.data),
  })

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-100">Zeitpläne</h1>
        <p className="text-dark-400 mt-1">Automatische Backup-Zeitpläne</p>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        <div className="lg:col-span-2">
          {isLoading ? (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              {[...Array(4)].map((_, i) => (
                <div
                  key={i}
                  className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse"
                >
                  <div className="h-6 bg-dark-700 rounded w-1/2 mb-2" />
                  <div className="h-4 bg-dark-700 rounded w-3/4" />
                </div>
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              {schedules?.map((schedule) => (
                <ScheduleCard key={schedule.id} schedule={schedule} />
              ))}
            </div>
          )}

          {!isLoading && schedules?.length === 0 && (
            <div className="bg-dark-800 rounded-xl border border-dark-700 p-12 text-center">
              <Clock className="w-12 h-12 text-dark-500 mx-auto mb-4" />
              <p className="text-dark-400">Keine Zeitpläne konfiguriert</p>
              <p className="text-sm text-dark-500 mt-2">
                Füge bei Backup Targets einen Cron-Schedule hinzu
              </p>
            </div>
          )}
        </div>

        <div>
          <CronHelp />
        </div>
      </div>
    </div>
  )
}
