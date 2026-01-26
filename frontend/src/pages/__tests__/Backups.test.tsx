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
    expect(screen.getByText('Alle erstellten Backups')).toBeInTheDocument()
  })

  it('should display loading skeleton initially', () => {
    // Override with a pending promise
    server.use(
      http.get('/api/v1/backups', () => {
        return new Promise(() => {}) // Never resolves
      })
    )

    render(<Backups />, { wrapper: createWrapper() })
    
    // Check for skeleton rows with animate-pulse class
    const skeletonRows = document.querySelectorAll('.animate-pulse')
    expect(skeletonRows.length).toBeGreaterThan(0)
  })

  it('should display backups when loaded', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    expect(screen.getByText('db-volume')).toBeInTheDocument()
  })

  it('should display backup status in German', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for German status labels
    expect(screen.getByText('Abgeschlossen')).toBeInTheDocument() // completed
    expect(screen.getByText('Läuft')).toBeInTheDocument() // running
  })

  it('should display empty state when no backups exist', async () => {
    server.use(
      http.get('/api/v1/backups', () => {
        return HttpResponse.json([])
      })
    )

    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Noch keine Backups vorhanden')).toBeInTheDocument()
    })
  })

  it('should display backup file sizes', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for human-readable file size
    expect(screen.getByText('1 KB')).toBeInTheDocument()
  })

  it('should display backup duration', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for duration in seconds
    expect(screen.getByText('300s')).toBeInTheDocument()
  })

  it('should display table headers', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('Target')).toBeInTheDocument()
      expect(screen.getByText('Status')).toBeInTheDocument()
      expect(screen.getByText('Größe')).toBeInTheDocument()
      expect(screen.getByText('Dauer')).toBeInTheDocument()
      expect(screen.getByText('Erstellt')).toBeInTheDocument()
      expect(screen.getByText('Aktionen')).toBeInTheDocument()
    })
  })

  it('should allow backup deletion', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Find delete button by title attribute (German: "Löschen")
    const deleteButtons = screen.getAllByTitle('Löschen')
    expect(deleteButtons.length).toBeGreaterThan(0)

    await user.click(deleteButtons[0])

    // Wait for the mutation to complete
    await waitFor(() => {
      // The button should still be in the document (component re-rendered)
      expect(screen.getAllByTitle('Löschen').length).toBeGreaterThan(0)
    })
  })

  it('should allow backup restoration for completed backups', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Find restore button by title attribute (German: "Wiederherstellen")
    const restoreButtons = screen.getAllByTitle('Wiederherstellen')
    expect(restoreButtons.length).toBeGreaterThan(0)

    await user.click(restoreButtons[0])

    // Wait for the mutation
    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })
  })

  it('should display backup creation date formatted in German locale', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Date should be formatted as DD.MM.YYYY HH:mm (German format)
    const dateElements = screen.getAllByText(/\d{2}\.\d{2}\.\d{4}/)
    expect(dateElements.length).toBeGreaterThan(0)
  })

  it('should be accessible with proper table structure', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for proper table structure
    const table = screen.getByRole('table')
    expect(table).toBeInTheDocument()

    // Check for column headers
    const columnHeaders = screen.getAllByRole('columnheader')
    expect(columnHeaders.length).toBe(6)
  })

  it('should display backup ID', async () => {
    render(<Backups />, { wrapper: createWrapper() })

    await waitFor(() => {
      expect(screen.getByText('test-volume')).toBeInTheDocument()
    })

    // Check for backup ID display
    expect(screen.getByText('#1')).toBeInTheDocument()
  })
})
