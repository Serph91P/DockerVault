import { http, HttpResponse } from 'msw'
import type { Backup, Container, Volume, BackupTarget } from '../../api'

// Mock data
const mockBackups: Backup[] = [
  {
    id: 1,
    target_id: 1,
    target_name: 'test-volume',
    backup_type: 'full',
    status: 'completed',
    file_path: '/backups/test.tar.gz',
    file_size: 1024,
    file_size_human: '1 KB',
    checksum: 'abc123',
    started_at: '2024-01-01T10:00:00Z',
    completed_at: '2024-01-01T10:05:00Z',
    duration_seconds: 300,
    created_at: '2024-01-01T09:59:00Z',
  },
  {
    id: 2,
    target_id: 2,
    target_name: 'db-volume',
    backup_type: 'incremental',
    status: 'running',
    started_at: '2024-01-01T11:00:00Z',
    created_at: '2024-01-01T10:59:00Z',
  },
]

const mockContainers: Container[] = [
  {
    id: 'container123',
    name: 'test-container',
    image: 'nginx:latest',
    status: 'running',
    state: 'running',
    created: '2024-01-01T09:00:00Z',
    labels: {
      'com.docker.compose.project': 'myproject',
      'com.docker.compose.service': 'web',
    },
    mounts: [
      {
        type: 'volume',
        source: 'test-volume',
        destination: '/data',
        name: 'test-volume',
        mode: 'rw',
        rw: true,
      },
    ],
    networks: ['bridge'],
    compose_project: 'myproject',
    compose_service: 'web',
    depends_on: [],
  },
]

const mockVolumes: Volume[] = [
  {
    name: 'test-volume',
    driver: 'local',
    mountpoint: '/var/lib/docker/volumes/test-volume/_data',
    labels: {},
    created_at: '2024-01-01T08:00:00Z',
    used_by: ['test-container'],
  },
]

const mockTargets: BackupTarget[] = [
  {
    id: 1,
    name: 'test-volume',
    target_type: 'volume',
    volume_name: 'test-volume',
    enabled: true,
    dependencies: [],
    stop_container: true,
    compression_enabled: true,
    created_at: '2024-01-01T07:00:00Z',
    updated_at: '2024-01-01T07:00:00Z',
  },
]

