import { Button as ButtonPrimitive } from '@base-ui/react/button'
import { cva, type VariantProps } from 'class-variance-authority'
import { twMerge } from 'tailwind-merge'

// shadcn/ui base-nova Button, adapted to DockerVault tokens and touch targets (MIT).
const variants = cva('inline-flex min-h-11 items-center justify-center gap-2 rounded-lg border px-4 py-2 text-sm font-medium disabled:opacity-50', {
  variants: {
    variant: {
      default: 'border-transparent bg-primary-700 text-white hover:bg-primary-800',
      outline: 'border-dark-500 bg-dark-800 text-dark-100 hover:bg-dark-700',
      ghost: 'border-transparent text-dark-200 hover:bg-dark-700',
      destructive: 'border-transparent bg-red-700 text-white hover:bg-red-800',
    },
  }, defaultVariants: { variant: 'default' },
})
export function Button({ className, variant, ...props }: ButtonPrimitive.Props & VariantProps<typeof variants>) {
  return <ButtonPrimitive data-slot="button" className={twMerge(variants({ variant }), typeof className === 'string' ? className : '')} {...props} />
}
export function IconButton({ label, ...props }: Omit<ButtonPrimitive.Props, 'aria-label'> & { label: string }) {
  return <Button variant="ghost" aria-label={label} title={label} {...props} />
}
