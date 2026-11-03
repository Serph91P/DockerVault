import { useEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ownsSession, sessionGeneration, sessionSignal } from '../store/session';
import {
  Shield,
  Key,
  Copy,
  Download,
  Check,
  AlertTriangle,
  Lock,
  Unlock,
  FileText,
  RefreshCw,
  X,
} from 'lucide-react';
import api from '../api';

interface EncryptionStatus {
  setup_completed: boolean;
  encryption_enabled: boolean;
  onboarding_deferred?: boolean;
  public_key?: string;
  key_created_at?: string;
}

interface SetupResponse {
  public_key: string;
  private_key: string;
  recovery_instructions: string;
}

export default function EncryptionSetup() {
  const queryClient = useQueryClient();
  const [showSetupModal, setShowSetupModal] = useState(false);
  const [setupData, setSetupData] = useState<SetupResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const [downloadedKey, setDownloadedKey] = useState(false);
  const [showInstructions, setShowInstructions] = useState(false);
  const [showRegenerateWarning, setShowRegenerateWarning] = useState(false);
  const custodyDialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (showSetupModal) custodyDialog.current?.querySelector('button')?.focus();
  }, [showSetupModal]);
  const closeCustody = () => {
    setSetupData(null);
    setShowSetupModal(false);
    setDownloadedKey(false);
    void refetch();
  };

  const [operation, setOperation] = useState<string | null>(null);
  const [operationError, setOperationError] = useState('');
  const busy = useRef(false);
  const alive = useRef(true);
  const requests = useRef(new AbortController());
  useEffect(() => {
    alive.current = true;
    requests.current = new AbortController();
    return () => { alive.current = false; requests.current.abort(); };
  }, []);

  const { data: status, isLoading, isError, refetch } = useQuery<EncryptionStatus>({
    queryKey: ['encryption-status'],
    queryFn: async ({ signal }) => (await api.get('/encryption/status', { signal })).data,
    retry: false,
  });

  // One-time responses never enter a Mutation cache (including Axios config/errors).
  const run = async (kind: string, path: string, body?: unknown) => {
    if (busy.current) return;
    if ((kind === 'setup' || kind === 'regenerate') && setupData) return;
    if (kind === 'setup' && status?.public_key) return;
    busy.current = true;
    const owner = sessionGeneration();
    setOperation(kind);
    setOperationError('');
    try {
      const res = await api.post(path, body, { signal: AbortSignal.any([requests.current.signal, sessionSignal()]) });
      if (!alive.current || !ownsSession(owner)) return;
      if (kind === 'setup' || kind === 'regenerate') {
        setSetupData(res.data as SetupResponse);
        setDownloadedKey(false);
        setShowSetupModal(true);
        setShowRegenerateWarning(false);
      } else {
        if (kind === 'confirm') {
          setSetupData(null);
          setShowSetupModal(false);
          setDownloadedKey(false);
        }
        await queryClient.invalidateQueries({ queryKey: ['encryption-status'] });
      }
    } catch {
      if (!alive.current || !ownsSession(owner)) return;
      setOperationError('Encryption request failed. Check the current status before retrying.');
      // A lost response may have committed a pending public identity. Never replay setup.
      if (kind === 'setup' || kind === 'regenerate') await refetch();
    } finally {
      busy.current = false;
      if (alive.current && ownsSession(owner)) setOperation(null);
    }
  };
  const setupMutation = { isPending: operation === 'setup', mutate: () => void run('setup', '/encryption/setup') };
  const regenerateMutation = { isPending: operation === 'regenerate', mutate: () => void run('regenerate', '/encryption/regenerate', { confirm_data_loss: true }) };
  const confirmMutation = { isPending: operation === 'confirm', mutate: () => void run('confirm', '/encryption/confirm-setup', { confirmed: true }) };
  const toggleMutation = { isPending: operation === 'toggle', mutate: (enabled: boolean) => void run('toggle', `/encryption/toggle?enabled=${enabled}`) };

  const copyToClipboard = async (text: string) => {
    const owner = sessionGeneration();
    try {
      await navigator.clipboard.writeText(text);
      if (alive.current && ownsSession(owner)) setCopied(true);
    } catch { if (alive.current && ownsSession(owner)) setOperationError('Clipboard unavailable. Save the key file instead.'); }
  };

  const downloadPrivateKey = () => {
    if (!setupData) return;
    
    const content = `# DockerVault Private Key
# Created: ${new Date().toISOString()}
# KEEP THIS FILE SECURE - DO NOT SHARE
#
# This key is required to decrypt your backups.
# Store it in a secure location (password manager, encrypted drive, etc.)

${setupData.private_key}

# Recovery Instructions:
${setupData.recovery_instructions.split('\n').map(line => `# ${line}`).join('\n')}
`;
    
    let url: string | undefined;
    try {
      url = URL.createObjectURL(new Blob([content], { type: 'text/plain' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `dockervault-private-key-${Date.now()}.txt`;
      a.click();
      setDownloadedKey(true);
    } catch {
      setDownloadedKey(false);
      setOperationError('Export did not start. Retry and verify the saved file before confirming custody.');
    } finally { if (url) URL.revokeObjectURL(url); }
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center p-8">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-indigo-600"></div>
      </div>
    );
  }

  if (isError || !status) return <div role="alert">Unable to load encryption status. <button onClick={() => void refetch()}>Retry</button></div>;

  return (
    <div className="bg-dark-800 rounded-xl border border-dark-700 p-6">
      {status.onboarding_deferred && !status.setup_completed && <p className="text-amber-400">Encryption deliberately deferred. New backups are unencrypted.</p>}
      {operationError && <p role="alert" className="text-red-400">{operationError}</p>}
      {/* Header */}
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-indigo-500/10 rounded-lg flex items-center justify-center">
            <Shield className="w-5 h-5 text-indigo-400" />
          </div>
          <div>
            <h2 className="text-lg font-semibold text-dark-100">
              Backup Encryption
            </h2>
            <p className="text-sm text-dark-400">
              Protect your backups with end-to-end encryption
            </p>
          </div>
        </div>
      </div>

      {/* Status Content */}
      <div>
        {!status.setup_completed && !status.public_key ? (
          // Not configured
          <div className="text-center py-8">
            <div className="mx-auto w-16 h-16 bg-amber-500/10 rounded-full flex items-center justify-center mb-4">
              <AlertTriangle className="w-8 h-8 text-amber-400" />
            </div>
            <h3 className="text-lg font-semibold text-dark-100 mb-2">
              Encryption Not Configured
            </h3>
            <p className="text-dark-400 mb-4 max-w-md mx-auto">
              Your backups are currently stored unencrypted. Set up encryption to protect
              your data with industry-standard AES-256 encryption.
            </p>
            <p className="text-xs text-amber-400/80 mb-6 max-w-sm mx-auto">
              ⚠ After setup, download and safely store your private key — without it, encrypted backups cannot be restored.
            </p>
            <button
              onClick={() => setupMutation.mutate()}
              disabled={setupMutation.isPending || setupData !== null}
              className="px-6 py-3 bg-indigo-600 hover:bg-indigo-700 text-white rounded-lg font-medium transition-colors"
            >
              {setupMutation.isPending ? 'Generating Keys...' : 'Set Up Encryption'}
            </button>
          </div>
        ) : (
          // Configured
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className={`p-2 rounded-lg ${
                  status.encryption_enabled
                    ? 'bg-green-500/10'
                    : 'bg-dark-700'
                }`}>
                  {status.encryption_enabled ? (
                    <Lock className="w-5 h-5 text-green-400" />
                  ) : (
                    <Unlock className="w-5 h-5 text-dark-400" />
                  )}
                </div>
                <div>
                  <p className="font-medium text-dark-100">
                    {!status.setup_completed ? 'Key custody pending' : status.encryption_enabled ? 'Encryption Enabled' : 'Encryption Disabled'}
                  </p>
                  <p className="text-sm text-dark-400">
                    {!status.setup_completed ? 'This key cannot be retrieved after reload. If it was lost, explicitly regenerate; keep old keys for old archives.' : status.encryption_enabled
                      ? 'New backups will be encrypted'
                      : 'New backups will NOT be encrypted'}
                  </p>
                </div>
              </div>
              {status.setup_completed && <button
                onClick={() => toggleMutation.mutate(!status.encryption_enabled)}
                disabled={toggleMutation.isPending}
                className={`px-4 py-2 rounded-lg font-medium transition-colors ${
                  status.encryption_enabled
                    ? 'bg-dark-700 text-dark-300 hover:bg-dark-600'
                    : 'bg-green-600 text-white hover:bg-green-700'
                }`}
              >
                {status.encryption_enabled ? 'Disable' : 'Enable'}
              </button>}
            </div>

            <div className="border-t border-dark-700 pt-4">
              <div className="flex items-center justify-between mb-2">
                <span className="text-sm font-medium text-dark-300">
                  Public Key
                </span>
                <button
                  onClick={() => copyToClipboard(status.public_key || '')}
                  className="text-sm text-indigo-400 hover:underline flex items-center gap-1"
                >
                  <Copy className="w-3 h-3" />
                  Copy
                </button>
              </div>
              <code className="block p-3 bg-dark-900 rounded-lg text-xs font-mono text-dark-300 break-all">
                {status.public_key}
              </code>
              {status.key_created_at && (
                <p className="text-xs text-dark-500 mt-2">
                  Created: {new Date(status.key_created_at).toLocaleString()}
                </p>
              )}
            </div>

            <div className="flex items-center gap-3 pt-2">
              <button
                onClick={() => setShowInstructions(true)}
                className="flex items-center gap-2 px-4 py-2 text-sm text-dark-300 bg-dark-700 hover:bg-dark-600 rounded-lg transition-colors"
              >
                <FileText className="w-4 h-4" />
                View Recovery Instructions
              </button>
              <button
                onClick={() => setShowRegenerateWarning(true)}
                className="flex items-center gap-2 px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 rounded-lg transition-colors"
              >
                <RefreshCw className="w-4 h-4" />
                Regenerate Keys
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Setup Modal */}
      {showSetupModal && setupData && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div ref={custodyDialog} role="dialog" aria-modal="true" aria-labelledby="custody-title" className="bg-dark-800 border border-dark-700 rounded-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto" onKeyDown={event => {
            if (event.key === 'Escape') { event.preventDefault(); closeCustody(); }
            if (event.key !== 'Tab') return;
            const controls = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
            const first = controls[0], last = controls[controls.length - 1];
            if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
            if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
          }}>
            <div className="px-6 py-4 border-b border-dark-700">
              <div className="flex items-center gap-3">
                <div className="p-2 bg-indigo-500/10 rounded-lg">
                  <Key className="w-5 h-5 text-indigo-400" />
                </div>
                <h2 id="custody-title" className="text-lg font-semibold text-dark-100">
                  Save Your Private Key
                </h2>
              </div>
            </div>

            <div className="p-6 space-y-6">
              <div className="bg-amber-500/10 border border-amber-500/30 rounded-lg p-4">
                <div className="flex gap-3">
                  <AlertTriangle className="w-5 h-5 text-amber-400 flex-shrink-0 mt-0.5" />
                  <div>
                    <p className="font-medium text-amber-200">
                      Important: Save this key NOW
                    </p>
                    <p className="text-sm text-amber-300/80 mt-1">
                      This private key will NOT be shown again. Without it, your encrypted
                      backups cannot be recovered. Store it in a secure location.
                    </p>
                  </div>
                </div>
              </div>

              <div>
                <div className="flex items-center justify-between mb-2">
                  <label className="text-sm font-medium text-dark-300">
                    Private Key
                  </label>
                  <div className="flex items-center gap-2">
                    <button
                      onClick={() => copyToClipboard(setupData.private_key)}
                      className="text-sm text-indigo-400 hover:underline flex items-center gap-1"
                    >
                      {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
                      {copied ? 'Copied!' : 'Copy'}
                    </button>
                  </div>
                </div>
                <code className="block p-3 bg-dark-900 rounded-lg text-xs font-mono text-dark-300 break-all select-all">
                  {setupData.private_key}
                </code>
              </div>

              <button
                onClick={downloadPrivateKey}
                className={`w-full flex items-center justify-center gap-2 px-4 py-3 rounded-lg font-medium transition-colors ${
                  downloadedKey
                    ? 'bg-green-500/10 text-green-400'
                    : 'bg-indigo-600 hover:bg-indigo-700 text-white'
                }`}
              >
                {downloadedKey ? (
                  <>
                    <Check className="w-5 h-5" />
                    Export started — verify your saved file
                  </>
                ) : (
                  <>
                    <Download className="w-5 h-5" />
                    Download Private Key File
                  </>
                )}
              </button>

              <div className="border-t border-dark-700 pt-4">
                <p className="text-sm text-dark-400 mb-4">
                  Export initiation cannot prove the file was saved. Confirm only after checking your secure copy. The file includes recovery instructions for restoring backups
                  without the DockerVault app.
                </p>
                {operationError && <p role="alert" className="text-red-400">{operationError}</p>}
                <button className="text-dark-300 mb-4" onClick={closeCustody}>Close and clear private key (custody remains incomplete)</button>
                <button
                  onClick={() => confirmMutation.mutate()}
                  disabled={!downloadedKey || confirmMutation.isPending}
                  className="w-full px-4 py-3 bg-green-600 hover:bg-green-700 disabled:bg-dark-700 text-white disabled:text-dark-500 rounded-lg font-medium transition-colors"
                >
                  {confirmMutation.isPending
                    ? 'Confirming...'
                    : 'I have saved my private key'}
                </button>
                {!downloadedKey && (
                  <p className="text-xs text-dark-500 text-center mt-2">
                    Download the key file first to continue
                  </p>
                )}
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Recovery Instructions Modal */}
      {showInstructions && status?.public_key && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-dark-800 border border-dark-700 rounded-xl max-w-3xl w-full max-h-[90vh] overflow-y-auto">
            <div className="px-6 py-4 border-b border-dark-700 flex items-center justify-between">
              <h2 className="text-lg font-semibold text-dark-100">
                Backup Recovery Instructions
              </h2>
              <button
                onClick={() => setShowInstructions(false)}
                className="text-dark-400 hover:text-dark-200"
              >
                <X className="w-5 h-5" />
              </button>
            </div>
            <div className="p-6 space-y-6">
              {/* Public Key */}
              <div>
                <h3 className="text-sm font-semibold text-dark-200 mb-2">Your Public Key</h3>
                <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{status.public_key}
                </pre>
              </div>

              {/* Recovery Info */}
              <div className="p-4 bg-blue-500/10 border border-blue-500/30 rounded-lg">
                <p className="text-blue-400 text-sm">
                  If you lose access to DockerVault, you can still recover your backups using standard command-line tools.
                </p>
              </div>

              {/* Prerequisites */}
              <div>
                <h3 className="text-sm font-semibold text-dark-200 mb-3">Prerequisites</h3>
                <ul className="space-y-2 text-sm text-dark-300">
                  <li className="flex items-start gap-2">
                    <span className="text-indigo-400 mt-1">•</span>
                    <span>Your private key file (exported during setup)</span>
                  </li>
                  <li className="flex items-start gap-2">
                    <span className="text-indigo-400 mt-1">•</span>
                    <span><code className="text-xs bg-dark-900 px-1.5 py-0.5 rounded">age</code> tool: <a href="https://github.com/FiloSottile/age" target="_blank" rel="noopener noreferrer" className="text-indigo-400 hover:underline">github.com/FiloSottile/age</a></span>
                  </li>
                  <li className="flex items-start gap-2">
                    <span className="text-indigo-400 mt-1">•</span>
                    <span><code className="text-xs bg-dark-900 px-1.5 py-0.5 rounded">openssl</code> (usually pre-installed)</span>
                  </li>
                </ul>
              </div>

              {/* Steps */}
              <div>
                <h3 className="text-sm font-semibold text-dark-200 mb-3">Recovery Steps</h3>
                <p className="text-sm text-dark-300 mb-3">Run these blocks in the same Bash shell. Save <a href="/recover_backup.py" download className="underline">recover_backup.py</a> with your exported key. It needs Python 3, age and OpenSSL, but no running app or Python packages. New archives are authenticated before plaintext is released; legacy CBC requires an independently trusted checksum.</p>
                <div className="space-y-4">
                  <div>
                    <p className="text-sm text-dark-300 mb-2"><span className="font-semibold text-dark-100">1.</span> Save your private key to a file:</p>
                    <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{`set -euo pipefail
umask 077
cat > private_key.txt << 'EOF'
AGE-SECRET-KEY-1XXXXXX...
EOF
chmod 600 private_key.txt`}
                    </pre>
                  </div>

                  <div>
                    <p className="text-sm text-dark-300 mb-2"><span className="font-semibold text-dark-100">2.</span> Check your saved recovery script:</p>
                    <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{`test -f recover_backup.py`}
                    </pre>
                  </div>

                  <div>
                    <p className="text-sm text-dark-300 mb-2"><span className="font-semibold text-dark-100">3.</span> Decrypt the backup:</p>
                    <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{`python3 recover_backup.py --identity private_key.txt --key backup.tar.gz.key \\
    --archive backup.tar.gz.enc --output backup.tar.gz`}
                    </pre>
                  </div>

                  <div>
                    <p className="text-sm text-dark-300 mb-2"><span className="font-semibold text-dark-100">4.</span> Extract the backup:</p>
                    <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{`tar xzf backup.tar.gz`}
                    </pre>
                  </div>

                  <div>
                    <p className="text-sm text-dark-300 mb-2"><span className="font-semibold text-dark-100">5.</span> Clean up:</p>
                    <pre className="bg-dark-900 rounded-lg p-3 text-xs font-mono text-dark-300 overflow-x-auto">
{`# The script automatically removes temporary secrets on success or error.
# Keep your private key secure; delete this working copy when finished.`}
                    </pre>
                  </div>
                </div>
              </div>

              {/* Security Notes */}
              <div className="p-4 bg-amber-500/10 border border-amber-500/30 rounded-lg">
                <h4 className="text-sm font-semibold text-amber-400 mb-2">Security Notes</h4>
                <ul className="space-y-1 text-sm text-amber-400/80">
                  <li>• Keep your private key secure and backed up separately</li>
                  <li>• Never share your private key</li>
                  <li>• New archives authenticate ciphertext; legacy CBC has no authenticity guarantee</li>
                  <li>• Each backup has a unique encryption key</li>
                </ul>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Regenerate Warning Modal */}
      {showRegenerateWarning && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-dark-800 border border-dark-700 rounded-xl max-w-md w-full">
            <div className="p-6">
              <div className="flex items-center gap-3 mb-4">
                <div className="p-2 bg-red-500/10 rounded-lg">
                  <AlertTriangle className="w-5 h-5 text-red-400" />
                </div>
                <h3 className="text-lg font-semibold text-dark-100">
                  Regenerate Encryption Keys?
                </h3>
              </div>
              <p className="text-dark-400 mb-6">
                This will create new encryption keys. <strong className="text-dark-200">All existing encrypted
                backups will become unrecoverable</strong> unless you still have the
                old private key.
              </p>
              <div className="flex gap-3">
                <button
                  onClick={() => setShowRegenerateWarning(false)}
                  className="flex-1 px-4 py-2 bg-dark-700 text-dark-300 rounded-lg font-medium hover:bg-dark-600 transition-colors"
                >
                  Cancel
                </button>
                <button
                  onClick={() => regenerateMutation.mutate()}
                  disabled={regenerateMutation.isPending}
                  className="flex-1 px-4 py-2 bg-red-600 text-white rounded-lg font-medium hover:bg-red-700 transition-colors"
                >
                  {regenerateMutation.isPending ? 'Regenerating...' : 'Regenerate Keys'}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
