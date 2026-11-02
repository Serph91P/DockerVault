import { Input as InputPrimitive } from '@base-ui/react/input'
import { useId, type ComponentProps } from 'react'
import { twMerge } from 'tailwind-merge'

// shadcn/ui base-nova Input, adapted to existing dark-first tokens (MIT).
export function Input({ className, ...props }: ComponentProps<'input'>) {
  return <InputPrimitive data-slot="input" className={twMerge('min-h-11 w-full min-w-0 rounded-lg border border-dark-500 bg-dark-900 px-3 py-2 text-base text-dark-100 placeholder:text-dark-400 aria-invalid:border-red-300 disabled:opacity-50', className)} {...props} />
}
export function TextField({ label, help, error, id: suppliedId, ...props }: ComponentProps<'input'> & { label: string; help?: string; error?: string }) {
  const generatedId = useId()
  const id = suppliedId || generatedId
  return <div className="space-y-1">
    <label htmlFor={id} className="block text-sm font-medium text-dark-200">{label}</label>
    <Input id={id} aria-invalid={error ? true : undefined} aria-describedby={error ? `${id}-error` : help ? `${id}-help` : undefined} {...props} />
    {help && <p id={`${id}-help`} className="text-sm text-dark-300">{help}</p>}
    {error && <p id={`${id}-error`} className="text-sm text-red-300" role="alert">{error}</p>}
  </div>
}
