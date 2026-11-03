import { afterEach, it, expect, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import BackupEditDialog from '../BackupEditDialog'
import BackupWizard from '../BackupWizard'
import { server } from '../../test/mocks/server'
import type { BackupTarget } from '../../api'

const target: BackupTarget = {
  id: 917, name: 'owned-clear-contract', target_type: 'path', host_path: '/owned/source',
  schedule_id: 41, schedule_cron: '0 3 * * *', retention_policy_id: 42, enabled: true,
  dependencies: [], selected_volumes: [], include_paths: [], exclude_paths: [], per_volume_rules: {},
  pre_backup_command: 'pg_dump before', post_backup_command: 'redis-cli after',
  stop_container: false, compression_enabled: true, remote_storage_ids: [], delete_local_after_sync: false,
  created_at: '2026-11-01T00:00:00Z', updated_at: '2026-11-01T00:00:00Z',
}

afterEach(cleanup)

function prepare(wizard: boolean) {
  let sent: Record<string, unknown> | undefined
  server.use(
    http.get('/api/v1/targets/917', () => HttpResponse.json(target)),
    http.get('/api/v1/schedules', () => HttpResponse.json([{ id: 41, name: 'Owned schedule', cron_expression: '0 2 * * *', enabled: true, target_count: 1 }])),
    http.get('/api/v1/retention', () => HttpResponse.json([{ id: 42, name: 'Owned retention', keep_last: 3, keep_daily: 0, keep_weekly: 0, keep_monthly: 0 }])),
    http.get('/api/v1/storage', () => HttpResponse.json([])),
    http.put('/api/v1/targets/917', async ({ request }) => {
      sent = await request.json() as Record<string, unknown>
      return HttpResponse.json({ ...target, ...sent })
    }),
  )
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const close = vi.fn()
  render(<QueryClientProvider client={client}>{wizard
    ? <BackupWizard editTarget={target} isOpen onClose={close} />
    : <BackupEditDialog target={target} isOpen onClose={close} />}</QueryClientProvider>)
  return { getSent: () => sent, close }
}

it('serializes explicit removals from the real edit dialog', async () => {
  const { getSent, close } = prepare(false)
  const user = userEvent.setup()
  await screen.findByRole('option', { name: /Owned schedule/ })
  await screen.findByRole('option', { name: 'Owned retention' })
  await user.selectOptions(screen.getByRole('option', { name: 'No schedule (manual only)' }).closest('select')!, '')
  await user.selectOptions(screen.getByRole('option', { name: 'Keep all backups' }).closest('select')!, '')
  await user.click(screen.getByRole('button', { name: /^None/ }))
  await user.click(screen.getByRole('button', { name: 'Advanced: Pre/Post Commands' }))
  await user.clear(screen.getByDisplayValue('pg_dump before'))
  await user.clear(screen.getByDisplayValue('redis-cli after'))
  await user.click(screen.getByRole('button', { name: 'Save Changes' }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(getSent()).toMatchObject({ schedule_id: null, schedule_cron: null, retention_policy_id: null, pre_backup_command: null, post_backup_command: null, enabled: true, compression_enabled: false, stop_container: false, remote_storage_ids: [], include_paths: [], exclude_paths: [] })
})

it('serializes explicit removals from the existing wizard edit caller', async () => {
  const { getSent, close } = prepare(true)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await screen.findByText('Owned schedule')
  await user.click(screen.getByRole('button', { name: /No schedule \(manual only\)/ }))
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await user.click(screen.getByRole('button', { name: /No retention policy/ }))
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await user.click(screen.getByRole('button', { name: 'Advanced Options' }))
  await user.clear(screen.getByDisplayValue('pg_dump before'))
  await user.clear(screen.getByDisplayValue('redis-cli after'))
  await user.click(screen.getByRole('button', { name: /^Next/ }))
  await user.click(screen.getByRole('button', { name: /Save Changes/ }))
  await waitFor(() => expect(close).toHaveBeenCalledOnce())
  expect(getSent()).toMatchObject({ schedule_id: null, schedule_cron: null, retention_policy_id: null, pre_backup_command: null, post_backup_command: null })
})
