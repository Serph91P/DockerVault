import React from 'react'
import ReactDOM from 'react-dom/client'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from './store/session'
import { createBrowserRouter, RouterProvider } from 'react-router-dom'
import { Toaster } from 'react-hot-toast'
import App from './App'
import './index.css'


const router = createBrowserRouter([{ path: '*', element: <App /> }])

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <>
        <RouterProvider router={router} />
        <Toaster
          position="bottom-right"
          toastOptions={{
            className: 'bg-dark-800 text-dark-100',
            style: {
              background: '#1e293b',
              color: '#f1f5f9',
            },
          }}
        />
      </>
    </QueryClientProvider>
  </React.StrictMode>,
)
