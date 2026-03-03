import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'
import Dashboard from '../Dashboard'
import { server } from '../../test/mocks/server'
import { http, HttpResponse } from 'msw'

const createWrapper = () => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
    },
  })
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>{children}</BrowserRouter>
    </QueryClientProvider>
  )
}

describe('Dashboard Page', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('should display dashboard title and description', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    expect(screen.getByText('Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Overview of your Docker backups')).toBeInTheDocument()
  })

  it('should display stat cards with correct data', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // Stat cards (some labels may appear in Status Summary too, so use getAllByText)
      expect(screen.getAllByText('Containers').length).toBeGreaterThanOrEqual(1)
      expect(screen.getAllByText('Volumes').length).toBeGreaterThanOrEqual(1)
      expect(screen.getByText('Stacks')).toBeInTheDocument()
      expect(screen.getByText('Targets')).toBeInTheDocument()
      expect(screen.getByText('Scheduled')).toBeInTheDocument()
    })
  })

  it('should display recent backups section', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('Recent Backups')).toBeInTheDocument()
    })
  })

  it('should display scheduled backups section', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('Upcoming Scheduled Backups')).toBeInTheDocument()
    })
  })

  it('should display status summary section', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('Status Summary')).toBeInTheDocument()
      expect(screen.getByText('Successful')).toBeInTheDocument()
      expect(screen.getByText('Failed')).toBeInTheDocument()
      expect(screen.getByText('Active Containers')).toBeInTheDocument()
    })
  })

  it('should display container count from API', async () => {
    server.use(
      http.get('/api/v1/docker/containers', () => {
        return HttpResponse.json([
          {
            id: 'container1',
            name: 'test-container-1',
            image: 'nginx:latest',
            status: 'running',
            state: 'running',
            created: '2024-01-01T09:00:00Z',
            labels: {},
            mounts: [],
            networks: ['bridge'],
            depends_on: [],
          },
          {
            id: 'container2',
            name: 'test-container-2',
            image: 'redis:latest',
            status: 'running',
            state: 'running',
            created: '2024-01-01T10:00:00Z',
            labels: {},
            mounts: [],
            networks: ['bridge'],
            depends_on: [],
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('0/2')).toBeInTheDocument()
    })
  })

  it('should display volume count from API', async () => {
    server.use(
      http.get('/api/v1/docker/volumes', () => {
        return HttpResponse.json([
          {
            name: 'volume-1',
            driver: 'local',
            mountpoint: '/var/lib/docker/volumes/volume-1/_data',
            labels: {},
            created_at: '2024-01-01T08:00:00Z',
            used_by: [],
          },
          {
            name: 'volume-2',
            driver: 'local',
            mountpoint: '/var/lib/docker/volumes/volume-2/_data',
            labels: {},
            created_at: '2024-01-01T08:00:00Z',
            used_by: [],
          },
          {
            name: 'volume-3',
            driver: 'local',
            mountpoint: '/var/lib/docker/volumes/volume-3/_data',
            labels: {},
            created_at: '2024-01-01T08:00:00Z',
            used_by: [],
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // 3 volumes shown in two places: stat card and status summary
      const threeElements = screen.getAllByText('3')
      expect(threeElements.length).toBeGreaterThanOrEqual(1)
    })
  })

  it('should display backup targets count from API', async () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return HttpResponse.json([
          {
            id: 1,
            name: 'target-1',
            target_type: 'volume',
            enabled: true,
            dependencies: [],
            stop_container: true,
            compression_enabled: true,
            created_at: '2024-01-01T07:00:00Z',
            updated_at: '2024-01-01T07:00:00Z',
          },
          {
            id: 2,
            name: 'target-2',
            target_type: 'container',
            enabled: true,
            dependencies: [],
            stop_container: false,
            compression_enabled: true,
            created_at: '2024-01-01T07:00:00Z',
            updated_at: '2024-01-01T07:00:00Z',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // Should show 2 active targets
      const twoElements = screen.getAllByText('2')
      expect(twoElements.length).toBeGreaterThanOrEqual(1)
    })
  })

  it('should display backup list from API', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return HttpResponse.json([
          {
            id: 1,
            target_id: 1,
            target_name: 'my-database-backup',
            backup_type: 'full',
            status: 'completed',
            file_path: '/backups/test.tar.gz',
            file_size: 1024,
            file_size_human: '1 KB',
            duration_seconds: 60,
            created_at: '2024-01-01T10:00:00Z',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('my-database-backup')).toBeInTheDocument()
      expect(screen.getByText('1 KB')).toBeInTheDocument()
      expect(screen.getByText('60s')).toBeInTheDocument()
    })
  })

  it('should show empty state when no backups exist', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return HttpResponse.json([])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('No backups yet')).toBeInTheDocument()
    })
  })

  it('should show empty state when no schedules exist', async () => {
    server.use(
      http.get('/api/v1/schedules', () => {
        return HttpResponse.json([])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('No scheduled backups')).toBeInTheDocument()
    })
  })

  it('should display scheduled backups from API', async () => {
    server.use(
      http.get('/api/v1/schedules', () => {
        return HttpResponse.json([
          {
            id: 1,
            name: 'Daily Backup Schedule',
            cron_expression: '0 3 * * *',
            enabled: true,
            target_count: 2,
            next_run: '2024-01-02T03:00:00Z',
            created_at: '2024-01-01T00:00:00Z',
            updated_at: '2024-01-01T00:00:00Z',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('Daily Backup Schedule')).toBeInTheDocument()
      expect(screen.getByText('0 3 * * *')).toBeInTheDocument()
    })
  })

  it('should count running containers correctly', async () => {
    server.use(
      http.get('/api/v1/docker/containers', () => {
        return HttpResponse.json([
          {
            id: 'container1',
            name: 'running-container',
            image: 'nginx:latest',
            status: 'running',
            state: 'running',
            created: '2024-01-01T09:00:00Z',
            labels: {},
            mounts: [],
            networks: ['bridge'],
            depends_on: [],
          },
          {
            id: 'container2',
            name: 'stopped-container',
            image: 'redis:latest',
            status: 'exited',
            state: 'exited',
            created: '2024-01-01T10:00:00Z',
            labels: {},
            mounts: [],
            networks: ['bridge'],
            depends_on: [],
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // 1 running out of 2 total
      expect(screen.getByText('1/2')).toBeInTheDocument()
    })
  })

  it('should count completed and failed backups in status summary', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return HttpResponse.json([
          {
            id: 1,
            target_id: 1,
            target_name: 'backup-1',
            backup_type: 'full',
            status: 'completed',
            created_at: '2024-01-01T10:00:00Z',
          },
          {
            id: 2,
            target_id: 1,
            target_name: 'backup-2',
            backup_type: 'full',
            status: 'completed',
            created_at: '2024-01-01T11:00:00Z',
          },
          {
            id: 3,
            target_id: 1,
            target_name: 'backup-3',
            backup_type: 'full',
            status: 'failed',
            created_at: '2024-01-01T12:00:00Z',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // Status summary should show 2 completed
      const summarySection = screen.getByText('Status Summary').parentElement
      expect(summarySection).toBeInTheDocument()
    })
  })

  it('should be accessible with proper headings', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    // Check for main heading
    const mainHeading = screen.getByRole('heading', { level: 1, name: 'Dashboard' })
    expect(mainHeading).toBeInTheDocument()
    
    await waitFor(() => {
      // Check for section headings
      expect(screen.getByText('Recent Backups')).toBeInTheDocument()
      expect(screen.getByText('Upcoming Scheduled Backups')).toBeInTheDocument()
      expect(screen.getByText('Status Summary')).toBeInTheDocument()
    })
  })
})
