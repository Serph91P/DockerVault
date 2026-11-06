type BackupParams = { target_id?: number; status?: string; limit?: number }
const family = <T extends string>(name: T) => ({ all: [name] as const, list: () => [name, 'list'] as const })

export const queryKeys = {
  backups: {
    all: ['backups'] as const,
    list: (params: BackupParams = {}) => ['backups', 'list', params] as const,
    detail: (id: number) => ['backups', 'detail', id] as const,
    logs: (id: number) => ['backups', 'logs', id] as const,
  },
  targets: {
    all: ['targets'] as const,
    list: (includeRemoved = false) => ['targets', 'list', { include_removed: includeRemoved }] as const,
    detail: (id: number) => ['targets', 'detail', id] as const,
  },
  storages: family('storages'),
  schedules: { ...family('schedules'), jobs: () => ['schedules', 'jobs'] as const, inventory: () => ['schedules', 'cron-inventory'] as const },
  retention: family('retention-policies'),
  containers: family('containers'),
  volumes: family('volumes'),
  stacks: family('stacks'),
  systemInfo: ['system-info'] as const,
  settings: { storage: ['settings', 'storage'] as const, komodo: ['settings', 'komodo'] as const },
  sshKeys: family('ssh-keys'),
  storageFiles: (id: number | 'local', path: string, root: string, owner: number) => ['storage-files', id, path, root, owner] as const,
  storageSize: (id: number | 'local', root: string, path: string, revision: number, owner: number) => ['storage-size', id, root, path, revision, owner] as const,
  storageOperations: (id: number | 'local', owner: number) => ['storage-operations', id, owner] as const,
  operationList: (owner: number) => ['storage-operations', 'list', { limit: 100 }, owner] as const,
  operationDetail: (id: string | null, storage: number | 'local', owner: number) => ['storage-operations', 'detail', id, storage, owner] as const,
}
