import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
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
  let user: ReturnType<typeof userEvent.setup>

  beforeEach(() => {
    user = userEvent.setup()
    vi.clearAllMocks()
  })

  it('should display page title and description', async () => {
    render(<Backups />, { wrapper: createWrapper() })
    
    expect(screen.getByText('Backups')).toBeInTheDocument()
    expect(screen.getByText('Manage backups for your Docker resources')).toBeInTheDocument()
  })

  it('should display three tabs for containers, volumes, and stacks', async () => {
    render(<Backups />, { wrapper: createWrapper() })
    
    expect(screen.getByRole('button', { name: /Containers/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Volumes/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Stacks/i })).toBeInTheDocument()
  })

  it('should display loading skeleton initially', () => {
    // Override with a pending promise
    server.use(
      http.get('/api/v1/docker/containers', () => {
        return new Promise(() => {}) // Never resolves
      })
    )

    render(<Backups />, { wrapper: createWrapper() })
    
    // Check for skeleton rows with animate-pulse class
    const skeletonRows = document.querySelectorAll('.animate-pulse')
    expect(skeletonRows.length).toBeGreaterThan(0)
  })

  it('should display containers when loaded on containers tab', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })
  })

  it('should switch to volumes tab and display volumes', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    // Wait for initial load
    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })

    // Click on Volumes tab
    const volumesTab = screen.getByRole('button', { name: /Volumes/i })
    await user.click(volumesTab)

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })
  })

  it('should switch to stacks tab and display stacks', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    // Wait for initial load
    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })

    // Click on Stacks tab
    const stacksTab = screen.getByRole('button', { name: /Stacks/i })
    await user.click(stacksTab)

    await waitFor(() => {
      expect(screen.getByText('myapp')).toBeInTheDocument()
    })
  })

  it('should display search input', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    const searchInput = screen.getByPlaceholderText('Search...')
    expect(searchInput).toBeInTheDocument()
  })

  it('should filter items when searching', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })

    const searchInput = screen.getByPlaceholderText('Search...')
    await user.type(searchInput, 'nginx')

    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })
  })

  it('should display sort options', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    const sortSelect = screen.getByRole('combobox')
    expect(sortSelect).toBeInTheDocument()
  })

  it('should display only with backup filter checkbox', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    const checkbox = screen.getByRole('checkbox')
    expect(checkbox).toBeInTheDocument()
    expect(screen.getByText('Only with backup')).toBeInTheDocument()
  })

  it('should display statistics cards at the bottom', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Active Backups')).toBeInTheDocument()
      expect(screen.getByText('Completed')).toBeInTheDocument()
      expect(screen.getByText('Failed')).toBeInTheDocument()
    })
  })

  it('should display empty state when no items match search', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('nginx-container')).toBeInTheDocument()
    })

    const searchInput = screen.getByPlaceholderText('Search...')
    await user.type(searchInput, 'nonexistent-item-12345')

    await waitFor(() => {
      expect(screen.getByText('No items match your search')).toBeInTheDocument()
    })
  })
})
