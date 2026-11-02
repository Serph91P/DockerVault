import { AlertTriangle, Loader2 } from 'lucide-react'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from './ui/dialog'
import { Button } from './ui/button'

interface ConfirmDialogProps {
  isOpen: boolean
  onClose: () => void
  onConfirm: () => void
  title: string
  message: string
  confirmLabel?: string
  confirmVariant?: 'danger' | 'primary'
  isLoading?: boolean
  confirmDisabled?: boolean
  children?: React.ReactNode
}

export default function ConfirmDialog({ isOpen, onClose, onConfirm, title, message, confirmLabel = 'Confirm', confirmVariant = 'danger', isLoading = false, confirmDisabled = false, children }: ConfirmDialogProps) {
  return <Dialog open={isOpen} onOpenChange={(open) => { if (!open && !isLoading) onClose() }}>
    <DialogContent className="w-full max-w-md rounded-xl border border-dark-600 bg-dark-800 p-6" aria-busy={isLoading || undefined}>
      <div className="flex items-start gap-3">
        <AlertTriangle className="size-6 shrink-0 text-amber-300" />
        <div className="min-w-0">
          <DialogTitle className="text-lg font-semibold text-dark-100">{title}</DialogTitle>
          <DialogDescription className="mt-2 text-sm text-dark-300">{message}</DialogDescription>
          {children && <div className="mt-3">{children}</div>}
        </div>
      </div>
      <div className="mt-6 flex flex-wrap justify-end gap-3">
        <Button variant="outline" onClick={onClose} disabled={isLoading}>Cancel</Button>
        <Button variant={confirmVariant === 'danger' ? 'destructive' : 'default'} onClick={onConfirm} disabled={isLoading || confirmDisabled}>
          {isLoading ? <><Loader2 className="size-4 animate-spin" />Processing...</> : confirmLabel}
        </Button>
      </div>
    </DialogContent>
  </Dialog>
}
