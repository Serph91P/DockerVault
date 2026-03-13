/* eslint-disable react-refresh/only-export-components */
import { useEffect, useState, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { Keyboard, X } from 'lucide-react'

const SHORTCUTS = [
  { key: 'g d', description: 'Go to Dashboard' },
  { key: 'g b', description: 'Go to Backups' },
  { key: 'g s', description: 'Go to Schedules' },
  { key: 'g r', description: 'Go to Retention' },
  { key: 'g o', description: 'Go to Remote Storage' },
  { key: 'g e', description: 'Go to Settings' },
  { key: '?', description: 'Show keyboard shortcuts' },
]

function ShortcutHelp({ isOpen, onClose }: { isOpen: boolean; onClose: () => void }) {
  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-dark-800 rounded-2xl border border-dark-700 shadow-2xl w-full max-w-sm p-6">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold text-dark-100 flex items-center gap-2">
            <Keyboard className="w-5 h-5" />
            Keyboard Shortcuts
          </h2>
          <button
            onClick={onClose}
            className="p-1 text-dark-400 hover:text-dark-200 rounded-lg hover:bg-dark-700 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="space-y-2">
          {SHORTCUTS.map(({ key, description }) => (
            <div key={key} className="flex items-center justify-between py-1.5">
              <span className="text-sm text-dark-300">{description}</span>
              <div className="flex gap-1">
                {key.split(' ').map((k) => (
                  <kbd
                    key={k}
                    className="px-2 py-0.5 bg-dark-700 border border-dark-600 rounded text-xs text-dark-200 font-mono min-w-[24px] text-center"
                  >
                    {k}
                  </kbd>
                ))}
              </div>
            </div>
          ))}
        </div>
        <p className="text-xs text-dark-500 mt-4">
          Shortcuts are disabled when typing in input fields.
        </p>
      </div>
    </div>
  )
}

export function useKeyboardShortcuts() {
  const navigate = useNavigate()
  const [showHelp, setShowHelp] = useState(false)
  const [pendingPrefix, setPendingPrefix] = useState<string | null>(null)

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Ignore when typing in inputs, textareas, or contenteditable
      const target = e.target as HTMLElement
      if (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable
      ) {
        return
      }

      // Ignore when modifiers are held (except shift for ?)
      if (e.ctrlKey || e.altKey || e.metaKey) return

      const key = e.key.toLowerCase()

      // Handle "g" prefix sequences
      if (pendingPrefix === 'g') {
        setPendingPrefix(null)
        switch (key) {
          case 'd':
            e.preventDefault()
            navigate('/')
            return
          case 'b':
            e.preventDefault()
            navigate('/backups')
            return
          case 's':
            e.preventDefault()
            navigate('/schedules')
            return
          case 'r':
            e.preventDefault()
            navigate('/retention')
            return
          case 'o':
            e.preventDefault()
            navigate('/storage')
            return
          case 'e':
            e.preventDefault()
            navigate('/settings')
            return
        }
        return
      }

      // Start "g" prefix
      if (key === 'g') {
        setPendingPrefix('g')
        // Clear prefix after 1 second if no follow-up key
        setTimeout(() => setPendingPrefix(null), 1000)
        return
      }

      // Show shortcut help
      if (key === '?' || (e.shiftKey && key === '/')) {
        e.preventDefault()
        setShowHelp((prev) => !prev)
        return
      }

      // Escape closes help
      if (key === 'escape' && showHelp) {
        setShowHelp(false)
        return
      }
    },
    [navigate, pendingPrefix, showHelp]
  )

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [handleKeyDown])

  return {
    showHelp,
    setShowHelp,
    ShortcutHelp,
  }
}
