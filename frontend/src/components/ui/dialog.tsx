import { Dialog as DialogPrimitive } from '@base-ui/react/dialog'
import { createContext, useContext, useLayoutEffect, useMemo, useRef, type ReactNode } from 'react'

const ReturnFocusContext = createContext<HTMLElement | null>(null)
function interactionOrigin() {
  const active = document.activeElement instanceof HTMLElement ? document.activeElement : null
  const menuTrigger = active?.closest('[role="menu"]')?.getAttribute('aria-labelledby')
  return menuTrigger ? document.getElementById(menuTrigger) : active
}

// Adapted from shadcn/ui base-nova Dialog (MIT); see README.md in this directory.
export function Dialog(props: DialogPrimitive.Root.Props) {
  // Capture on the opening render, before commit removes the launching menu.
  const origin = useMemo(() => props.open ? interactionOrigin() : null, [props.open])
  return <ReturnFocusContext.Provider value={origin}><DialogPrimitive.Root {...props} /></ReturnFocusContext.Provider>
}
export function DialogTrigger(props: DialogPrimitive.Trigger.Props) { return <DialogPrimitive.Trigger {...props} /> }
export function DialogTitle(props: DialogPrimitive.Title.Props) { return <DialogPrimitive.Title {...props} /> }
export function DialogDescription(props: DialogPrimitive.Description.Props) { return <DialogPrimitive.Description {...props} /> }
export function DialogClose(props: DialogPrimitive.Close.Props) { return <DialogPrimitive.Close {...props} /> }

function FocusManagedPopup(props: DialogPrimitive.Popup.Props) {
  const origin = useContext(ReturnFocusContext)
  const returnTarget = useRef<HTMLElement | null>(null)
  const pendingStatus = useRef<HTMLParagraphElement | null>(null)
  const busy = props['aria-busy'] === true || props['aria-busy'] === 'true'
  useLayoutEffect(() => { if (busy) pendingStatus.current?.focus({ preventScroll: true }) }, [busy])
  return <DialogPrimitive.Popup
    initialFocus={() => { returnTarget.current = origin ?? interactionOrigin(); return true }}
    finalFocus={() => returnTarget.current?.isConnected ? returnTarget.current : document.querySelector<HTMLElement>('#main-content')}
    {...props}
  >
    {busy && <p ref={pendingStatus} role="status" tabIndex={0} className="px-4 py-2 text-sm text-dark-200">Processing. Please wait.</p>}
    {props.children}
  </DialogPrimitive.Popup>
}

export function DialogContent({ children, className = '', ...props }: DialogPrimitive.Popup.Props) {
  return <DialogPrimitive.Portal>
    <DialogPrimitive.Backdrop className="overlay-backdrop" />
    <DialogPrimitive.Viewport className="overlay-viewport">
      <FocusManagedPopup data-overlay="dialog" className={`overlay-content ${className}`} {...props}>{children}</FocusManagedPopup>
    </DialogPrimitive.Viewport>
  </DialogPrimitive.Portal>
}

/** Compatibility boundary for existing imperative callers; domain content stays local. */
export function Modal({ open = true, onClose, title, pending = false, sheet = false, children }: {
  open?: boolean; onClose: () => void; title: string; pending?: boolean; sheet?: boolean; children: ReactNode
}) {
  return <Dialog open={open} onOpenChange={(next, details) => { if (!next) { if (pending) details.cancel(); else onClose() } }}>
    <DialogContent
      className={sheet ? 'overlay-sheet legacy-modal' : 'legacy-modal'}
      aria-busy={pending || undefined}
    >
      <DialogTitle className="sr-only">{title}</DialogTitle>
      {children}
    </DialogContent>
  </Dialog>
}
