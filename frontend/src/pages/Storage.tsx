import { queryKeys } from '../api/queryKeys'
import { QueryFeedback } from '../components/QueryFeedback'
import NotificationContext from '../components/NotificationContext'
import { useNotificationStore } from '../store/notifications'
import { useValidationFeedback } from '../components/ValidationFeedback'
import { useEffect, useState } from 'react';
import { operationOutcome, type Outcome } from '../api/outcomes'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  Cloud,
  Server,
  HardDrive,
  Key,
  Wifi,
  Plus,
  Edit,
  Trash2,
  TestTube,
  CheckCircle,
  XCircle,
  Loader2,
  FolderSync,
  Globe,
  Database,
  FolderOpen,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import api, { RemoteStorage } from '../api';
import { storageApi, type SSHKeyInfo } from '../api';
import StorageBrowser from '../components/StorageBrowser';
import ConfirmDialog from '../components/ConfirmDialog';
import { Modal } from '../components/ui/dialog';
import SSHKeyManager from '../components/SSHKeyManager';

interface StorageFormData {
  name: string;
  storage_type: string;
  enabled: boolean;
  host: string;
  port: string;
  username: string;
  password: string;
  base_path: string;
  ssh_key_path: string;
  s3_bucket: string;
  s3_region: string;
  s3_access_key: string;
  s3_secret_key: string;
  s3_endpoint_url: string;
  webdav_url: string;
  rclone_remote: string;
}

const STORAGE_TYPES = [
  { value: 'local', label: 'Local / NFS', icon: HardDrive, description: 'Local directory or NFS mount' },
  { value: 'ssh', label: 'SSH / SFTP', icon: Key, description: 'SSH server with rsync or SFTP' },
  { value: 's3', label: 'S3 Compatible', icon: Cloud, description: 'AWS S3, MinIO, Backblaze B2' },
  { value: 'webdav', label: 'WebDAV', icon: Globe, description: 'Nextcloud, ownCloud, etc.' },
  { value: 'ftp', label: 'FTP (unencrypted)', icon: Server, description: 'Plain FTP; use SFTP for secure transport. FTPS is not supported.' },
  { value: 'rclone', label: 'Rclone', icon: FolderSync, description: '40+ cloud providers (GDrive, Dropbox, ...)' },
];

const getStorageIcon = (type: string) => {
  const found = STORAGE_TYPES.find(t => t.value === type);
  return found?.icon || Database;
};

