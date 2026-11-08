import { Link, Outlet, useLocation } from 'react-router-dom'
import { LayoutDashboard, Archive, History, Cloud, Settings, Wifi, WifiOff, LogOut, Menu, Keyboard } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useWebSocketStore } from '../store/websocket'
import { useAuthStore } from '../store/auth'
import { useKeyboardShortcuts } from './KeyboardShortcuts'
import { NotificationBadge, NotificationCenter } from './NotificationCenter'
import { Dialog, DialogContent, DialogTitle, DialogTrigger, DialogClose } from './ui/dialog'
import { Button, IconButton } from './ui/button'
import { startQueryFreshness } from '../store/freshness'
import ConnectionSignals from './ConnectionSignals'
import { useNotificationStore } from '../store/notifications'

const navigation = [
  { name: 'Overview', href: '/', icon: LayoutDashboard },
  { name: 'Protection', href: '/protection', icon: Archive },
  { name: 'History & Restore', href: '/history', icon: History },
  { name: 'Storage', href: '/storage', icon: Cloud },
  { name: 'Settings', href: '/settings', icon: Settings },
]
const sections: Record<string, string> = { schedules: 'Schedules', retention: 'Retention' }

export default function Layout() {
  const location = useLocation()
  const connected = useWebSocketStore((state) => state.connected)
  const latest = useNotificationStore(state => state.notifications[0])
  const { user, logout } = useAuthStore()
  const { showHelp, setShowHelp, ShortcutHelp } = useKeyboardShortcuts()
  const [mobileOpen, setMobileOpen] = useState(false)
  const [showNotifications, setShowNotifications] = useState(false)
  const main = useRef<HTMLElement>(null)
  const current = navigation.find((item) => item.href === '/' ? location.pathname === '/' : location.pathname === item.href || location.pathname.startsWith(`${item.href}/`))
  const section = sections[location.pathname.split('/')[2]] || (location.pathname === '/protection/new' ? 'New plan' : /^\/protection\/[1-9]\d*\/edit$/.test(location.pathname) ? 'Edit plan' : /^\/protection\/[1-9]\d*$/.test(location.pathname) ? 'Plan detail' : undefined)
  const policySegments = location.pathname.split('/')
  const policyKind = policySegments[2]
  const policyNoun = policyKind === 'schedules' ? 'schedule' : 'retention policy'
  const policyTitle = sections[policyKind] && policySegments[3] ? policySegments[3] === 'new' ? `New ${policyNoun}` : policySegments[4] === 'edit' ? `Edit ${policyNoun}` : policyKind === 'schedules' ? 'Schedule detail' : 'Retention policy detail' : undefined
  const title = current ? policyTitle || section || current.name : 'Page not found'

  useEffect(startQueryFreshness, [])


  useEffect(() => {
    document.title = `${title} · DockerVault`
    const section = location.pathname === '/settings' && ['#connection', '#encryption', '#ssh-keys'].includes(location.hash) ? document.getElementById(location.hash.slice(1)) : location.pathname === '/' && location.hash === '#overview-capacity' ? document.getElementById('overview-capacity') : null
    if (section) section.focus()
    else {
      main.current?.focus({ preventScroll: true })
      main.current?.scrollTo?.(0, 0)
    }
  }, [location.pathname, location.hash, title])

  const links = (mobile = false) => <nav aria-label={mobile ? 'Mobile navigation' : 'Primary navigation'} className="shell-links">
    {navigation.map((item) => <Link key={item.href} to={item.href} aria-current={current?.href === item.href ? 'page' : undefined} title={item.name} onClick={() => setMobileOpen(false)}>
      <item.icon aria-hidden="true" className="size-5 shrink-0" /><span>{item.name}</span>
    </Link>)}
    {current?.href === '/protection' && <div className="shell-subnav">
      <Link to="/protection/schedules" aria-current={section === 'Schedules' ? 'page' : undefined}>Schedules</Link>
      <Link to="/protection/retention" aria-current={section === 'Retention' ? 'page' : undefined}>Retention</Link>
    </div>}
  </nav>

  const account = <div className="shell-account">
    <p className="break-all text-sm text-dark-200">{user?.username || 'Account'}</p>
    <Button variant="ghost" onClick={() => void logout()}><LogOut className="size-4" />Log out</Button>
  </div>

  return <div className="app-shell">
    <a href="#main-content" className="skip-link">Skip to main content</a>
    <p role="status" className="sr-only">{latest ? `${latest.title}${latest.target ? `, ${latest.target.kind} ${latest.target.id}` : ''}` : ''}</p>
    <aside className="shell-sidebar" aria-label="Application sidebar">
      <Link to="/" className="shell-brand" aria-label="DockerVault overview"><Archive className="size-6 text-primary-300" /><span>DockerVault</span></Link>
      {links()}{account}
    </aside>
    <div className="shell-workspace">
      <header className="shell-header">
        <Dialog open={mobileOpen} onOpenChange={setMobileOpen}>
          <DialogTrigger render={<Button variant="outline" className="mobile-nav-trigger" aria-label="Open navigation" />}><Menu className="size-5" /></DialogTrigger>
          <DialogContent className="overlay-sheet shell-mobile">
            <div className="flex items-center justify-between gap-2 p-4"><DialogTitle className="font-semibold">Navigation</DialogTitle><DialogClose render={<Button variant="outline" />}>Close</DialogClose></div>
            <p className="px-4 text-sm text-dark-300">Current location: {title}</p>
            {links(true)}{account}
            <Button variant="ghost" onClick={() => setShowNotifications(true)}>Notifications & operations</Button>
            <Button variant="ghost" onClick={() => setShowHelp(true)}>Keyboard shortcuts</Button>
            <NotificationCenter isOpen={showNotifications} onClose={() => setShowNotifications(false)} />
            <ShortcutHelp isOpen={showHelp} onClose={() => setShowHelp(false)} />
          </DialogContent>
        </Dialog>
        <nav aria-label="Breadcrumb" className="min-w-0 flex-1 text-sm text-dark-300"><Link to="/">DockerVault</Link><span aria-hidden="true"> / </span>{section ? <><Link to="/protection">Protection</Link><span aria-hidden="true"> / </span></> : null}{policyTitle && <><Link to={`/protection/${policyKind}`}>{section}</Link><span aria-hidden="true"> / </span></>}<span aria-current="page">{title}</span></nav>
        <span role="status" className="connection-status" title="Live updates connection">{connected ? <Wifi className="size-4" /> : <WifiOff className="size-4" />}<span>{connected ? 'Live updates connected' : 'Live updates disconnected'}</span></span>
        <NotificationBadge onClick={() => setShowNotifications(true)} />
        <ConnectionSignals />
        <IconButton label="Keyboard shortcuts" onClick={() => setShowHelp(true)}><Keyboard className="size-5" /></IconButton>
      </header>
      <main id="main-content" ref={main} tabIndex={-1} className="shell-main">{!connected && <p role="status" className="mb-4 rounded-lg border border-amber-500/40 p-3 text-amber-200">Live updates interrupted. HTTP data is refreshed while this tab is visible (10-30 seconds). Other service availability is measured separately.</p>}<Outlet /></main>
    </div>
    {!mobileOpen && <NotificationCenter isOpen={showNotifications} onClose={() => setShowNotifications(false)} />}
    {!mobileOpen && <ShortcutHelp isOpen={showHelp} onClose={() => setShowHelp(false)} />}
  </div>
}
