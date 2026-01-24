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

  it('should display dashboard overview', async () => {
    // Mock dashboard stats API
    server.use(
      http.get('/api/v1/stats/overview', () => {
        return HttpResponse.json({
          total_backups: 150,
          successful_backups: 142,
          failed_backups: 8,
          total_size: 52428800, // 50 MB
          total_size_human: '50 MB',
          active_targets: 12,
          scheduled_jobs: 5,
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('150')).toBeInTheDocument() // Total backups
    })
    
    expect(screen.getByText('142')).toBeInTheDocument() // Successful
    expect(screen.getByText('8')).toBeInTheDocument() // Failed
    expect(screen.getByText('50 MB')).toBeInTheDocument() // Total size
    expect(screen.getByText('12')).toBeInTheDocument() // Active targets
    expect(screen.getByText('5')).toBeInTheDocument() // Scheduled jobs
  })

  it('should display recent backups', async () => {
    // Mock recent backups API
    server.use(
      http.get('/api/v1/backups', ({ request }) => {
        const url = new URL(request.url)
        const limit = url.searchParams.get('limit')
        expect(limit).toBe('5') // Recent backups limit
        
        return HttpResponse.json([
          {
            id: 1,
            target_name: 'database-volume',
            backup_type: 'full',
            status: 'completed',
            created_at: '2024-01-01T10:00:00Z',
            file_size: 1048576,
            file_size_human: '1 MB',
          },
          {
            id: 2,
            target_name: 'app-volume',
            backup_type: 'incremental',
            status: 'running',
            created_at: '2024-01-01T09:30:00Z',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('database-volume')).toBeInTheDocument()
    })
    
    expect(screen.getByText('app-volume')).toBeInTheDocument()
    expect(screen.getByText('completed')).toBeInTheDocument()
    expect(screen.getByText('running')).toBeInTheDocument()
    expect(screen.getByText('1 MB')).toBeInTheDocument()
  })

  it('should display system status', async () => {
    // Mock system status API
    server.use(
      http.get('/api/v1/system/status', () => {
        return HttpResponse.json({
          docker_available: true,
          docker_version: '20.10.21',
          storage_usage: {
            used: 21474836480, // 20 GB
            total: 107374182400, // 100 GB
            used_human: '20 GB',
            total_human: '100 GB',
            percentage: 20,
          },
          backup_engine_status: 'running',
          scheduler_status: 'running',
          active_backups: 2,
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('Docker Available')).toBeInTheDocument()
    })
    
    expect(screen.getByText('20.10.21')).toBeInTheDocument()
    expect(screen.getByText('20 GB')).toBeInTheDocument()
    expect(screen.getByText('100 GB')).toBeInTheDocument()
    expect(screen.getByText('20%')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument() // Active backups
  })

  it('should display backup activity chart', async () => {
    // Mock chart data API
    server.use(
      http.get('/api/v1/stats/activity', () => {
        return HttpResponse.json({
          daily_stats: [
            { date: '2024-01-01', successful: 5, failed: 0 },
            { date: '2024-01-02', successful: 3, failed: 1 },
            { date: '2024-01-03', successful: 7, failed: 0 },
            { date: '2024-01-04', successful: 4, failed: 2 },
            { date: '2024-01-05', successful: 6, failed: 0 },
          ]
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      // Look for chart container or chart-related text
      expect(screen.getByText(/backup activity/i)).toBeInTheDocument()
    })
    
    // If using Recharts, might see SVG elements
    const charts = document.querySelectorAll('svg')
    expect(charts.length).toBeGreaterThan(0)
  })

  it('should display Docker status warnings', async () => {
    // Mock Docker unavailable
    server.use(
      http.get('/api/v1/system/status', () => {
        return HttpResponse.json({
          docker_available: false,
          error: 'Docker daemon not running',
          backup_engine_status: 'error',
          scheduler_status: 'stopped',
          active_backups: 0,
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText(/docker.*not.*available/i)).toBeInTheDocument()
    })
    
    expect(screen.getByText(/error/i)).toBeInTheDocument()
    expect(screen.getByText(/stopped/i)).toBeInTheDocument()
  })

  it('should handle loading states', () => {
    // Override with pending promises
    server.use(
      http.get('/api/v1/stats/overview', () => new Promise(() => {})),
      http.get('/api/v1/backups', () => new Promise(() => {})),
      http.get('/api/v1/system/status', () => new Promise(() => {}))
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    // Should show loading indicators
    expect(screen.getByText(/loading/i)).toBeInTheDocument()
  })

  it('should handle API errors gracefully', async () => {
    server.use(
      http.get('/api/v1/stats/overview', () => {
        return new HttpResponse(null, {
          status: 500,
          statusText: 'Internal Server Error',
        })
      }),
      http.get('/api/v1/backups', () => {
        return new HttpResponse(null, {
          status: 503,
          statusText: 'Service Unavailable',
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText(/error.*loading/i)).toBeInTheDocument()
    })
  })

  it('should refresh data automatically', async () => {
    vi.useFakeTimers()
    
    let callCount = 0
    server.use(
      http.get('/api/v1/stats/overview', () => {
        callCount++
        return HttpResponse.json({
          total_backups: 150 + callCount,
          successful_backups: 142,
          failed_backups: 8,
          total_size: 52428800,
          total_size_human: '50 MB',
          active_targets: 12,
          scheduled_jobs: 5,
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('151')).toBeInTheDocument() // Initial call
    })
    
    // Advance time to trigger refresh (assuming 30-second interval)
    vi.advanceTimersByTime(30000)
    
    await waitFor(() => {
      expect(screen.getByText('152')).toBeInTheDocument() // Second call
    })
    
    vi.useRealTimers()
  })

  it('should display quick actions', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText(/dashboard/i)).toBeInTheDocument()
    })
    
    // Look for quick action buttons
    expect(screen.getByRole('button', { name: /create.*backup/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /view.*all.*backups/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /manage.*targets/i })).toBeInTheDocument()
  })

  it('should be accessible', async () => {
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText(/dashboard/i)).toBeInTheDocument()
    })
    
    // Check for proper headings structure
    const mainHeading = screen.getByRole('heading', { level: 1 })
    expect(mainHeading).toBeInTheDocument()
    
    // Check for section headings
    const sectionHeadings = screen.getAllByRole('heading', { level: 2 })
    expect(sectionHeadings.length).toBeGreaterThan(0)
    
    // Check for proper landmark regions
    const main = screen.getByRole('main')
    expect(main).toBeInTheDocument()
  })

  it('should show storage usage warning when high', async () => {
    server.use(
      http.get('/api/v1/system/status', () => {
        return HttpResponse.json({
          docker_available: true,
          docker_version: '20.10.21',
          storage_usage: {
            used: 96636764160, // 90 GB
            total: 107374182400, // 100 GB
            used_human: '90 GB',
            total_human: '100 GB',
            percentage: 90,
          },
          backup_engine_status: 'running',
          scheduler_status: 'running',
          active_backups: 0,
        })
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText('90%')).toBeInTheDocument()
    })
    
    // Should show warning indicator or text
    expect(screen.getByText(/warning/i)).toBeInTheDocument()
  })

  it('should display upcoming scheduled backups', async () => {
    server.use(
      http.get('/api/v1/schedules/upcoming', () => {
        return HttpResponse.json([
          {
            target_name: 'database-volume',
            next_run: '2024-01-02T02:00:00Z',
            cron_expression: '0 2 * * *',
          },
          {
            target_name: 'app-volume',
            next_run: '2024-01-02T06:00:00Z',
            cron_expression: '0 6 * * *',
          },
        ])
      })
    )
    
    render(<Dashboard />, { wrapper: createWrapper() })
    
    await waitFor(() => {
      expect(screen.getByText(/upcoming.*schedules/i)).toBeInTheDocument()
    })
    
    expect(screen.getByText('database-volume')).toBeInTheDocument()
    expect(screen.getByText('app-volume')).toBeInTheDocument()
  })
})
