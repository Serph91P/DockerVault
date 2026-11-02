import { Dialog as DialogPrimitive } from '@base-ui/react/dialog'
import { useRef, type ReactNode } from 'react'

// Adapted from shadcn/ui base-nova Dialog (MIT); see README.md in this directory.
export function Dialog(props: DialogPrimitive.Root.Props) { return <DialogPrimitive.Root {...props} /> }
export function DialogTrigger(props: DialogPrimitive.Trigger.Props) { return <DialogPrimitive.Trigger {...props} /> }
export function DialogTitle(props: DialogPrimitive.Title.Props) { return <DialogPrimitive.Title {...props} /> }
export function DialogDescription(props: DialogPrimitive.Description.Props) { return <DialogPrimitive.Description {...props} /> }
export function DialogClose(props: DialogPrimitive.Close.Props) { return <DialogPrimitive.Close {...props} /> }

export function DialogContent({ children, className = '', ...props }: DialogPrimitive.Popup.Props) {
  const returnTarget = useRef<HTMLElement | null>(null)
  return <DialogPrimitive.Portal>
    <DialogPrimitive.Backdrop className="overlay-backdrop" />
    <DialogPrimitive.Viewport className="overlay-viewport">
      <DialogPrimitive.Popup data-overlay="dialog" className={`overlay-content ${className}`}
        initialFocus={() => { returnTarget.current = document.activeElement instanceof HTMLElement ? document.activeElement : null; return true }}
        finalFocus={() => returnTarget.current?.isConnected ? returnTarget.current : document.querySelector<HTMLElement>('#main-content')}
        {...props}>{children}</DialogPrimitive.Popup>
    </DialogPrimitive.Viewport>
  </DialogPrimitive.Portal>
}

/** Compatibility boundary for existing imperative callers; domain content stays local. */
export function Modal({ open = true, onClose, title, pending = false, sheet = false, children }: {
  open?: boolean; onClose: () => void; title: string; pending?: boolean; sheet?: boolean; children: ReactNode
}) {
  const returnTarget = useRef<HTMLElement | null>(null)
  return <Dialog open={open} onOpenChange={(next) => { if (!next && !pending) onClose() }}>
    <DialogContent
      className={sheet ? 'overlay-sheet legacy-modal' : 'legacy-modal'}
      aria-busy={pending || undefined}
      initialFocus={() => {
        returnTarget.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
        return true
      }}
      finalFocus={() => returnTarget.current?.isConnected ? returnTarget.current : document.querySelector<HTMLElement>('#main-content')}
    >
      <DialogTitle className="sr-only">{title}</DialogTitle>
      {children}
    </DialogContent>
  </Dialog>
}
