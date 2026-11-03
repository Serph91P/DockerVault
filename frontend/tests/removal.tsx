// Test-only mount of the existing, intentionally unrouted legacy entry point.
import React from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'
import Targets from '../src/pages/Targets'
import '../src/index.css'

createRoot(document.getElementById('root')!).render(<React.StrictMode><QueryClientProvider client={new QueryClient()}><BrowserRouter><Targets /></BrowserRouter></QueryClientProvider></React.StrictMode>)
