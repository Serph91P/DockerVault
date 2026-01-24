import { Routes, Route } from 'react-router-dom'
import Layout from './components/Layout'
import Dashboard from './pages/Dashboard'
import Containers from './pages/Containers'
import Volumes from './pages/Volumes'
import Stacks from './pages/Stacks'
import Targets from './pages/Targets'
import Backups from './pages/Backups'
import Schedules from './pages/Schedules'
import Retention from './pages/Retention'
import Storage from './pages/Storage'
import Settings from './pages/Settings'

function App() {
  return (
    <Routes>
      <Route path="/" element={<Layout />}>
        <Route index element={<Dashboard />} />
        <Route path="containers" element={<Containers />} />
        <Route path="volumes" element={<Volumes />} />
        <Route path="stacks" element={<Stacks />} />
        <Route path="targets" element={<Targets />} />
        <Route path="backups" element={<Backups />} />
        <Route path="schedules" element={<Schedules />} />
        <Route path="retention" element={<Retention />} />
        <Route path="storage" element={<Storage />} />
        <Route path="settings" element={<Settings />} />
      </Route>
    </Routes>
  )
}

export default App
