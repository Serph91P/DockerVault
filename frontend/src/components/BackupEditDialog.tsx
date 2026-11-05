import type { BackupTarget } from '../api'
import BackupWizard from './BackupWizard'

// Compatibility for existing embedded callers; the sole editor is route-composed.
export default function BackupEditDialog({ target, isOpen, onClose }: { target: BackupTarget; isOpen: boolean; onClose: () => void }) {
  return <BackupWizard editTarget={target} isOpen={isOpen} onClose={onClose} />
}
