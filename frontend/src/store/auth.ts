import { create } from 'zustand';
import api from '../api';
import { clearSession, onSessionExpired } from './session';
import { onSocketSessionExpired } from './websocket';
import { bindNotificationOwner } from './notifications';

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
  statusError: string | null;
  logoutError: string | null;
  checkAuthStatus: () => Promise<void>;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  setup: (username: string, password: string) => Promise<void>;
}

let authGeneration = 0;

export const useAuthStore = create<AuthState>((set) => ({
  user: null,
  isAuthenticated: false,
  isLoading: true,
  setupRequired: false,
  statusError: null,
  logoutError: null,

  checkAuthStatus: async () => {
    const generation = ++authGeneration;
    set({ isLoading: true, statusError: null });
    try {
      const response = await api.get('/auth/status');
      if (generation !== authGeneration) return;
      const { authenticated, setup_complete, user } = response.data;
      if (typeof authenticated !== 'boolean' || typeof setup_complete !== 'boolean') {
        throw new Error('Invalid authentication status');
      }
      if (authenticated && (!user || !Number.isSafeInteger(user.id) || user.id <= 0)) throw new Error('Invalid authenticated user');
      const previousUser = useAuthStore.getState().user;
      if (!authenticated || previousUser?.id !== user?.id) clearSession(authenticated ? user?.id : undefined, authenticated && previousUser === null ? user.id : undefined);
      if (authenticated) bindNotificationOwner(user.id);
      set({
        isAuthenticated: authenticated,
        setupRequired: setup_complete === false,
        user: authenticated ? user || null : null,
        isLoading: false,
      });
    } catch {
      if (generation !== authGeneration) return;
      clearSession();
      set({
        isAuthenticated: false,
        setupRequired: false,
        user: null,
        isLoading: false,
        statusError: 'Unable to check authentication status. Please retry.',
      });
    }
  },

  login: async (username, password) => {
    const generation = ++authGeneration;
    clearSession();
    set({ user: null, isAuthenticated: false, logoutError: null });
    const response = await api.post('/auth/login', { username, password });
    if (generation !== authGeneration) throw new Error('Session changed');
    bindNotificationOwner(response.data.user.id);
    set({ isAuthenticated: true, user: response.data.user, setupRequired: false, statusError: null });
  },

  logout: async () => {
    const generation = ++authGeneration;
    set({ isAuthenticated: false, user: null, setupRequired: false, isLoading: true, statusError: null, logoutError: null });
    clearSession();
    // Local isolation happens immediately, even if server revocation fails.
    try {
      await api.post('/auth/logout');
    } catch {
      if (generation === authGeneration) set({ logoutError: 'Local session cleared, but server sign-out could not be confirmed. Retry sign-out before leaving this browser.' });
    } finally {
      if (generation === authGeneration) set({ isLoading: false });
    }
  },

  setup: async (username, password) => {
    const generation = ++authGeneration;
    clearSession();
    const response = await api.post('/auth/setup', {
      username,
      password,
      confirm_password: password,
    });
    if (generation !== authGeneration) throw new Error('Session changed');
    bindNotificationOwner(response.data.user.id);
    set({ isAuthenticated: true, user: response.data.user, setupRequired: false, statusError: null });
  },
}));

function expireLocalSession() {
  ++authGeneration;
  useAuthStore.setState({ isAuthenticated: false, user: null, setupRequired: false, isLoading: false, statusError: null });
  clearSession();
}
onSessionExpired(expireLocalSession);
onSocketSessionExpired(expireLocalSession);
