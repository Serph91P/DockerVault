import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useWebSocketStore } from '../websocket'
import { act, renderHook } from '@testing-library/react'

// Mock WebSocket
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
    // @ts-expect-error - Mocking WebSocket
    global.WebSocket = vi.fn((url: string) => {
      mockWebSocket = new MockWebSocket(url)
      return mockWebSocket
    })
    
    // Reset store state
    useWebSocketStore.getState().disconnect()
    useWebSocketStore.setState({
      connected: false,
      connecting: false,
      error: null,
      backupProgress: new Map(),
      notifications: [],
    })
  })
  
  afterEach(() => {
    vi.resetAllMocks()
  })

  it('should initialize with default state', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    expect(result.current.connected).toBe(false)
    expect(result.current.connecting).toBe(false)
    expect(result.current.error).toBe(null)
    expect(result.current.backupProgress.size).toBe(0)
    expect(result.current.notifications).toHaveLength(0)
  })

  it('should connect to WebSocket', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    expect(result.current.connecting).toBe(true)
    expect(WebSocket).toHaveBeenCalledWith('ws://localhost:8000/ws')
    
    // Wait for connection to open
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    expect(result.current.connected).toBe(true)
    expect(result.current.connecting).toBe(false)
  })

  it('should handle connection errors', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      mockWebSocket.simulateError()
    })
    
    expect(result.current.connected).toBe(false)
    expect(result.current.connecting).toBe(false)
    expect(result.current.error).toBeTruthy()
  })

  it('should disconnect from WebSocket', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    // First connect
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    expect(result.current.connected).toBe(true)
    
    // Then disconnect
    act(() => {
      result.current.disconnect()
    })
    
    expect(mockWebSocket.close).toHaveBeenCalled()
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
    
    const progressMessage = {
      type: 'backup_progress',
      backup_id: 1,
      progress: 50,
      message: 'Backup in progress...'
    }
    
    act(() => {
      mockWebSocket.simulateMessage(progressMessage)
    })
    
    const progress = result.current.backupProgress.get(1)
    expect(progress).toEqual({
      progress: 50,
      message: 'Backup in progress...'
    })
  })

  it('should handle backup completion messages', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // First set progress
    const progressMessage = {
      type: 'backup_progress',
      backup_id: 1,
      progress: 50,
      message: 'Backup in progress...'
    }
    
    act(() => {
      mockWebSocket.simulateMessage(progressMessage)
    })
    
    expect(result.current.backupProgress.has(1)).toBe(true)
    
    // Then complete backup
    const completionMessage = {
      type: 'backup_completed',
      backup_id: 1,
      status: 'completed',
      message: 'Backup completed successfully'
    }
    
    act(() => {
      mockWebSocket.simulateMessage(completionMessage)
    })
    
    expect(result.current.backupProgress.has(1)).toBe(false)
    expect(result.current.notifications).toHaveLength(1)
    expect(result.current.notifications[0]).toMatchObject({
      type: 'success',
      message: 'Backup completed successfully'
    })
  })

  it('should handle backup failure messages', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    const failureMessage = {
      type: 'backup_failed',
      backup_id: 1,
      error: 'Docker daemon not available'
    }
    
    act(() => {
      mockWebSocket.simulateMessage(failureMessage)
    })
    
    expect(result.current.backupProgress.has(1)).toBe(false)
    expect(result.current.notifications).toHaveLength(1)
    expect(result.current.notifications[0]).toMatchObject({
      type: 'error',
      message: 'Backup failed: Docker daemon not available'
    })
  })

  it('should clear notifications', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    // Add some notifications
    act(() => {
      result.current.addNotification({
        type: 'info',
        message: 'Test notification 1'
      })
      result.current.addNotification({
        type: 'success',
        message: 'Test notification 2'
      })
    })
    
    expect(result.current.notifications).toHaveLength(2)
    
    act(() => {
      result.current.clearNotifications()
    })
    
    expect(result.current.notifications).toHaveLength(0)
  })

  it('should remove specific notification', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.addNotification({
        id: 'test-1',
        type: 'info',
        message: 'Test notification 1'
      })
      result.current.addNotification({
        id: 'test-2',
        type: 'success',
        message: 'Test notification 2'
      })
    })
    
    expect(result.current.notifications).toHaveLength(2)
    
    act(() => {
      result.current.removeNotification('test-1')
    })
    
    expect(result.current.notifications).toHaveLength(1)
    expect(result.current.notifications[0].id).toBe('test-2')
  })

  it('should handle invalid JSON messages gracefully', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    // Simulate invalid JSON message
    const event = new MessageEvent('message', {
      data: 'invalid json'
    })
    
    act(() => {
      mockWebSocket.onmessage?.(event)
    })
    
    // Should not crash and should not add any notifications
    expect(result.current.notifications).toHaveLength(0)
    expect(result.current.connected).toBe(true) // Still connected
  })

  it('should handle unknown message types', async () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 10))
    })
    
    const unknownMessage = {
      type: 'unknown_message_type',
      data: 'some data'
    }
    
    act(() => {
      mockWebSocket.simulateMessage(unknownMessage)
    })
    
    // Should not crash and should not add any notifications
    expect(result.current.notifications).toHaveLength(0)
    expect(result.current.connected).toBe(true)
  })

  it('should automatically reconnect on connection loss', async () => {
    vi.useFakeTimers()
    
    const { result } = renderHook(() => useWebSocketStore())
    
    act(() => {
      result.current.connect()
    })
    
    await act(async () => {
      vi.advanceTimersByTime(10)
    })
    
    expect(result.current.connected).toBe(true)
    
    // Simulate connection loss
    act(() => {
      mockWebSocket.readyState = MockWebSocket.CLOSED
      mockWebSocket.onclose?.()
    })
    
    expect(result.current.connected).toBe(false)
    
    // Should attempt to reconnect after delay
    act(() => {
      vi.advanceTimersByTime(5000) // 5 second reconnect delay
    })
    
    await act(async () => {
      vi.advanceTimersByTime(10)
    })
    
    // Should have attempted to reconnect
    expect(WebSocket).toHaveBeenCalledTimes(2)
    
    vi.useRealTimers()
  })

  it('should limit maximum number of notifications', () => {
    const { result } = renderHook(() => useWebSocketStore())
    
    // Add many notifications (assuming max is 10)
    for (let i = 0; i < 15; i++) {
      act(() => {
        result.current.addNotification({
          type: 'info',
          message: `Test notification ${i}`
        })
      })
    }
    
    // Should not exceed maximum
    expect(result.current.notifications.length).toBeLessThanOrEqual(10)
  })
})
