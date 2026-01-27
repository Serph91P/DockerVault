import { create } from 'zustand';
import api from '../api';

interface User {
  id: number;
  username: string;
  is_admin: boolean;
  created_at: string;
  last_login: string | null;
}

interface AuthState {
  user: User | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  setupRequired: boolean;
  
  // Actions
  checkAuthStatus: () => Promise<void>;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  setup: (username: string, password: string) => Promise<void>;
}

export const useAuthStore = create<AuthState>((set) => ({
  user: null,
  isAuthenticated: false,
  isLoading: true,
  setupRequired: false,
  
  checkAuthStatus: async () => {
    set({ isLoading: true });
    try {
      const response = await api.get('/auth/status');
      const { authenticated, setup_complete, user } = response.data;
      
      set({
        isAuthenticated: authenticated,
        setupRequired: !setup_complete,
        user: user || null,
        isLoading: false,
      });
    } catch {
      set({
        isAuthenticated: false,
        setupRequired: true,
        user: null,
        isLoading: false,
      });
    }
  },
  
  login: async (username: string, password: string) => {
    const response = await api.post('/auth/login', { username, password });
    const { user } = response.data;
    
    set({
      isAuthenticated: true,
      user,
      setupRequired: false,
    });
  },
  
  logout: async () => {
    try {
      await api.post('/auth/logout');
    } finally {
      set({
        isAuthenticated: false,
        user: null,
      });
    }
  },
  
  setup: async (username: string, password: string) => {
    const response = await api.post('/auth/setup', {
      username,
      password,
      confirm_password: password,
    });
    const { user } = response.data;
    
    set({
      isAuthenticated: true,
      user,
      setupRequired: false,
    });
  },
}));
