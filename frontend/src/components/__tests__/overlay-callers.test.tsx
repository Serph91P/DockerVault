import { StrictMode, useState, type ReactNode } from 'react'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it } from 'vitest'
import { http, HttpResponse } from 'msw'
import { server } from '../../test/mocks/server'
import type { Backup, BackupTarget } from '../../api'
import BackupBrowser from '../BackupBrowser'
import BackupLogViewer from '../BackupLogViewer'
import BackupEditDialog from '../BackupEditDialog'
import BackupWizard from '../BackupWizard'
import RestoreWizard from '../RestoreWizard'
import StorageBrowser from '../StorageBrowser'
import { NotificationCenter } from '../NotificationCenter'
import { useKeyboardShortcuts } from '../KeyboardShortcuts'
import { Modal } from '../ui/dialog'
import ConfirmDialog from '../ConfirmDialog'

const target: BackupTarget = { id:1,name:'Synthetic plan',target_type:'path',host_path:'/synthetic',enabled:true,dependencies:[],stop_container:false,compression_enabled:true,created_at:'2026-01-01',updated_at:'2026-01-01' }
const backup: Backup = { id:1,target_id:1,status:'completed',backup_type:'full',created_at:'2026-01-01',file_path:'/synthetic/archive.tar' }
const cases: [string,(close:()=>void)=>ReactNode][] = [
 ['Browse Backup',close=><BackupBrowser backup={backup} onClose={close}/>],
 ['Backup 1 — Job Log',close=><BackupLogViewer backupId={1} onClose={close}/>],
 ['Edit Backup',close=><BackupEditDialog target={target} isOpen onClose={close}/>],
 ['Create Backup Target',close=><BackupWizard isOpen onClose={close}/>],
 ['Restore Backup',close=><RestoreWizard backupId={1} onClose={close}/>],
 ['Browse Local',close=><StorageBrowser storage={{id:'local',name:'Local',storage_type:'local',base_path:'/synthetic'}} onClose={close}/>],
 ['Notifications',close=><NotificationCenter isOpen onClose={close}/>],
]
function Harness({ content }: { content: (close:()=>void)=>ReactNode }) {
 const [open,setOpen]=useState(false)
 const { ShortcutHelp, showHelp, setShowHelp }=useKeyboardShortcuts()
 const location=useLocation()
 return <><button onClick={()=>setOpen(true)}>Open fixture</button><a href="#behind">Background action</a><output data-testid="path">{location.pathname}</output>{open&&content(()=>setOpen(false))}<ShortcutHelp isOpen={showHelp} onClose={()=>setShowHelp(false)}/></>
}
function renderCase(content:(close:()=>void)=>ReactNode) {
 const client=new QueryClient({defaultOptions:{queries:{retry:false}}})
 return render(<StrictMode><QueryClientProvider client={client}><MemoryRouter><Harness content={content}/></MemoryRouter></QueryClientProvider></StrictMode>)
}
beforeEach(()=> {
 server.use(http.get('/api/v1/backups/1/files',()=>HttpResponse.json({files:[]})),http.get('/api/v1/backups/1/logs',()=>HttpResponse.json([])),http.get('/api/v1/backups/1/restore-info',()=>new HttpResponse(null,{status:404})),http.get('/api/v1/storage/local/files',()=>HttpResponse.json({files:[]})))
})
describe('complete overlay caller contract',()=> {
 it.each(cases)('%s contains focus, suspends shortcuts, closes on Escape and restores focus',async(title,content)=> {
  const user=userEvent.setup();renderCase(content)
  const trigger=screen.getByRole('button',{name:'Open fixture'});await user.click(trigger)
  const dialog=await screen.findByRole('dialog',{name:title})
  await waitFor(()=>expect(dialog.contains(document.activeElement)).toBe(true))
  for(let index=0;index<6;index++){await user.tab({shift:index%2===0});await waitFor(()=>expect(dialog.contains(document.activeElement)).toBe(true))}
  await user.keyboard('gb?')
  expect(screen.getByTestId('path')).toHaveTextContent('/')
  expect(screen.queryByRole('dialog',{name:'Keyboard Shortcuts'})).not.toBeInTheDocument()
  await user.keyboard('{Escape}')
  await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  await waitFor(()=>expect(trigger).toHaveFocus())
 })
 it('escapes only the top nested confirmation, then returns to the parent trigger',async()=> {
  function Nested({close}:{close:()=>void}) {
   const [child,setChild]=useState(false)
   return <Modal title="Storage parent" onClose={close}><button onClick={()=>setChild(true)}>Delete item</button><ConfirmDialog isOpen={child} title="Delete item?" message="Synthetic only" onClose={()=>setChild(false)} onConfirm={()=>{}}/></Modal>
  }
  const user=userEvent.setup();renderCase(close=><Nested close={close}/>);await user.click(screen.getByRole('button',{name:'Open fixture'}));await user.click(screen.getByRole('button',{name:'Delete item'}))
  expect(screen.getByRole('dialog',{name:'Delete item?'})).toBeInTheDocument()
  await user.keyboard('{Escape}')
  await waitFor(()=>expect(screen.queryByRole('dialog',{name:'Delete item?'})).not.toBeInTheDocument())
  expect(within(screen.getByRole('dialog',{name:'Storage parent'})).getByRole('button',{name:'Delete item'})).toHaveFocus()
  await user.keyboard('{Escape}');await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
 })
})
