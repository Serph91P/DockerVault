import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Key, Plus, Trash2, Upload, Copy, Check, Loader2, Download } from 'lucide-react';
import axios from 'axios';
import { storageApi, type SSHKeyInfo } from '../api';

/**
 * Manages SSH keypairs stored under /app/data/ssh_keys.
 *
 * - Generates ed25519 keypairs
 * - Lets the user copy/download the public key
 * - Optionally installs the public key on a remote (Hetzner Storage Box's
 *   `install-ssh-key` helper or generic ~/.ssh/authorized_keys append)
 */
export default function SSHKeyManager() {
  const queryClient = useQueryClient();
  const [showCreate, setShowCreate] = useState(false);
  const [createName, setCreateName] = useState('');
  const [createComment, setCreateComment] = useState('');
  const [installFor, setInstallFor] = useState<SSHKeyInfo | null>(null);
  const [copiedName, setCopiedName] = useState<string | null>(null);

  const { data: keys, isLoading } = useQuery({
    queryKey: ['ssh-keys'],
    queryFn: async () => (await storageApi.listSSHKeys()).data,
  });

  const createMut = useMutation({
    mutationFn: () =>
      storageApi.generateSSHKey({
        name: createName.trim(),
        comment: createComment.trim() || undefined,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['ssh-keys'] });
      setShowCreate(false);
      setCreateName('');
      setCreateComment('');
    },
  });

  const deleteMut = useMutation({
    mutationFn: (name: string) => storageApi.deleteSSHKey(name),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['ssh-keys'] }),
  });

  const copy = async (key: SSHKeyInfo) => {
    try {
      await navigator.clipboard.writeText(key.public_key);
      setCopiedName(key.name);
      setTimeout(() => setCopiedName(null), 1500);
    } catch {
      // ignore — Safari without HTTPS will reject the writeText
    }
  };

  return (
    <div className="bg-white dark:bg-slate-900 rounded-lg shadow border border-slate-200 dark:border-slate-700">
      <div className="flex items-center justify-between p-4 border-b border-slate-200 dark:border-slate-700">
        <div className="flex items-center gap-2">
          <Key className="w-5 h-5 text-indigo-600 dark:text-indigo-400" />
          <h2 className="font-semibold text-slate-900 dark:text-white">SSH Keys</h2>
        </div>
        <button
          onClick={() => setShowCreate(true)}
          className="flex items-center gap-1 px-3 py-1.5 text-sm bg-indigo-600 text-white rounded hover:bg-indigo-700"
        >
          <Plus className="w-4 h-4" />
          Generate
        </button>
      </div>

      <div className="p-4 space-y-3">
        {isLoading && (
          <div className="flex items-center gap-2 text-slate-500 text-sm">
            <Loader2 className="w-4 h-4 animate-spin" /> Loading…
          </div>
        )}
        {!isLoading && (!keys || keys.length === 0) && (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            No SSH keys yet. Generate one and use the public key on your remote
            (e.g. Hetzner Storage Box).
          </p>
        )}
        {keys?.map((key) => (
          <div
            key={key.name}
            className="border border-slate-200 dark:border-slate-700 rounded-md p-3 space-y-2"
          >
            <div className="flex items-center justify-between gap-2">
              <div>
                <div className="font-medium text-slate-900 dark:text-white">{key.name}</div>
                {key.fingerprint && (
                  <div className="text-xs text-slate-500 font-mono">{key.fingerprint}</div>
                )}
                <div className="text-xs text-slate-400 mt-1 font-mono">
                  Path: {key.private_path}
                </div>
              </div>
              <div className="flex items-center gap-1">
                <button
                  onClick={() => copy(key)}
                  className="p-2 hover:bg-slate-100 dark:hover:bg-slate-800 rounded"
                  title="Copy public key"
                >
                  {copiedName === key.name ? (
                    <Check className="w-4 h-4 text-green-600" />
                  ) : (
                    <Copy className="w-4 h-4 text-slate-600 dark:text-slate-400" />
                  )}
                </button>
                <a
                  href={storageApi.downloadSSHPublicKeyURL(key.name)}
                  className="p-2 hover:bg-slate-100 dark:hover:bg-slate-800 rounded"
                  title="Download .pub"
                >
                  <Download className="w-4 h-4 text-slate-600 dark:text-slate-400" />
                </a>
                <button
                  onClick={() => setInstallFor(key)}
                  className="p-2 hover:bg-indigo-100 dark:hover:bg-indigo-900 rounded"
                  title="Install on remote"
                >
                  <Upload className="w-4 h-4 text-indigo-600 dark:text-indigo-400" />
                </button>
                <button
                  onClick={() => {
                    if (confirm(`Delete SSH key "${key.name}"? This cannot be undone.`)) {
                      deleteMut.mutate(key.name);
                    }
                  }}
                  className="p-2 hover:bg-red-100 dark:hover:bg-red-900 rounded"
                  title="Delete key"
                >
                  <Trash2 className="w-4 h-4 text-red-600" />
                </button>
              </div>
            </div>
            <pre className="text-xs bg-slate-50 dark:bg-slate-800 p-2 rounded font-mono overflow-x-auto">
              {key.public_key}
            </pre>
          </div>
        ))}
      </div>

      {showCreate && (
        <CreateKeyModal
          name={createName}
          comment={createComment}
          onName={setCreateName}
          onComment={setCreateComment}
          isPending={createMut.isPending}
          error={
            createMut.error
              ? axios.isAxiosError(createMut.error)
                ? createMut.error.response?.data?.detail || String(createMut.error)
                : String(createMut.error)
              : null
          }
          onCancel={() => {
            setShowCreate(false);
            createMut.reset();
          }}
          onSubmit={() => createMut.mutate()}
        />
      )}

      {installFor && (
        <InstallKeyModal
          keyInfo={installFor}
          onClose={() => setInstallFor(null)}
        />
      )}
    </div>
  );
}

