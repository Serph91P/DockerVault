"""
Backup Encryption Module

Uses envelope encryption:
- Each backup gets a unique DEK (Data Encryption Key)
- DEK is encrypted with the user's public key
- Backups can be restored without the app using standard tools

Supported key types: age (modern, recommended)
"""

import asyncio
import logging
import os
import secrets
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import aiofiles

logger = logging.getLogger(__name__)

# DEK size in bytes (256-bit for AES-256)
DEK_SIZE = 32


def key_sidecar_path(archive_path: str) -> str:
    """Replace only the terminal encryption suffix, never directory/stem segments."""
    return archive_path.removesuffix(".enc") + ".key"


@dataclass
class KeyPair:
    """Encryption key pair"""

    public_key: str
    private_key: str


@dataclass
class EncryptedBackup:
    """Result of backup encryption"""

    encrypted_path: Path
    key_path: Path
    dek_encrypted: bytes


class EncryptionError(Exception):
    """Encryption operation failed"""

    pass


class DecryptionError(Exception):
    """Decryption operation failed"""

    pass


def _check_age_installed() -> bool:
    """Check if age is installed"""
    try:
        result = subprocess.run(
            ["age", "--version"], capture_output=True, text=True, timeout=5
        )
        return result.returncode == 0
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


def _check_age_keygen_installed() -> bool:
    """Check if age-keygen is installed"""
    try:
        result = subprocess.run(
            ["age-keygen", "--version"], capture_output=True, text=True, timeout=5
        )
        return result.returncode == 0
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


