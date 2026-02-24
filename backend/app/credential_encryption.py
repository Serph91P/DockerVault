"""
Symmetric encryption for remote storage credentials at rest.

Uses Fernet (AES-128-CBC with HMAC-SHA256) from the cryptography library.
Encrypted values are prefixed with 'fernet:' to distinguish them from
legacy plaintext values, enabling backward-compatible migration.
"""

import logging
import os
from pathlib import Path

import stat

from cryptography.fernet import Fernet, InvalidToken

from app.config import settings

logger = logging.getLogger(__name__)

FERNET_PREFIX = "fernet:"
_KEY_FILE_PATH = Path("/app/data/.credential_key")
_fernet: Fernet | None = None


def _enforce_key_file_permissions() -> None:
    """Verify and fix key file permissions to 0600 on startup."""
    if not _KEY_FILE_PATH.exists():
        return
    current = _KEY_FILE_PATH.stat().st_mode & 0o777
    if current != 0o600:
        logger.warning(
            "Key file %s had insecure permissions %o, fixing to 0600",
            _KEY_FILE_PATH,
            current,
        )
        os.chmod(_KEY_FILE_PATH, 0o600)


def _get_fernet() -> Fernet:
    """Get or initialize the Fernet instance, auto-generating a key if needed."""
    global _fernet
    if _fernet is not None:
        return _fernet

    key = settings.CREDENTIAL_ENCRYPTION_KEY

    if not key:
        if _KEY_FILE_PATH.exists():
            key = _KEY_FILE_PATH.read_text().strip()
            _enforce_key_file_permissions()
            logger.info("Loaded credential encryption key from %s", _KEY_FILE_PATH)
        else:
            key = Fernet.generate_key().decode()
            _KEY_FILE_PATH.parent.mkdir(parents=True, exist_ok=True)
            _KEY_FILE_PATH.write_text(key)
            os.chmod(_KEY_FILE_PATH, 0o600)
            logger.warning(
                "Auto-generated credential encryption key and saved to %s. "
                "Back up this file to avoid losing access to encrypted credentials.",
                _KEY_FILE_PATH,
            )

    _fernet = Fernet(key.encode() if isinstance(key, str) else key)
    return _fernet


def encrypt_value(plaintext: str) -> str:
    """Encrypt a plaintext string, returning a 'fernet:'-prefixed ciphertext."""
    if not plaintext:
        return plaintext
    f = _get_fernet()
    token = f.encrypt(plaintext.encode()).decode()
    return f"{FERNET_PREFIX}{token}"


def decrypt_value(ciphertext: str) -> str:
    """Decrypt a 'fernet:'-prefixed ciphertext. Returns input unchanged if not prefixed (backward compat)."""
    if not ciphertext or not ciphertext.startswith(FERNET_PREFIX):
        return ciphertext
    f = _get_fernet()
    token = ciphertext[len(FERNET_PREFIX) :]
    return f.decrypt(token.encode()).decode()
