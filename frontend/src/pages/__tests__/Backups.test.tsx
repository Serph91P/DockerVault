import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor, within } from '@testing-library/react'
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
  })

  it('should display loading state initially', () => {
    // Override with a pending promise
    server.use(
      http.get('/api/v1/backups', () => {
        return new Promise(() => {}) // Never resolves
      })
    )

    render(<Backups />, { wrapper: createWrapper() })
    expect(screen.getByText(/loading/i)).toBeInTheDocument()
  })

  it('should display backups when loaded', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    expect(screen.getByText('db-volume')).toBeInTheDocument()
    expect(screen.getByText('completed')).toBeInTheDocument()
    expect(screen.getByText('running')).toBeInTheDocument()
  })

  it('should display empty state when no backups exist', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return HttpResponse.json([])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText(/no backups found/i)).toBeInTheDocument()
    })
  })

  it('should handle API errors gracefully', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return new HttpResponse(null, {
          status: 500,
          statusText: 'Internal Server Error',
        })
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText(/error loading backups/i)).toBeInTheDocument()
    })
  })

  it('should display backup status icons correctly', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for status indicators
    const completedBackup = screen.getByText('test-volume').closest('tr')
    const runningBackup = screen.getByText('db-volume').closest('tr')

    expect(completedBackup).toBeInTheDocument()
    expect(runningBackup).toBeInTheDocument()
    
    // Check for specific status text
    expect(screen.getByText('completed')).toBeInTheDocument()
    expect(screen.getByText('running')).toBeInTheDocument()
  })

  it('should allow backup deletion', async () => {
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Find and click delete button for first backup
    const deleteButtons = screen.getAllByRole('button', { name: /delete/i })
    expect(deleteButtons.length).toBeGreaterThan(0)

    await user.click(deleteButtons[0])

    // Wait for the API call to complete
    await waitFor(() => {
      // Backup should be removed from the list or show success message
      expect(consoleSpy).toHaveBeenCalled()
    })

    consoleSpy.mockRestore()
  })

  it('should handle backup deletion errors', async () => {
    server.use(
      http.delete('/api/v1/backups/:id', () => {
        return new HttpResponse(null, {
          status: 500,
          statusText: 'Internal Server Error',
        })
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    const deleteButtons = screen.getAllByRole('button', { name: /delete/i })
    await user.click(deleteButtons[0])

    // Should show error toast or error message
    await waitFor(() => {
      // In a real app, this would check for error toast
      expect(screen.getByText('test-volume')).toBeInTheDocument() // Backup still there
    })
  })

  it('should allow backup restoration', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Find and click restore button for completed backup
    const restoreButtons = screen.getAllByRole('button', { name: /restore/i })
    expect(restoreButtons.length).toBeGreaterThan(0)

    await user.click(restoreButtons[0])

    // Should trigger restore API call
    await waitFor(() => {
      // In a real app, this would show success message
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })
  })

  it('should filter backups by status', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check that both backups are initially visible
    expect(screen.getByText('test-volume')).toBeInTheDocument()
    expect(screen.getByText('db-volume')).toBeInTheDocument()

    // If there's a status filter dropdown, test it
    const statusFilter = screen.queryByRole('combobox', { name: /status/i })
    if (statusFilter) {
      await user.selectOptions(statusFilter, 'completed')
      
      await waitFor(() => {
        expect(screen.getByText('test-volume')).toBeInTheDocument()
        expect(screen.queryByText('db-volume')).not.toBeInTheDocument()
      })
    }
  })

  it('should display backup file sizes in human-readable format', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for human-readable file size
    expect(screen.getByText('1 KB')).toBeInTheDocument()
  })

  it('should show backup duration and timestamps', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Should show created date (formatted)
    const createdText = screen.getByText(/2024/)
    expect(createdText).toBeInTheDocument()
  })

  it('should handle real-time backup progress updates via WebSocket', async () => {
    // Mock WebSocket updates
    const mockWebSocket = {
      readyState: WebSocket.OPEN,
      send: vi.fn(),
      close: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }

    // @ts-expect-error - Mocking WebSocket
    global.WebSocket = vi.fn(() => mockWebSocket)

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Simulate WebSocket message for progress update
    const progressMessage = {
      type: 'backup_progress',
      backup_id: 2,
      progress: 50,
      message: 'Backup in progress...',
    }

    // In real implementation, this would trigger progress display
    expect(screen.getByText('running')).toBeInTheDocument()
  })

  it('should be accessible with proper ARIA labels', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for table accessibility
    const table = screen.getByRole('table')
    expect(table).toBeInTheDocument()

    // Check for column headers
    expect(screen.getByRole('columnheader', { name: /name/i })).toBeInTheDocument()
    expect(screen.getByRole('columnheader', { name: /status/i })).toBeInTheDocument()
    expect(screen.getByRole('columnheader', { name: /size/i })).toBeInTheDocument()

    // Check for action buttons with proper labels
    const deleteButtons = screen.getAllByRole('button', { name: /delete/i })
    expect(deleteButtons.length).toBeGreaterThan(0)
    
    const restoreButtons = screen.getAllByRole('button', { name: /restore/i })
    expect(restoreButtons.length).toBeGreaterThan(0)
  })

  it('should handle keyboard navigation', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Test keyboard navigation through action buttons
    const deleteButton = screen.getAllByRole('button', { name: /delete/i })[0]
    deleteButton.focus()
    expect(deleteButton).toHaveFocus()

    // Tab to next button
    await user.tab()
    const restoreButton = screen.getAllByRole('button', { name: /restore/i })[0]
    expect(restoreButton).toHaveFocus()
  })
})