async def generate_key_pair() -> KeyPair:
    """
    Generate a new age key pair.

    Returns:
        KeyPair with public and private keys

    Raises:
        EncryptionError if key generation fails
    """
    if not _check_age_keygen_installed():
        raise EncryptionError("age-keygen not installed. Install with: apt install age")

    try:
        process = await asyncio.create_subprocess_exec(
            "age-keygen",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            raise EncryptionError(f"Key generation failed: {stderr.decode()}")

        # Parse output - age-keygen outputs:
        # # created: 2024-01-01T00:00:00Z
        # # public key: age1...
        # AGE-SECRET-KEY-1...
        output = stdout.decode()
        lines = output.strip().split("\n")

        private_key = None
        public_key = None

        for line in lines:
            if line.startswith("# public key:"):
                public_key = line.split(": ", 1)[1].strip()
            elif line.startswith("AGE-SECRET-KEY-"):
                private_key = line.strip()

        if not public_key or not private_key:
            raise EncryptionError("Failed to parse generated keys")

        return KeyPair(public_key=public_key, private_key=private_key)

    except asyncio.TimeoutError:
        raise EncryptionError("Key generation timed out")
    except Exception as e:
        if isinstance(e, EncryptionError):
            raise
        raise EncryptionError(f"Key generation error: {e}")


def generate_dek() -> bytes:
    """Generate a random Data Encryption Key"""
    return secrets.token_bytes(DEK_SIZE)


async def encrypt_dek(dek: bytes, public_key: str) -> bytes:
    """
    Encrypt DEK with public key using age.

    Args:
        dek: Data Encryption Key bytes
        public_key: age public key (age1...)

    Returns:
        Encrypted DEK bytes
    """
    if not _check_age_installed():
        raise EncryptionError("age not installed")
    from .recovery import KEY_MAGIC

    if len(dek) != DEK_SIZE:
        raise EncryptionError("Invalid DEK size")

    try:
        process = await asyncio.create_subprocess_exec(
            "age",
            "-r",
            public_key,
            "-a",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        # Bind the authenticated archive version inside the authenticated age envelope.
        stdout, stderr = await process.communicate(input=KEY_MAGIC + dek)

        if process.returncode != 0:
            raise EncryptionError(f"DEK encryption failed: {stderr.decode()}")

        return stdout

    except Exception as e:
        if isinstance(e, EncryptionError):
            raise
        raise EncryptionError(f"DEK encryption error: {e}")


async def decrypt_dek(encrypted_dek: bytes, private_key: str) -> bytes:
    """
    Decrypt DEK with private key using age.

    Args:
        encrypted_dek: Encrypted DEK bytes
        private_key: age private key (AGE-SECRET-KEY-...)

    Returns:
        Decrypted DEK bytes
    """
    if not _check_age_installed():
        raise DecryptionError("age not installed")

    if not encrypted_dek or not encrypted_dek.strip():
        raise DecryptionError(
            "Encrypted DEK is empty. The .key file may be missing or corrupted."
        )

    # Strip whitespace from key (clipboard/textarea may add extra chars)
    private_key = private_key.strip()

    if not private_key.startswith("AGE-SECRET-KEY-"):
        raise DecryptionError(
            "Invalid private key format. Key must start with AGE-SECRET-KEY-"
        )

    # Write private key to temp file (age requires file input for identity).
    # Use mkstemp + manual cleanup: the file is closed before the subprocess
    # reads it, guaranteeing the content is fully flushed to disk.
    fd, key_file = tempfile.mkstemp(suffix=".key")
    try:
        os.chmod(key_file, 0o600)
        os.write(fd, private_key.encode())
        os.write(fd, b"\n")
        os.close(fd)

        process = await asyncio.create_subprocess_exec(
            "age",
            "-d",
            "-i",
            key_file,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate(input=encrypted_dek)

        if process.returncode != 0:
            logger.error(
                "age decryption failed (exit %d): %s",
                process.returncode,
                stderr.decode(),
            )
            raise DecryptionError(f"DEK decryption failed: {stderr.decode()}")

        return stdout

    except Exception as e:
        if isinstance(e, DecryptionError):
            raise
        raise DecryptionError(f"DEK decryption error: {e}")
    finally:
        try:
            os.unlink(key_file)
        except OSError:
            pass


async def _crypt_file(
    input_path: Path, output_path: Path, dek: bytes, *, decrypt: bool
) -> None:
    """Run the standalone versioned codec without exposing key material in argv."""
    import signal
    import sys

    error_type = DecryptionError if decrypt else EncryptionError
    fd, key_file = tempfile.mkstemp(suffix=".key-material")
    process = None
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(dek)
        args = [
            sys.executable,
            str(Path(__file__).with_name("recovery.py")),
            "--archive",
            str(input_path),
            "--output",
            str(output_path),
            "--key-material-file",
            key_file,
        ]
        if not decrypt:
            args.append("--encrypt")
        process = await asyncio.create_subprocess_exec(
            *args, stderr=asyncio.subprocess.PIPE, start_new_session=True
        )
        try:
            _, stderr = await process.communicate()
        except asyncio.CancelledError:
            if process.returncode is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            settled = asyncio.create_task(process.communicate())
            while not settled.done():
                try:
                    await asyncio.shield(settled)
                except asyncio.CancelledError:
                    continue
            settled.result()
            raise
        if process.returncode:
            raise error_type(stderr.decode().strip() or "Archive codec failed")
    except Exception as exc:
        if isinstance(exc, error_type):
            raise
        raise error_type(f"Archive codec failed: {exc}") from exc
    finally:
        os.unlink(key_file)


async def encrypt_file(input_path: Path, output_path: Path, dek: bytes) -> None:
    """Write a versioned AES-CBC encrypt-then-HMAC archive, atomically."""
    await _crypt_file(input_path, output_path, dek, decrypt=False)


async def decrypt_file(input_path: Path, output_path: Path, dek: bytes) -> None:
    """Authenticate new archives before publishing plaintext; explicitly read legacy CBC."""
    await _crypt_file(input_path, output_path, dek, decrypt=True)


async def encrypt_backup(
    backup_path: Path,
    public_key: str,
) -> EncryptedBackup:
    """
    Encrypt a backup file with envelope encryption.

    Creates:
    - backup.tar.gz.enc (encrypted backup)
    - backup.tar.gz.key (encrypted DEK, ASCII armored)

    Args:
        backup_path: Path to unencrypted backup file
        public_key: age public key for DEK encryption

    Returns:
        EncryptedBackup with paths and encrypted DEK
    """
    # Generate unique DEK for this backup
    dek = generate_dek()

    # Encrypt the backup file. Write to a temporary path first and atomically
    # rename so a crash during encryption never leaves a half-written .enc
    # file alongside the original (which would later be deleted).
    encrypted_path = backup_path.with_suffix(backup_path.suffix + ".enc")
    encrypted_tmp = encrypted_path.with_suffix(encrypted_path.suffix + ".tmp")
    if encrypted_tmp.exists():
        try:
            encrypted_tmp.unlink()
        except OSError:
            pass
    await encrypt_file(backup_path, encrypted_tmp, dek)

    # fsync the encrypted file before the rename so the bytes are durable.
    fd = os.open(str(encrypted_tmp), os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(str(encrypted_tmp), str(encrypted_path))

    # Encrypt the DEK with public key
    encrypted_dek = await encrypt_dek(dek, public_key)

    # Save encrypted DEK alongside backup – use synchronous write with
    # explicit fsync so the key survives unexpected container restarts.
    key_path = backup_path.with_suffix(backup_path.suffix + ".key")
    fd = os.open(str(key_path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.write(fd, encrypted_dek)
        os.fsync(fd)
    finally:
        os.close(fd)

    # Verify the .key was written correctly before removing the plain backup
    actual_size = os.path.getsize(key_path)
    if actual_size == 0 or actual_size != len(encrypted_dek):
        raise EncryptionError(
            f"Key file verification failed: expected {len(encrypted_dek)} bytes, "
            f"got {actual_size} at {key_path}"
        )

    # Remove unencrypted backup only after both .enc and .key are durable.
    backup_path.unlink()

    logger.info(f"Encrypted backup: {encrypted_path}")

    return EncryptedBackup(
        encrypted_path=encrypted_path, key_path=key_path, dek_encrypted=encrypted_dek
    )


async def decrypt_backup(
    encrypted_path: Path,
    key_path: Path,
    private_key: str,
    output_path: Optional[Path] = None,
) -> Path:
    """
    Decrypt a backup file.

    Args:
        encrypted_path: Path to encrypted backup (.enc)
        key_path: Path to encrypted DEK file (.key)
        private_key: age private key
        output_path: Optional output path (default: remove .enc suffix)

    Returns:
        Path to decrypted backup
    """
    # Read encrypted DEK
    async with aiofiles.open(key_path, "rb") as f:
        encrypted_dek = await f.read()

    if not encrypted_dek or not encrypted_dek.strip():
        raise DecryptionError(f"Encryption key file is empty or corrupted: {key_path}")

    # Decrypt DEK
    dek = await decrypt_dek(encrypted_dek, private_key)

    # Determine output path
    if output_path is None:
        # Remove .enc suffix
        output_path = encrypted_path.with_suffix("")

    # Decrypt backup
    await decrypt_file(encrypted_path, output_path, dek)

    logger.info(f"Decrypted backup: {output_path}")

    return output_path


async def list_backup_contents(
    encrypted_path: Path,
    key_path: Path,
    private_key: str,
) -> list[dict]:
    """
    List contents of an encrypted backup without fully extracting.

    Returns list of files with name, size, and type.
    """
    # Derive temp suffix from original archive name (strip .enc)
    original_name = encrypted_path.name
    if original_name.endswith(".enc"):
        original_name = original_name[: -len(".enc")]
    if ".tar." in original_name:
        tar_suffix = original_name[original_name.index(".tar") :]
    elif original_name.endswith(".tar"):
        tar_suffix = ".tar"
    else:
        tar_suffix = ".tar.gz"

    with tempfile.NamedTemporaryFile(suffix=tar_suffix, delete=False) as f:
        temp_path = Path(f.name)

    try:
        # Decrypt to temp
        await decrypt_backup(encrypted_path, key_path, private_key, temp_path)

        # List tar contents — use -tf (no gzip flag) and let tar auto-detect
        process = await asyncio.create_subprocess_exec(
            "tar",
            "-tf",
            str(temp_path),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, _ = await process.communicate()

        files = []
        for line in stdout.decode().strip().split("\n"):
            if line:
                files.append(
                    {
                        "name": line,
                        "is_dir": line.endswith("/"),
                    }
                )

        return files

    finally:
        if temp_path.exists():
            temp_path.unlink()


async def extract_single_file(
    encrypted_path: Path,
    key_path: Path,
    private_key: str,
    file_path: str,
    output_dir: Path,
) -> Path:
    """
    Extract a single file from an encrypted backup.

    Args:
        encrypted_path: Path to encrypted backup
        key_path: Path to encrypted DEK
        private_key: age private key
        file_path: Path within the archive to extract
        output_dir: Directory to extract to

    Returns:
        Path to extracted file
    """
    # Create temp file for decrypted backup
    with tempfile.NamedTemporaryFile(suffix=".tar.gz", delete=False) as f:
        temp_path = Path(f.name)

    try:
        # Decrypt to temp
        await decrypt_backup(encrypted_path, key_path, private_key, temp_path)

        # Extract single file
        output_dir.mkdir(parents=True, exist_ok=True)

        process = await asyncio.create_subprocess_exec(
            "tar",
            "-xzf",
            str(temp_path),
            "-C",
            str(output_dir),
            file_path,
            stderr=asyncio.subprocess.PIPE,
        )
        _, stderr = await process.communicate()

        if process.returncode != 0:
            raise DecryptionError(f"Extraction failed: {stderr.decode()}")

        return output_dir / file_path

    finally:
        if temp_path.exists():
            temp_path.unlink()


def get_recovery_instructions(public_key: str) -> str:
    """
    Generate recovery instructions for the user.
    """
    return f"""
# DockerVault Backup Recovery Instructions

## Your Public Key
```
{public_key}
```

## Recovery WITHOUT the DockerVault App

If you lose access to DockerVault, you can still recover your backups
using standard command-line tools.

### Prerequisites
- Your private key file (the one you exported during setup)
- `age` tool installed: https://github.com/FiloSottile/age
- `openssl` (usually pre-installed)
- Python 3 and the standalone `recover_backup.py` script. Save it with your keys:
  `/recover_backup.py` on your DockerVault installation, or obtain `backend/app/recovery.py`
  from the trusted DockerVault release source and save it as `recover_backup.py`.

### Steps

Run the blocks in the same Bash shell. The standalone script needs no Python
packages or running DockerVault instance. It authenticates v1 archives before
releasing plaintext, and converts legacy raw binary DEKs to newline-free hex.
Legacy CBC archives remain readable but cannot provide cryptographic authenticity;
verify them against a separately trusted checksum before extraction. Never bypass
an authentication error by stripping the version header or changing the key file.

1. **Save your private key to a file** (if not already):
   ```bash
   set -euo pipefail
   umask 077
   cat > private_key.txt << 'EOF'
   AGE-SECRET-KEY-1XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
   EOF
   chmod 600 private_key.txt
   ```

2. **Check the saved standalone script**:
   ```bash
   test -f recover_backup.py
   ```

3. **Decrypt the backup**:
   ```bash
   python3 recover_backup.py --identity private_key.txt --key backup.tar.gz.key \\
       --archive backup.tar.gz.enc --output backup.tar.gz
   ```

4. **Extract the backup**:
   ```bash
   tar xzf backup.tar.gz
   ```

5. **Clean up**:
   ```bash
   # The script removes temporary DEK/password/plaintext files on success or error.
   # Keep your private key secure; delete this working copy when finished.
   ```

### Extract a Single File
```bash
# After step 3, list contents:
tar tzf backup.tar.gz

# Extract specific file:
tar xzf backup.tar.gz path/to/specific/file
```

## Security Notes
- Keep your private key secure and backed up separately
- Never share your private key
- New v1 archives authenticate their ciphertext; legacy CBC has no authenticity guarantee
- Each backup has a unique encryption key
"""
