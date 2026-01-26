import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useWebSocketStore } from '../websocket'
import { act, renderHook } from '@testing-library/react'

// Mock WebSocket using vi.stubGlobal which works in newer jsdom versions
class MockWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  
  readyState = MockWebSocket.CONNECTING
  onopen?: () => void
  onclose?: () => void
  onmessage?: (event: MessageEvent) => void
  onerror?: () => void
  
  constructor(public url: string) {
    // Simulate connection opening after a tick
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN
      this.onopen?.()
    }, 0)
  }
  
  send = vi.fn()
  close = vi.fn(() => {
    this.readyState = MockWebSocket.CLOSED
    this.onclose?.()
  })
  
  // Helper method to simulate receiving messages
  simulateMessage(data: Record<string, unknown>) {
    const event = new MessageEvent('message', {
      data: JSON.stringify(data)
    })
    this.onmessage?.(event)
  }
  
  // Helper method to simulate connection error
  simulateError() {
    this.onerror?.()
  }
}

describe('WebSocket Store', () => {
  let mockWebSocket: MockWebSocket
  
  beforeEach(() => {
    // Create a factory function that captures the instance
    const createMockWebSocket = (url: string): MockWebSocket => {
      const instance = new MockWebSocket(url)
      mockWebSocket = instance
      return instance
    }
    
    // Create constructor that uses the factory
    const MockWebSocketConstructor = function(url: string) {
      return createMockWebSocket(url)
    } as unknown as typeof WebSocket
    
    // Add static constants
    Object.defineProperty(MockWebSocketConstructor, 'CONNECTING', { value: 0 })
    Object.defineProperty(MockWebSocketConstructor, 'OPEN', { value: 1 })
    Object.defineProperty(MockWebSocketConstructor, 'CLOSING', { value: 2 })
    Object.defineProperty(MockWebSocketConstructor, 'CLOSED', { value: 3 })
    
    vi.stubGlobal('WebSocket', MockWebSocketConstructor)
    
    // Reset store state
    useWebSocketStore.setState({
      connected: false,
      backupProgress: new Map(),
      recentBackups: [],
    })
  })
  
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetAllMocks()
  })

  it('should initialize with default state', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    expect(result.current.connected).toBe(false)
    expect(result.current.backupProgress.size).toBe(0)
    expect(result.current.recentBackups).toHaveLength(0)
  })

  it('should connect to WebSocket', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    // mockWebSocket should be created
    expect(mockWebSocket).toBeDefined()
    
    // Wait for connection to open
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    expect(result.current.connected).toBe(true)
  })

  it('should handle connection errors', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    
    act(() => {
      result.current.connect()
    })
    
    // Wait for connection
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Simulate error
    act(() => {
      mockWebSocket.simulateError()
    })
    
    expect(consoleSpy).toHaveBeenCalled()
    consoleSpy.mockRestore()
  })

  it('should disconnect from WebSocket', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    act(() => {
      result.current.disconnect()
    })
    
    expect(result.current.connected).toBe(false)
  })

  it('should handle backup progress messages', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Simulate backup progress message
    act(() => {
      mockWebSocket.simulateMessage({
        type: 'backup_progress',
        backup_id: 123,
        progress: 50,
        message: 'Backing up files...',
      })
    })
    
    expect(result.current.backupProgress.has(123)).toBe(true)
    expect(result.current.backupProgress.get(123)?.progress).toBe(50)
  })

  it('should handle backup completion messages', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Add progress first
    act(() => {
      result.current.updateProgress({
        backup_id: 123,
        progress: 100,
        message: 'Completing...',
      })
    })
    
    expect(result.current.backupProgress.has(123)).toBe(true)
    
    // Simulate backup completed message
    act(() => {
      mockWebSocket.simulateMessage({
        type: 'backup_completed',
        backup_id: 123,
      })
    })
    
    // Progress should be removed after completion
    expect(result.current.backupProgress.has(123)).toBe(false)
  })

  it('should handle backup failure messages', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Add progress first
    act(() => {
      result.current.updateProgress({
        backup_id: 456,
        progress: 30,
        message: 'In progress...',
      })
    })
    
    // Simulate backup failed message
    act(() => {
      mockWebSocket.simulateMessage({
        type: 'backup_failed',
        backup_id: 456,
        error: 'Disk full',
      })
    })
    
    // Progress should be removed after failure
    expect(result.current.backupProgress.has(456)).toBe(false)
  })

  it('should update progress correctly', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.updateProgress({
        backup_id: 1,
        progress: 25,
        message: 'Starting...',
      })
    })
    
    expect(result.current.backupProgress.get(1)?.progress).toBe(25)
    
    act(() => {
      result.current.updateProgress({
        backup_id: 1,
        progress: 75,
        message: 'Almost done...',
      })
    })
    
    expect(result.current.backupProgress.get(1)?.progress).toBe(75)
  })

  it('should add recent backups', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.addRecentBackup({
        id: 1,
        target_id: 1,
        target_name: 'test-backup',
        backup_type: 'full',
        status: 'completed',
        created_at: '2024-01-01T10:00:00Z',
      })
    })
    
    expect(result.current.recentBackups).toHaveLength(1)
    expect(result.current.recentBackups[0].target_name).toBe('test-backup')
  })

  it('should limit recent backups to 10 items', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    // Add 15 backups
    for (let i = 1; i <= 15; i++) {
      act(() => {
        result.current.addRecentBackup({
          id: i,
          target_id: 1,
          target_name: `backup-${i}`,
          backup_type: 'full',
          status: 'completed',
          created_at: `2024-01-01T${String(i).padStart(2, '0')}:00:00Z`,
        })
      })
    }
    
    // Should only keep 10 most recent
    expect(result.current.recentBackups).toHaveLength(10)
    // Most recent should be first
    expect(result.current.recentBackups[0].id).toBe(15)
  })

  it('should handle unknown message types gracefully', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Simulate unknown message type
    act(() => {
      mockWebSocket.simulateMessage({
        type: 'unknown_type',
        data: 'some data',
      })
    })
    
    expect(consoleSpy).toHaveBeenCalledWith('Unknown WebSocket message type:', 'unknown_type')
    consoleSpy.mockRestore()
  })

  it('should handle invalid JSON messages gracefully', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Simulate invalid JSON message
    act(() => {
      const event = new MessageEvent('message', {
        data: 'not valid json'
      })
      mockWebSocket.onmessage?.(event)
    })
    
    expect(consoleSpy).toHaveBeenCalled()
    consoleSpy.mockRestore()
  })
})