export default function Storage() {
  const queryClient = useQueryClient();
  const [showForm, setShowForm] = useState(false);
  const [editingStorage, setEditingStorage] = useState<RemoteStorage | null>(null);
  const [testingId, setTestingId] = useState<number | null>(null);
  useEffect(() => { try { localStorage.removeItem('storage-test-results') } catch { /* Never load unscoped provider results. */ } }, [])

  const [testResults, setTestResults] = useState<Record<number, { success: boolean; outcome: Outcome; message: string; testedAt: string; revision?: string }>>({});
  const saveTestResult = (id: number, result: { success: boolean; outcome: Outcome; message: string }, revision = queryClient.getQueryData<RemoteStorage[]>(queryKeys.storages.list())?.find(storage => storage.id === id)?.updated_at) => {
    useNotificationStore.getState().addNotification({ type: result.success ? 'success' : 'warning', title: `Storage #${id} test result`, message: result.message, phase: result.outcome })
    setTestResults((prev) => {
      const next = { ...prev, [id]: { ...result, revision, testedAt: new Date().toISOString() } };

      return next;
    });
  };
  const [browsingStorage, setBrowsingStorage] = useState<RemoteStorage | null>(null);
  const [localBrowserOpen, setLocalBrowserOpen] = useState(false);
  const [confirmDialog, setConfirmDialog] = useState<{ id: number; name: string } | null>(null);
  const [formStep, setFormStep] = useState(1); // ST1: 1=Name+Type, 2=Details, 3=Review

  const [formData, setFormData] = useState<StorageFormData>({
    name: '',
    storage_type: 'local',
    enabled: true,
    host: '',
    port: '',
    username: '',
    password: '',
    base_path: '/backups',
    ssh_key_path: '',
    s3_bucket: '',
    s3_region: 'eu-central-1',
    s3_access_key: '',
    s3_secret_key: '',
    s3_endpoint_url: '',
    webdav_url: '',
    rclone_remote: '',
  });

  const { data: storages, isLoading, ...storagesQuery } = useQuery<RemoteStorage[]>({
    queryKey: queryKeys.storages.list(),
    queryFn: async () => {
      const res = await api.get('/storage');
      return res.data;
    },
  });

  const { data: sshKeys, ...sshQuery } = useQuery<SSHKeyInfo[]>({
    queryKey: queryKeys.sshKeys.list(),
    queryFn: async () => (await storageApi.listSSHKeys()).data,
  });

  const createMutation = useMutation({
    mutationFn: () => api.post('/storage', { ...formData, port: formData.port ? parseInt(formData.port) : null }).then(r => r.data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.storages.all });
      resetForm();
    },
  });

  const updateMutation = useMutation({
    mutationFn: () => api.put(`/storage/${editingStorage!.id}`, { ...formData, port: formData.port ? parseInt(formData.port) : null }).then(r => r.data),
    onSuccess: () => {
      setTestResults({});
      queryClient.invalidateQueries({ queryKey: queryKeys.storages.all });
      resetForm();
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/storage/${id}`),
    onSuccess: () => {
      setConfirmDialog(null);
      queryClient.invalidateQueries({ queryKey: queryKeys.storages.all });
    },
  });

  const validation = useValidationFeedback(createMutation.error ?? updateMutation.error)
  const testMutation = useMutation({
    mutationFn: async (id: number) => {
      setTestingId(id);
      const revision = queryClient.getQueryData<RemoteStorage[]>(queryKeys.storages.list())?.find(storage => storage.id === id)?.updated_at
      const res = await api.post(`/storage/${id}/test`);
      return { id, revision, outcome: operationOutcome(res.data) };
    },
    onSuccess: (data) => {
      setTestingId(null);
      if (queryClient.getQueryData<RemoteStorage[]>(queryKeys.storages.list())?.find(storage => storage.id === data.id)?.updated_at !== data.revision) return
      saveTestResult(data.id, { success: data.outcome === 'completed', outcome: data.outcome, message: data.outcome === 'completed' ? 'Connection confirmed in this UI session' : data.outcome === 'partial' ? 'Partial result; test reported warnings' : data.outcome === 'failed' ? 'Connection test failed' : 'Connection result unknown; retry the test' }, data.revision);
    },
    onError: (_error: Error, id) => {
      saveTestResult(id, { success: false, outcome: 'unknown', message: 'Test response unavailable; no connection result confirmed' });
      setTestingId(null);
    },
  });

  const resetForm = () => {
    setFormData({
      name: '',
      storage_type: 'local',
      enabled: true,
      host: '',
      port: '',
      username: '',
      password: '',
      base_path: '/backups',
      ssh_key_path: '',
      s3_bucket: '',
      s3_region: 'eu-central-1',
      s3_access_key: '',
      s3_secret_key: '',
      s3_endpoint_url: '',
      webdav_url: '',
      rclone_remote: '',
    });
    setShowForm(false);
    setEditingStorage(null);
    setFormStep(1);
  };

  const handleEdit = (storage: RemoteStorage) => {
    setEditingStorage(storage);
    setFormData({
      name: storage.name,
      storage_type: storage.storage_type,
      enabled: storage.enabled,
      host: storage.host || '',
      port: storage.port?.toString() || '',
      username: storage.username || '',
      password: '',
      base_path: storage.base_path || '/backups',
      ssh_key_path: storage.ssh_key_path || '',
      s3_bucket: storage.s3_bucket || '',
      s3_region: storage.s3_region || 'eu-central-1',
      s3_access_key: '',
      s3_secret_key: '',
      s3_endpoint_url: storage.s3_endpoint_url || '',
      webdav_url: storage.webdav_url || '',
      rclone_remote: storage.rclone_remote || '',
    });
    setShowForm(true);
    setFormStep(2); // When editing, start at connection details
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (formStep !== 3 || createMutation.isPending || updateMutation.isPending) return;
    if (editingStorage) {
      updateMutation.mutate();
    } else {
      createMutation.mutate();
    }
  };

  const renderTypeFields = () => {
    switch (formData.storage_type) {
      case 'local':
        return (
          <div>
            <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
              Local Path
            </label>
            <input
              type="text"
              value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
              onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
              placeholder="/mnt/backup-storage"
              className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
            />
          </div>
        );

      case 'ssh':
        return (
          <>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Host
                </label>
                <input
                  type="text"
                  value={formData.host} aria-label="Host" {...validation.field('host')}
                  onChange={(e) => setFormData({ ...formData, host: e.target.value })}
                  placeholder="backup-server.local"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Port
                </label>
                <input
                  type="number"
                  value={formData.port} aria-label="Port" {...validation.field('port')}
                  onChange={(e) => setFormData({ ...formData, port: e.target.value })}
                  placeholder="22"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Username
                </label>
                <input
                  type="text"
                  value={formData.username} aria-label="Username" {...validation.field('username')}
                  onChange={(e) => setFormData({ ...formData, username: e.target.value })}
                  placeholder="backup"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Password / SSH-Key Path
                </label>
                <input
                  type="password"
                  value={formData.password} aria-label="Password" {...validation.field('password')}
                  onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                  placeholder="••••••••"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                SSH Key (optional)
              </label>
              {(() => {
                const keyPaths = (sshKeys || []).map((k) => k.private_path);
                const isManagedKey =
                  formData.ssh_key_path !== '' && keyPaths.includes(formData.ssh_key_path);
                const isCustom = formData.ssh_key_path !== '' && !isManagedKey;
                const selectValue = isCustom
                  ? '__custom__'
                  : isManagedKey
                  ? formData.ssh_key_path
                  : '';
                return (
                  <>
                    <select
                      value={selectValue}
                      onChange={(e) => {
                        const v = e.target.value;
                        if (v === '__custom__') {
                          // keep current text if any, otherwise blank to start fresh
                          setFormData({
                            ...formData,
                            ssh_key_path: isCustom ? formData.ssh_key_path : '',
                          });
                        } else {
                          setFormData({ ...formData, ssh_key_path: v });
                        }
                      }}
                      className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                    >
                      <option value="">— None (use password) —</option>
                      {(sshKeys || []).map((k) => (
                        <option key={k.name} value={k.private_path}>
                          {k.name}
                          {k.fingerprint ? ` — ${k.fingerprint.split(' ')[1] || ''}` : ''}
                        </option>
                      ))}
                      <option value="__custom__">Custom path…</option>
                    </select>
                    {isCustom && (
                      <input
                        type="text"
                        value={formData.ssh_key_path} aria-label="SSH key path" {...validation.field('ssh_key_path')}
                        onChange={(e) =>
                          setFormData({ ...formData, ssh_key_path: e.target.value })
                        }
                        placeholder="/app/data/ssh_keys/hetzner_box"
                        className="mt-2 w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                      />
                    )}
                    {(sshKeys || []).length === 0 && (
                      <p className="text-xs text-slate-500 dark:text-slate-400 mt-1">
                        Tip: generate a key in the “SSH Keys” panel above and it will appear here.
                      </p>
                    )}
                  </>
                );
              })()}
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Remote Path
              </label>
              <input
                type="text"
                value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
                onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
                placeholder="/home/backup/docker-backups"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
          </>
        );

      case 's3':
        return (
          <>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Bucket
                </label>
                <input
                  type="text"
                  value={formData.s3_bucket} aria-label="S3 bucket" {...validation.field('s3_bucket')}
                  onChange={(e) => setFormData({ ...formData, s3_bucket: e.target.value })}
                  placeholder="my-backups"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Region
                </label>
                <input
                  type="text"
                  value={formData.s3_region} aria-label="S3 region" {...validation.field('s3_region')}
                  onChange={(e) => setFormData({ ...formData, s3_region: e.target.value })}
                  placeholder="eu-central-1"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Access Key
                </label>
                <input
                  type="text"
                  value={formData.s3_access_key} aria-label="S3 access key" {...validation.field('s3_access_key')}
                  onChange={(e) => setFormData({ ...formData, s3_access_key: e.target.value })}
                  placeholder="AKIAIOSFODNN7EXAMPLE"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Secret Key
                </label>
                <input
                  type="password"
                  value={formData.s3_secret_key} aria-label="S3 secret key" {...validation.field('s3_secret_key')}
                  onChange={(e) => setFormData({ ...formData, s3_secret_key: e.target.value })}
                  placeholder="••••••••"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Endpoint URL (for MinIO/Backblaze, optional)
              </label>
              <input
                type="text"
                value={formData.s3_endpoint_url} aria-label="S3 endpoint URL" {...validation.field('s3_endpoint_url')}
                onChange={(e) => setFormData({ ...formData, s3_endpoint_url: e.target.value })}
                placeholder="https://s3.eu-central-003.backblazeb2.com"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Path Prefix
              </label>
              <input
                type="text"
                value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
                onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
                placeholder="docker-backups/"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
          </>
        );

      case 'webdav':
        return (
          <>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                WebDAV URL
              </label>
              <input
                type="text"
                value={formData.webdav_url} aria-label="WebDAV URL" {...validation.field('webdav_url')}
                onChange={(e) => setFormData({ ...formData, webdav_url: e.target.value })}
                placeholder="https://cloud.example.com/remote.php/dav/files/user/"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Username
                </label>
                <input
                  type="text"
                  value={formData.username} aria-label="Username" {...validation.field('username')}
                  onChange={(e) => setFormData({ ...formData, username: e.target.value })}
                  placeholder="admin"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Password / App Password
                </label>
                <input
                  type="password"
                  value={formData.password} aria-label="Password" {...validation.field('password')}
                  onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                  placeholder="••••••••"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Backup Folder
              </label>
              <input
                type="text"
                value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
                onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
                placeholder="Backups/Docker"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
          </>
        );

      case 'ftp':
        return (
          <>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Host
                </label>
                <input
                  type="text"
                  value={formData.host} aria-label="Host" {...validation.field('host')}
                  onChange={(e) => setFormData({ ...formData, host: e.target.value })}
                  placeholder="ftp.example.com"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Port
                </label>
                <input
                  type="number"
                  value={formData.port} aria-label="Port" {...validation.field('port')}
                  onChange={(e) => setFormData({ ...formData, port: e.target.value })}
                  placeholder="21"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Username
                </label>
                <input
                  type="text"
                  value={formData.username} aria-label="Username" {...validation.field('username')}
                  onChange={(e) => setFormData({ ...formData, username: e.target.value })}
                  placeholder="backup"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                  Password
                </label>
                <input
                  type="password"
                  value={formData.password} aria-label="Password" {...validation.field('password')}
                  onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                  placeholder="••••••••"
                  className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                />
              </div>
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Path
              </label>
              <input
                type="text"
                value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
                onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
                placeholder="/backups/docker"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
          </>
        );

      case 'rclone':
        return (
          <>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Rclone Remote Name
              </label>
              <input
                type="text"
                value={formData.rclone_remote} aria-label="Rclone remote" {...validation.field('rclone_remote')}
                onChange={(e) => setFormData({ ...formData, rclone_remote: e.target.value })}
                placeholder="gdrive"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
              <p className="text-xs text-slate-500 mt-1">
                The remote must be created first with `rclone config`
              </p>
            </div>
            <div>
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                Path
              </label>
              <input
                type="text"
                value={formData.base_path} aria-label="Base path" {...validation.field('base_path')}
                onChange={(e) => setFormData({ ...formData, base_path: e.target.value })}
                placeholder="backups/docker"
                className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
              />
            </div>
          </>
        );

      default:
        return null;
    }
  };

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex justify-between items-center">
        <div>
          <h1 className="text-2xl font-bold text-slate-900 dark:text-white">Storage</h1>
          <p className="text-slate-600 dark:text-slate-400">
            Browse local backups and configure external storage locations
          </p>
        </div>
        <button
          onClick={() => setShowForm(true)}
          className="flex items-center gap-2 px-4 py-2 bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 transition-colors"
        >
          <Plus className="w-5 h-5" />
          Add Storage
        </button>
      </div>

      {/* Storage Type Overview */}
      <NotificationContext domain="storage" />
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-4">
        {STORAGE_TYPES.map(({ value, label, icon: Icon, description }) => {
          const count = storages?.filter((s) => s.storage_type === value).length ?? 'Unknown';
          return (
            <div
              key={value}
              className="bg-white dark:bg-slate-800 rounded-lg p-4 border border-slate-200 dark:border-slate-700"
            >
              <div className="flex items-center gap-3 mb-2">
                <Icon className="w-5 h-5 text-indigo-600 dark:text-indigo-400" />
                <span className="font-medium text-slate-900 dark:text-white">{label}</span>
              </div>
              <p className="text-xs text-slate-500 dark:text-slate-400 mb-2">{description}</p>
              <span className="text-2xl font-bold text-slate-900 dark:text-white">{count}</span>
            </div>
          );
        })}
      </div>

      {/* Local Backups */}
      <div className="bg-white dark:bg-slate-800 rounded-lg border border-slate-200 dark:border-slate-700 px-6 py-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <div className="p-3 rounded-lg bg-green-100 dark:bg-green-900/30">
              <HardDrive className="w-6 h-6 text-green-600 dark:text-green-400" />
            </div>
            <div>
              <h2 className="font-medium text-slate-900 dark:text-white">
                Local Backups
              </h2>
              <p className="text-sm text-slate-500 dark:text-slate-400">
                Browse the local backup directory
              </p>
            </div>
          </div>
          <button
            onClick={() => setLocalBrowserOpen(true)}
            className="p-2 text-slate-600 dark:text-slate-400 hover:text-indigo-600 dark:hover:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-900/20 rounded-lg transition-colors"
            title="Browse Files"
          >
            <FolderOpen className="w-5 h-5" />
          </button>
        </div>
      </div>

      {/* SSH Keys */}
      <SSHKeyManager />

      {/* Storage List */}
      <div className="bg-white dark:bg-slate-800 rounded-lg border border-slate-200 dark:border-slate-700">
        <div className="px-6 py-4 border-b border-slate-200 dark:border-slate-700">
          <h2 className="text-lg font-semibold text-slate-900 dark:text-white">Configured Storage Locations</h2>
        </div>

        <QueryFeedback name="Storage locations" query={storagesQuery} />
        <QueryFeedback name="SSH keys" query={sshQuery} />
        {isLoading ? (
          <div className="p-6 space-y-3">
            {[...Array(2)].map((_, i) => (
              <div key={i} className="flex items-center gap-4 animate-pulse">
                <div className="w-12 h-12 bg-slate-200 dark:bg-slate-700 rounded-lg" />
                <div className="flex-1">
                  <div className="h-5 bg-slate-200 dark:bg-slate-700 rounded w-1/3 mb-2" />
                  <div className="h-4 bg-slate-200 dark:bg-slate-700 rounded w-1/2" />
                </div>
              </div>
            ))}
          </div>
        ) : storages && storages.length > 0 ? (
          <div className="divide-y divide-slate-200 dark:divide-slate-700">
            {storages.map((storage) => {
              const Icon = getStorageIcon(storage.storage_type);
              const typeInfo = STORAGE_TYPES.find((t) => t.value === storage.storage_type);
              return (
                <div key={storage.id} className="px-6 py-4 flex items-center justify-between">
                  <div className="flex items-center gap-4">
                    <div
                      className={`p-3 rounded-lg ${
                        storage.enabled
                          ? 'bg-indigo-100 dark:bg-indigo-900/30'
                          : 'bg-slate-100 dark:bg-slate-700'
                      }`}
                    >
                      <Icon
                        className={`w-6 h-6 ${
                          storage.enabled
                            ? 'text-indigo-600 dark:text-indigo-400'
                            : 'text-slate-400'
                        }`}
                      />
                    </div>
                    <div>
                      <div className="flex items-center gap-2">
                        <h2 className="font-medium text-slate-900 dark:text-white">{storage.name}</h2>
                        {!storage.enabled && (
                          <span className="px-2 py-0.5 text-xs rounded-full bg-slate-100 dark:bg-slate-700 text-slate-600 dark:text-slate-400">
                            Disabled
                          </span>
                        )}
                      </div>
                      <div className="flex items-center gap-2 text-sm text-slate-500 dark:text-slate-400">
                        <span className="px-2 py-0.5 bg-slate-100 dark:bg-slate-700 text-slate-700 dark:text-slate-200 rounded text-xs">
                          {typeInfo?.label || storage.storage_type}
                        </span>
                        {storage.storage_type === 'ssh' && storage.host && (
                          <span>{storage.username}@{storage.host}</span>
                        )}
                        {storage.storage_type === 's3' && storage.s3_bucket && (
                          <span>s3://{storage.s3_bucket}</span>
                        )}
                        {storage.storage_type === 'webdav' && storage.webdav_url && (
                          <span>{new URL(storage.webdav_url).hostname}</span>
                        )}
                        {storage.storage_type === 'rclone' && storage.rclone_remote && (
                          <span>{storage.rclone_remote}:{storage.base_path}</span>
                        )}
                        {(storage.storage_type === 'local' || storage.storage_type === 'ftp') && (
                          <span>{storage.base_path}</span>
                        )}
                      </div>
                    </div>
                  </div>
                  <div className="flex flex-col items-end gap-1">
                    <div className="flex items-center gap-2">
                      {testResults[storage.id] && testResults[storage.id].revision === storage.updated_at && (
                        <span
                          className={`flex items-center gap-1 text-sm ${
                            testResults[storage.id].success ? 'text-green-600' : 'text-red-600'
                          }`}
                          title={`${testResults[storage.id].message} — ${new Date(testResults[storage.id].testedAt).toLocaleString()}`}
                        >
                          {testResults[storage.id].success ? (
                            <CheckCircle className="w-4 h-4" />
                          ) : (
                            <XCircle className="w-4 h-4" />
                          )}
                          {testResults[storage.id].success ? 'Connected' : testResults[storage.id].outcome === 'partial' ? 'Partial' : testResults[storage.id].outcome === 'failed' ? 'Failed' : 'Unknown'}
                        </span>
                      )}
                      <button
                        onClick={() => setBrowsingStorage(storage)}
                        className="p-2 text-slate-600 dark:text-slate-400 hover:text-indigo-600 dark:hover:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-900/20 rounded-lg transition-colors"
                        title="Browse Files"
                      >
                        <FolderOpen className="w-5 h-5" />
                      </button>
                      <button
                        onClick={() => testMutation.mutate(storage.id)}
                        disabled={testingId === storage.id}
                        className="p-2 text-slate-600 dark:text-slate-400 hover:text-indigo-600 dark:hover:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-900/20 rounded-lg transition-colors"
                        title="Test Connection"
                      >
                        {testingId === storage.id ? (
                          <Loader2 className="w-5 h-5 animate-spin" />
                        ) : (
                          <TestTube className="w-5 h-5" />
                        )}
                      </button>
                      <button
                        onClick={() => handleEdit(storage)}
                        className="p-2 text-slate-600 dark:text-slate-400 hover:text-indigo-600 dark:hover:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-900/20 rounded-lg transition-colors"
                        title="Edit"
                      >
                        <Edit className="w-5 h-5" />
                      </button>
                      <button
                        onClick={() => setConfirmDialog({ id: storage.id, name: storage.name })}
                        className="p-2 text-slate-600 dark:text-slate-400 hover:text-red-600 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20 rounded-lg transition-colors"
                        title="Delete"
                      >
                        <Trash2 className="w-5 h-5" />
                      </button>
                    </div>
                    {testResults[storage.id]?.message && testResults[storage.id].revision === storage.updated_at && (
                      <p
                        className={`max-w-xs text-right text-xs ${
                          testResults[storage.id].success ? 'text-green-600' : 'text-red-600'
                        }`}
                      >
                        {testResults[storage.id].message}
                      </p>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        ) : storages && !storagesQuery.isError ? (
          <div className="p-8 text-center text-slate-500 dark:text-slate-400">
            <Wifi className="w-12 h-12 mx-auto mb-4 opacity-50" />
            <p className="text-lg font-medium">No remote storages configured</p>
            <p className="text-sm">Add a storage location to backup off-site</p>
          </div>
        ) : null}
      </div>

      {/* Storage Browser Modal */}
      {browsingStorage && (
        <StorageBrowser
          storage={browsingStorage}
          onClose={() => setBrowsingStorage(null)}
        />
      )}

      {/* Local Backup Browser Modal */}
      {localBrowserOpen && (
        <StorageBrowser
          storage={{
            id: 'local' as const,
            name: 'Local Backups',
            storage_type: 'local',
            base_path: '/backups',
          }}
          onClose={() => setLocalBrowserOpen(false)}
        />
      )}

      {/* Add/Edit Form Modal - ST1: Step-based */}
      {showForm && (
        <Modal title={editingStorage ? 'Edit Storage' : 'Add New Storage'} onClose={() => setShowForm(false)} pending={createMutation.isPending || updateMutation.isPending}>
          <div className="bg-white dark:bg-slate-800 rounded-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto">
            {/* Header with step indicator */}
            <div className="px-6 py-4 border-b border-slate-200 dark:border-slate-700">
              <h2 className="text-lg font-semibold text-slate-900 dark:text-white">
                {editingStorage ? 'Edit Storage' : 'Add New Storage'}
              </h2>
              <div className="flex flex-wrap items-center gap-2 mt-2">
                {['Name & Type', 'Connection', 'Review'].map((label, i) => {
                  const step = i + 1;
                  return (
                    <div key={step} className="flex items-center gap-2">
                      <div className={`flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium transition-colors ${
                        step === formStep
                          ? 'bg-indigo-500 text-white'
                          : step < formStep
                          ? 'bg-green-500/20 text-green-400'
                          : 'bg-slate-200 dark:bg-slate-700 text-slate-700 dark:text-slate-200'
                      }`}>
                        <span>{step}</span>
                        <span>{label}</span>
                      </div>
                      {i < 2 && <div className={`w-4 h-0.5 ${step < formStep ? 'bg-green-500' : 'bg-slate-300 dark:bg-slate-600'}`} />}
                    </div>
                  );
                })}
              </div>
            </div>

            <form onSubmit={handleSubmit} className="p-6 space-y-4">
              {validation.summary}
              {/* Step 1: Name & Type */}
              {formStep === 1 && (
                <>
                  <div>
                    <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-1">
                      Name
                    </label>
                    <input
                      type="text"
                      value={formData.name} aria-label="Name" {...validation.field('name')}
                      onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                      placeholder="My Backup Storage"
                      className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded-lg bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium text-slate-700 dark:text-slate-300 mb-2">
                      Storage Type
                    </label>
                    <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
                      {STORAGE_TYPES.map(({ value, label, icon: Icon, description }) => (
                        <button
                          key={value}
                          type="button"
                          onClick={() => setFormData({ ...formData, storage_type: value })}
                          className={`flex flex-col items-start gap-1 px-4 py-3 rounded-lg border-2 transition-colors text-left ${
                            formData.storage_type === value
                              ? 'border-indigo-500 bg-indigo-50 dark:bg-indigo-900/30'
                              : 'border-slate-200 dark:border-slate-600 hover:border-slate-400'
                          }`}
                        >
                          <div className="flex items-center gap-2">
                            <Icon className={`w-5 h-5 ${formData.storage_type === value ? 'text-indigo-600 dark:text-indigo-400' : 'text-slate-500'}`} />
                            <span className={`text-sm font-medium ${formData.storage_type === value ? 'text-indigo-700 dark:text-indigo-300' : 'text-slate-700 dark:text-slate-300'}`}>{label}</span>
                          </div>
                          <span className="text-xs text-slate-500 dark:text-slate-400">{description}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                </>
              )}

              {/* Step 2: Connection Details */}
              {formStep === 2 && (
                <>
                  <div className="flex items-center gap-2 mb-2 text-sm text-slate-500 dark:text-slate-400">
                    {(() => { const Icon = getStorageIcon(formData.storage_type); return <Icon className="w-4 h-4" />; })()}
                    <span>{STORAGE_TYPES.find(t => t.value === formData.storage_type)?.label} — {formData.name || 'Unnamed'}</span>
                  </div>
                  {renderTypeFields()}
                </>
              )}

              {/* Step 3: Review & Confirm */}
              {formStep === 3 && (
                <>
                  <div className="space-y-3 text-sm">
                    <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                      <span className="text-slate-500">Name</span>
                      <span className="font-medium text-slate-900 dark:text-white">{formData.name || '—'}</span>
                    </div>
                    <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                      <span className="text-slate-500">Type</span>
                      <span className="font-medium text-slate-900 dark:text-white">{STORAGE_TYPES.find(t => t.value === formData.storage_type)?.label}</span>
                    </div>
                    {formData.storage_type === 'ssh' && formData.host && (
                      <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                        <span className="text-slate-500">Host</span>
                        <span className="font-mono text-slate-900 dark:text-white">{formData.username ? `${formData.username}@` : ''}{formData.host}{formData.port ? `:${formData.port}` : ''}</span>
                      </div>
                    )}
                    {formData.storage_type === 's3' && formData.s3_bucket && (
                      <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                        <span className="text-slate-500">Bucket</span>
                        <span className="font-mono text-slate-900 dark:text-white">s3://{formData.s3_bucket}/{formData.base_path}</span>
                      </div>
                    )}
                    {formData.storage_type === 'webdav' && formData.webdav_url && (
                      <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                        <span className="text-slate-500">WebDAV</span>
                        <span className="font-mono text-slate-900 dark:text-white text-right break-all">{formData.webdav_url}</span>
                      </div>
                    )}
                    {formData.storage_type === 'rclone' && formData.rclone_remote && (
                      <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                        <span className="text-slate-500">Remote</span>
                        <span className="font-mono text-slate-900 dark:text-white">{formData.rclone_remote}:{formData.base_path}</span>
                      </div>
                    )}
                    {(formData.storage_type === 'local' || formData.storage_type === 'ftp') && (
                      <div className="flex justify-between py-2 border-b border-slate-200 dark:border-slate-700">
                        <span className="text-slate-500">{formData.storage_type === 'ftp' ? 'Host' : 'Path'}</span>
                        <span className="font-mono text-slate-900 dark:text-white">
                          {formData.storage_type === 'ftp' && formData.host ? `${formData.host}:${formData.base_path}` : formData.base_path}
                        </span>
                      </div>
                    )}
                  </div>

                  <div className="flex items-center gap-2 pt-2">
                    <input
                      type="checkbox"
                      id="enabled"
                      checked={formData.enabled}
                      onChange={(e) => setFormData({ ...formData, enabled: e.target.checked })}
                      className="w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500"
                    />
                    <label htmlFor="enabled" className="text-sm text-slate-700 dark:text-slate-300">
                      Storage enabled
                    </label>
                  </div>
                </>
              )}

              {/* Navigation */}
              <div className="flex justify-between pt-4 border-t border-slate-200 dark:border-slate-700">
                <div>
                  {formStep > 1 && (
                    <button
                      type="button"
                      onClick={() => setFormStep(formStep - 1)}
                      disabled={createMutation.isPending || updateMutation.isPending}
                      className="flex items-center gap-1 px-4 py-2 text-slate-700 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-700 rounded-lg transition-colors"
                    >
                      <ChevronLeft className="w-4 h-4" />
                      Back
                    </button>
                  )}
                </div>
                <div className="flex gap-3">
                  <button
                    type="button"
                    onClick={resetForm}
                    disabled={createMutation.isPending || updateMutation.isPending}
                    className="px-4 py-2 text-slate-700 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-700 rounded-lg transition-colors"
                  >
                    Cancel
                  </button>
                  {formStep < 3 ? (
                    <button
                      key="next"
                      type="button"
                      onClick={() => setFormStep(formStep + 1)}
                      disabled={formStep === 1 && !formData.name}
                      className="flex items-center gap-1 px-4 py-2 bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 transition-colors disabled:opacity-50"
                    >
                      Next
                      <ChevronRight className="w-4 h-4" />
                    </button>
                  ) : (
                    <button
                      key="confirm"
                      type="submit"
                      disabled={createMutation.isPending || updateMutation.isPending}
                      className="px-4 py-2 bg-indigo-600 text-white rounded-lg hover:bg-indigo-700 transition-colors disabled:opacity-50"
                    >
                      {createMutation.isPending || updateMutation.isPending ? (
                        <Loader2 className="w-5 h-5 animate-spin" />
                      ) : editingStorage ? (
                        'Save'
                      ) : (
                        'Add Storage'
                      )}
                    </button>
                  )}
                </div>
              </div>
            </form>
          </div>
        </Modal>
      )}

      {/* Confirm Delete Dialog */}
      <ConfirmDialog
        isOpen={!!confirmDialog}
        onClose={() => { setConfirmDialog(null); deleteMutation.reset(); }}
        onConfirm={() => {
          if (confirmDialog) {
            deleteMutation.mutate(confirmDialog.id);
          }
        }}
        title="Delete Storage"
        message={`Really delete storage "${confirmDialog?.name}"? This action cannot be undone.`}
        confirmLabel="Delete"
        confirmVariant="danger"
        isLoading={deleteMutation.isPending}
      >
        {deleteMutation.isError && <p role="alert" className="text-red-300">Failed to delete storage. Retry or cancel.</p>}
      </ConfirmDialog>
    </div>
  );
}
