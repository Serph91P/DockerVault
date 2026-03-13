import type { LucideIcon } from 'lucide-react'

interface EmptyStateProps {
  icon: LucideIcon
  title: string
  description?: string
  action?: {
    label: string
    onClick: () => void
    icon?: LucideIcon
  }
  compact?: boolean
}

export default function EmptyState({ icon: Icon, title, description, action, compact = false }: EmptyStateProps) {
  const ActionIcon = action?.icon

  return (
    <div
      className={`bg-dark-800 rounded-xl border border-dark-700 text-center ${compact ? 'p-6' : 'p-12'}`}
    >
      <Icon className={`${compact ? 'w-8 h-8' : 'w-12 h-12'} text-dark-500 mx-auto ${compact ? 'mb-2' : 'mb-4'}`} />
      <p className="text-dark-400">{title}</p>
      {description && (
        <p className={`text-sm text-dark-500 ${compact ? 'mt-1' : 'mt-2'}`}>{description}</p>
      )}
      {action && (
        <button
          onClick={action.onClick}
          className={`inline-flex items-center gap-2 px-4 py-2 bg-primary-500 hover:bg-primary-600 text-white rounded-lg transition-colors ${compact ? 'mt-3 text-sm' : 'mt-4'}`}
        >
          {ActionIcon && <ActionIcon className="w-4 h-4" />}
          {action.label}
        </button>
      )}
    </div>
  )
}
