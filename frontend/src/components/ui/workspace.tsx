import type { ReactNode } from 'react'
import { AlertTriangle, CheckCircle, Info, Loader2 } from 'lucide-react'
import { Button } from './button'

export function PageHeader({ title, description, actions }: { title: string; description?: string; actions?: ReactNode }) {
  return <header className="flex flex-wrap items-start justify-between gap-4"><div className="min-w-0 flex-1"><h1 className="text-2xl font-semibold text-dark-100 break-words">{title}</h1>{description && <p className="mt-1 text-dark-300">{description}</p>}</div>{actions && <div className="flex flex-wrap gap-2">{actions}</div>}</header>
}
export function FilterBar({ children }: { children: ReactNode }) {
  return <div role="group" aria-label="Filters" className="flex flex-wrap items-end gap-3">{children}</div>
}
export function EntityTable({ label, children }: { label: string; children: ReactNode }) {
  return <div role="region" aria-label={`${label} — scroll horizontally for more columns`} tabIndex={0} className="max-w-full overflow-x-auto"><table className="w-full min-w-[36rem] text-sm"><caption className="sr-only">{label}</caption>{children}</table></div>
}
export function Pagination({ page, pages, onPageChange }: { page: number; pages: number; onPageChange: (page: number) => void }) {
  return <nav aria-label="Pagination" className="flex flex-wrap items-center gap-3"><Button variant="outline" disabled={page <= 1} onClick={() => onPageChange(page - 1)} aria-label="Previous page">Previous</Button><span aria-live="polite">Page {page} of {Math.max(1, pages)}</span><Button variant="outline" disabled={page >= pages} onClick={() => onPageChange(page + 1)} aria-label="Next page">Next</Button></nav>
}
export function SelectionToolbar({ count, onClear, children }: { count: number; onClear: () => void; children?: ReactNode }) {
  return <div className="flex flex-wrap items-center gap-3 border-t border-dark-500 p-3"><p role="status">{count} selected</p>{children}<Button variant="ghost" onClick={onClear}>Clear selection</Button></div>
}
export function StatusLabel({ tone = 'neutral', children }: { tone?: 'neutral' | 'success' | 'warning' | 'danger'; children: ReactNode }) {
  const Icon = tone === 'success' ? CheckCircle : tone === 'warning' || tone === 'danger' ? AlertTriangle : Info
  const colors = { neutral: 'text-dark-200', success: 'text-green-300', warning: 'text-amber-200', danger: 'text-red-300' }
  return <span className={`inline-flex items-center gap-1.5 text-sm ${colors[tone]}`}><Icon className="size-4" aria-hidden="true" />{children}</span>
}
export function QueryState({ state, title, description, action, announce = true }: { state: 'loading' | 'empty' | 'filtered-empty' | 'error'; title: string; description?: string; action?: ReactNode; announce?: boolean }) {
  return <section role={!announce ? undefined : state === 'error' ? 'alert' : state === 'loading' ? 'status' : undefined} className="space-y-3 py-8 text-dark-200">{state === 'loading' && <Loader2 className="size-5 animate-spin" aria-hidden="true" />}<h2 className="text-lg font-semibold">{title}</h2>{description && <p className="text-sm text-dark-300">{description}</p>}{action}</section>
}
