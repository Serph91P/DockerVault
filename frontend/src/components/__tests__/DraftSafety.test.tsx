import { afterEach, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import BackupWizard from '../BackupWizard'
import { http, HttpResponse } from 'msw'
import { initialData } from '../BackupWizard/planEditorModel'
import { draftKey } from '../BackupWizard/planEditorDraft'
import { useAuthStore } from '../../store/auth'
import { server } from '../../test/mocks/server'

afterEach(() => { cleanup(); localStorage.clear() })
it('sanitizes legacy hook bodies on read while preserving useful draft choices', async () => {
  const marker = 'synthetic-sensitive-hook'
  server.use(http.get('/api/v1/retention', () => HttpResponse.json([])), http.get('/api/v1/storage', () => HttpResponse.json([])))
  useAuthStore.setState({ user: { id: 7, username: 'fixture', is_admin: true, created_at: '', last_login: null }, isAuthenticated: true })
  localStorage.setItem('dockervault-wizard-draft', JSON.stringify({
    ownerId: 7, currentStep: 7, savedAt: Date.now(), data: { ...initialData, targetType: 'path', targetName: 'Synthetic plan', hostPath: '/synthetic', preCommand: marker, postCommand: marker, private_key: marker },
  }))
  render(<QueryClientProvider client={new QueryClient()}><BackupWizard isOpen onClose={() => {}} /></QueryClientProvider>)
  expect((localStorage.getItem('dockervault-wizard-draft') ?? '').includes(marker)).toBe(false)
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Restore draft' }))
  await user.click(screen.getByRole('button', { name: 'Advanced Options' }))
  expect(screen.getByPlaceholderText(/pg_dump/i)).toHaveValue('')
  await user.type(screen.getByPlaceholderText(/pg_dump/i), marker)
  expect(JSON.stringify(localStorage).includes(marker)).toBe(false)
  expect(JSON.parse(localStorage.getItem(draftKey(7, 'new'))!).data.hostPath).toBe('/synthetic')
})
