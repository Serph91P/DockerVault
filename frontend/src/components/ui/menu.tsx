import { Menu as MenuPrimitive } from '@base-ui/react/menu'
import type { ReactNode } from 'react'

export function Menu(props: MenuPrimitive.Root.Props) { return <MenuPrimitive.Root {...props} /> }
export function MenuTrigger(props: MenuPrimitive.Trigger.Props) { return <MenuPrimitive.Trigger {...props} /> }
export function MenuItem(props: MenuPrimitive.Item.Props) { return <MenuPrimitive.Item {...props} /> }
export function MenuContent({ children }: { children: ReactNode }) {
  return <MenuPrimitive.Portal><MenuPrimitive.Positioner sideOffset={4} align="end" className="z-[110]">
    <MenuPrimitive.Popup data-overlay="menu" className="max-h-[min(24rem,80dvh)] min-w-44 max-w-[calc(100vw-2rem)] break-words overflow-y-auto rounded-lg border border-dark-500 bg-dark-800 py-1 text-sm shadow-xl [&_[role=menuitem]]:flex [&_[role=menuitem]]:min-h-11 [&_[role=menuitem]]:w-full [&_[role=menuitem]]:items-center [&_[role=menuitem]]:gap-2 [&_[role=menuitem]]:px-3 [&_[data-highlighted]]:bg-dark-600">{children}</MenuPrimitive.Popup>
  </MenuPrimitive.Positioner></MenuPrimitive.Portal>
}
