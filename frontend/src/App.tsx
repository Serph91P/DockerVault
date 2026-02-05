import { useEffect } from 'react'
import { Routes, Route, Navigate } from 'react-router-dom'
import Layout from './components/Layout'
import Dashboard from './pages/Dashboard'
import Targets from './pages/Targets'
import Backups from './pages/Backups'
import Schedules from './pages/Schedules'
import Retention from './pages/Retention'
import Storage from './pages/Storage'
import Settings from './pages/Settings'
import Login from './pages/Login'
import SetupWizard from './pages/SetupWizard'
import { useAuthStore } from './store/auth'

function App() {
  const { isAuthenticated, isLoading, setupRequired, checkAuthStatus } = useAuthStore()
  
  useEffect(() => {
    checkAuthStatus()
  }, [checkAuthStatus])
  
  // Loading state
  if (isLoading) {
    return (
      <div className="min-h-screen bg-dark-900 flex items-center justify-center">
        <div className="flex flex-col items-center gap-4">
          <div className="w-12 h-12 border-4 border-indigo-500/30 border-t-indigo-500 rounded-full animate-spin" />
          <p className="text-dark-400">Loading...</p>
        </div>
      </div>
    )
  }
  
  // Setup required
  if (setupRequired) {
    return <SetupWizard />
  }
  
  // Not authenticated
  if (!isAuthenticated) {
    return <Login />
  }
  
  // Authenticated - show main app
  return (
    <Routes>
      <Route path="/" element={<Layout />}>
        <Route index element={<Dashboard />} />
        <Route path="backups" element={<Backups />} />
        <Route path="targets" element={<Targets />} />
        <Route path="schedules" element={<Schedules />} />
        <Route path="retention" element={<Retention />} />
        <Route path="storage" element={<Storage />} />
        <Route path="settings" element={<Settings />} />
      </Route>
      {/* Redirect any unknown routes to dashboard */}
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  )
}

export default App