function CreateKeyModal(props: {
  name: string;
  comment: string;
  onName: (s: string) => void;
  onComment: (s: string) => void;
  isPending: boolean;
  error: string | null;
  onCancel: () => void;
  onSubmit: () => void;
}) {
  const valid = /^[A-Za-z0-9_-]{1,40}$/.test(props.name);
  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
      <div className="bg-white dark:bg-slate-900 rounded-lg shadow-xl max-w-md w-full p-5 space-y-4">
        <h3 className="font-semibold text-slate-900 dark:text-white">Generate SSH key</h3>
        <p className="text-xs text-slate-500 dark:text-slate-400">
          A new ed25519 keypair will be stored under{' '}
          <code className="font-mono">/app/data/ssh_keys/&lt;name&gt;</code>. The
          private key never leaves the container.
        </p>
        <div>
          <label className="block text-sm text-slate-700 dark:text-slate-300 mb-1">Name</label>
          <input
            type="text"
            value={props.name}
            onChange={(e) => props.onName(e.target.value)}
            placeholder="hetzner_box"
            className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
          />
          <div className="text-xs text-slate-500 mt-1">
            Letters, digits, underscore, dash. Max 40 chars.
          </div>
        </div>
        <div>
          <label className="block text-sm text-slate-700 dark:text-slate-300 mb-1">
            Comment (optional)
          </label>
          <input
            type="text"
            value={props.comment}
            onChange={(e) => props.onComment(e.target.value)}
            placeholder="dockervault@home-server"
            className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800 text-slate-900 dark:text-white"
          />
        </div>
        {props.error && (
          <div className="text-sm text-red-600 dark:text-red-400">{props.error}</div>
        )}
        <div className="flex justify-end gap-2 pt-2">
          <button
            onClick={props.onCancel}
            className="px-4 py-2 text-slate-700 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-800 rounded"
          >
            Cancel
          </button>
          <button
            disabled={!valid || props.isPending}
            onClick={props.onSubmit}
            className="px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700 disabled:opacity-50"
          >
            {props.isPending && <Loader2 className="w-4 h-4 animate-spin inline mr-1" />}
            Generate
          </button>
        </div>
      </div>
    </div>
  );
}

function InstallKeyModal(props: { keyInfo: SSHKeyInfo; onClose: () => void }) {
  const [host, setHost] = useState('');
  const [port, setPort] = useState('23');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [method, setMethod] = useState<'auto' | 'hetzner' | 'authorized_keys'>('auto');

  const installMut = useMutation({
    mutationFn: () =>
      storageApi.installSSHKey(props.keyInfo.name, {
        host: host.trim(),
        port: Number(port) || 22,
        username: username.trim(),
        password,
        method,
      }),
  });

  const errorText = installMut.error
    ? axios.isAxiosError(installMut.error)
      ? installMut.error.response?.data?.detail || String(installMut.error)
      : String(installMut.error)
    : null;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
      <div className="bg-white dark:bg-slate-900 rounded-lg shadow-xl max-w-lg w-full p-5 space-y-4">
        <div>
          <h3 className="font-semibold text-slate-900 dark:text-white">
            Install key on remote
          </h3>
          <p className="text-xs text-slate-500 dark:text-slate-400 mt-1">
            Uses your password once to upload the public part of{' '}
            <code className="font-mono">{props.keyInfo.name}</code>. The
            password is never stored.
          </p>
        </div>
        <div className="grid grid-cols-3 gap-2">
          <div className="col-span-2">
            <label className="block text-sm mb-1">Host</label>
            <input
              type="text"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="u123456.your-storagebox.de"
              className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800"
            />
          </div>
          <div>
            <label className="block text-sm mb-1">Port</label>
            <input
              type="number"
              value={port}
              onChange={(e) => setPort(e.target.value)}
              className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800"
            />
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="block text-sm mb-1">Username</label>
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="u123456"
              className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800"
            />
          </div>
          <div>
            <label className="block text-sm mb-1">Password</label>
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800"
            />
          </div>
        </div>
        <div>
          <label className="block text-sm mb-1">Install method</label>
          <select
            value={method}
            onChange={(e) => setMethod(e.target.value as typeof method)}
            className="w-full px-3 py-2 border border-slate-300 dark:border-slate-600 rounded bg-white dark:bg-slate-800"
          >
            <option value="auto">Auto-detect</option>
            <option value="hetzner">Hetzner Storage Box (install-ssh-key, port 23)</option>
            <option value="authorized_keys">Generic (~/.ssh/authorized_keys)</option>
          </select>
        </div>

        {installMut.isSuccess && (
          <div className="text-sm text-green-700 dark:text-green-400 bg-green-50 dark:bg-green-900/30 p-2 rounded">
            Key installed successfully.
          </div>
        )}
        {errorText && (
          <pre className="text-xs text-red-700 dark:text-red-400 bg-red-50 dark:bg-red-900/30 p-2 rounded whitespace-pre-wrap">
            {errorText}
          </pre>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button
            onClick={props.onClose}
            className="px-4 py-2 text-slate-700 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-800 rounded"
          >
            Close
          </button>
          <button
            disabled={!host || !username || !password || installMut.isPending}
            onClick={() => installMut.mutate()}
            className="px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700 disabled:opacity-50"
          >
            {installMut.isPending && <Loader2 className="w-4 h-4 animate-spin inline mr-1" />}
            Install
          </button>
        </div>
      </div>
    </div>
  );
}