export const handlers = [
  // Backups API
  http.get('/api/v1/backups', () => {
    return HttpResponse.json(mockBackups)
  }),

  http.get('/api/v1/backups/:id', ({ params }) => {
    const backup = mockBackups.find(b => b.id === Number(params.id))
    if (!backup) {
      return new HttpResponse(null, {
        status: 404,
        statusText: 'Backup not found',
      })
    }
    return HttpResponse.json(backup)
  }),

  http.post('/api/v1/backups', async ({ request }) => {
    const body = await request.json() as { target_id: number; backup_type: string }
    
    if (body.target_id === 99999) {
      return new HttpResponse(JSON.stringify({ detail: 'Target not found' }), {
        status: 404,
        headers: { 'Content-Type': 'application/json' },
      })
    }

    const newBackup: Backup = {
      id: Date.now(),
      target_id: body.target_id,
      target_name: 'test-target',
      backup_type: body.backup_type as 'full' | 'incremental',
      status: 'pending',
      created_at: new Date().toISOString(),
    }
    
    return HttpResponse.json(newBackup, { status: 201 })
  }),

  http.delete('/api/v1/backups/:id', () => {
    return new HttpResponse(null, { status: 204 })
  }),

  http.post('/api/v1/backups/:id/restore', async ({ request }) => {
    const body = await request.json() as { target_path?: string }
    
    // Check for path traversal attempts
    if (body.target_path && body.target_path.includes('..')) {
      return new HttpResponse(
        JSON.stringify({ detail: 'Invalid path: path traversal detected' }),
        {
          status: 400,
          headers: { 'Content-Type': 'application/json' },
        }
      )
    }
    
    return HttpResponse.json({ message: 'Restore initiated' })
  }),

  // Containers API
  http.get('/api/v1/docker/containers', () => {
    return HttpResponse.json(mockContainers)
  }),

  http.post('/api/v1/docker/containers/:id/stop', ({ params }) => {
    return HttpResponse.json({ message: `Container ${params.id} stopped` })
  }),

  http.post('/api/v1/docker/containers/:id/start', ({ params }) => {
    return HttpResponse.json({ message: `Container ${params.id} started` })
  }),

  // Volumes API
  http.get('/api/v1/docker/volumes', () => {
    return HttpResponse.json(mockVolumes)
  }),

  // Targets API
  http.get('/api/v1/targets', () => {
    return HttpResponse.json(mockTargets)
  }),

  http.get('/api/v1/targets/:id', ({ params }) => {
    const target = mockTargets.find(t => t.id === Number(params.id))
    if (!target) {
      return new HttpResponse(null, {
        status: 404,
        statusText: 'Target not found',
      })
    }
    return HttpResponse.json(target)
  }),

  http.post('/api/v1/targets', async ({ request }) => {
    const body = await request.json() as Partial<BackupTarget>
    const newTarget: BackupTarget = {
      id: Date.now(),
      name: body.name || 'new-target',
      target_type: body.target_type || 'volume',
      enabled: body.enabled ?? true,
      dependencies: body.dependencies || [],
      stop_container: body.stop_container ?? true,
      compression_enabled: body.compression_enabled ?? true,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      ...body,
    }
    return HttpResponse.json(newTarget, { status: 201 })
  }),

  http.put('/api/v1/targets/:id', async ({ params, request }) => {
    const body = await request.json() as Partial<BackupTarget>
    const existingTarget = mockTargets.find(t => t.id === Number(params.id))
    
    if (!existingTarget) {
      return new HttpResponse(null, { status: 404 })
    }
    
    const updatedTarget: BackupTarget = {
      ...existingTarget,
      ...body,
      updated_at: new Date().toISOString(),
    }
    
    return HttpResponse.json(updatedTarget)
  }),

  http.delete('/api/v1/targets/:id', () => {
    return new HttpResponse(null, { status: 204 })
  }),

  // Schedules API
  http.get('/api/v1/schedules', () => {
    return HttpResponse.json([
      {
        id: 1,
        target_id: 1,
        target_name: 'test-volume',
        cron_expression: '0 2 * * *',
        enabled: true,
        next_run: '2024-01-02T02:00:00Z',
      },
    ])
  }),

  http.post('/api/v1/schedules/:targetId/trigger', ({ params }) => {
    return HttpResponse.json({ message: `Backup triggered for target ${params.targetId}` })
  }),

  http.post('/api/v1/schedules/estimate', async ({ request }) => {
    const body = await request.json() as { target_id: number, cron_expression: string }
    
    if (body.cron_expression === 'invalid') {
      return new HttpResponse(JSON.stringify({ detail: 'Invalid cron expression' }), {
        status: 400,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    
    return HttpResponse.json({
      next_runs: ['2024-01-02T02:00:00Z', '2024-01-03T02:00:00Z'],
      description: 'Daily at 2:00 AM',
    })
  }),

  // System/Stats APIs
  http.get('/api/v1/stats/overview', () => {
    return HttpResponse.json({
      total_backups: 150,
      successful_backups: 142,
      failed_backups: 8,
      total_size: 52428800,
      total_size_human: '50 MB',
      active_targets: 12,
      scheduled_jobs: 5,
    })
  }),

  http.get('/api/v1/system/status', () => {
    return HttpResponse.json({
      docker_available: true,
      docker_version: '20.10.21',
      storage_usage: {
        used: 21474836480,
        total: 107374182400,
        used_human: '20 GB',
        total_human: '100 GB',
        percentage: 20,
      },
      backup_engine_status: 'running',
      scheduler_status: 'running',
      active_backups: 2,
    })
  }),

  // Health check
  http.get('/api/v1/health', () => {
    return HttpResponse.json({ status: 'healthy' })
  }),

  // Error simulation handlers
  http.get('/api/v1/backups/error', () => {
    return new HttpResponse(null, {
      status: 500,
      statusText: 'Internal Server Error',
    })
  }),
]
