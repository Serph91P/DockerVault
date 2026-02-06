import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'
import Backups from '../Backups'
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

describe('Backups Page', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('should display page title and description', async () => {
    render(<Backups />, { wrapper: createWrapper() })
    
    expect(screen.getByText('Backups')).toBeInTheDocument()
    expect(screen.getByText('Manage your backup configurations and history')).toBeInTheDocument()
  })

  it('should display New Backup button', async () => {
    render(<Backups />, { wrapper: createWrapper() })
    
    expect(screen.getByRole('button', { name: /New Backup/i })).toBeInTheDocument()
  })

  it('should display loading skeleton initially', () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return new Promise(() => {})
      })
    )

    render(<Backups />, { wrapper: createWrapper() })
    
    const skeletonRows = document.querySelectorAll('.animate-pulse')
    expect(skeletonRows.length).toBeGreaterThan(0)
  })

  it('should display statistics cards', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Active Backups')).toBeInTheDocument()
      expect(screen.getByText('Total Backups')).toBeInTheDocument()
      expect(screen.getByText('Completed')).toBeInTheDocument()
      expect(screen.getByText('Failed')).toBeInTheDocument()
    })
  })

  it('should display empty state when no targets configured', async () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return HttpResponse.json([])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('No backups configured')).toBeInTheDocument()
    })
  })

  it('should display configured backup targets', async () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return HttpResponse.json([
          {
            id: 1,
            name: 'Test Backup',
            target_type: 'container',
            container_name: 'nginx',
            enabled: true,
            created_at: '2024-01-01T00:00:00Z',
          },
        ])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Test Backup')).toBeInTheDocument()
    })
  })

  it('should show disabled badge for disabled targets', async () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return HttpResponse.json([
          {
            id: 1,
            name: 'Disabled Backup',
            target_type: 'volume',
            volume_name: 'test-vol',
            enabled: false,
            created_at: '2024-01-01T00:00:00Z',
          },
        ])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Disabled')).toBeInTheDocument()
    })
  })

  it('should display Create Backup button in empty state', async () => {
    server.use(
      http.get('/api/v1/targets', () => {
        return HttpResponse.json([])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Backup/i })).toBeInTheDocument()
    })
  })
})
