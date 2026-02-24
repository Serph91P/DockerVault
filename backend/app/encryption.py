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
        stdout, stderr = await process.communicate(input=dek)

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

    # Write private key to temp file (age requires file input for identity)
    # delete=True ensures the file is removed when the context manager exits
    with tempfile.NamedTemporaryFile(mode="w", suffix=".key", delete=True) as f:
        f.write(private_key)
        f.flush()
        key_file = f.name
        os.chmod(key_file, 0o600)

        try:
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
                raise DecryptionError(f"DEK decryption failed: {stderr.decode()}")

            return stdout

        except Exception as e:
            if isinstance(e, DecryptionError):
                raise
            raise DecryptionError(f"DEK decryption error: {e}")


async def encrypt_file(input_path: Path, output_path: Path, dek: bytes) -> None:
    """
    Encrypt a file using AES-256-CBC with the given DEK.

    Uses openssl for compatibility - can be decrypted without the app.

    Args:
        input_path: Path to file to encrypt
        output_path: Path for encrypted output
        dek: Data Encryption Key (32 bytes)
    """
    try:
        # Use openssl for maximum compatibility
        process = await asyncio.create_subprocess_exec(
            "openssl",
            "enc",
            "-aes-256-cbc",
            "-pbkdf2",
            "-iter",
            "100000",
            "-in",
            str(input_path),
            "-out",
            str(output_path),
            "-pass",
            "stdin",
            stdin=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        # DEK passed via stdin to avoid exposure in /proc/cmdline
        _, stderr = await process.communicate(input=dek.hex().encode())

        if process.returncode != 0:
            raise EncryptionError(f"File encryption failed: {stderr.decode()}")

    except Exception as e:
        if isinstance(e, EncryptionError):
            raise
        raise EncryptionError(f"File encryption error: {e}")


async def decrypt_file(input_path: Path, output_path: Path, dek: bytes) -> None:
    """
    Decrypt a file using AES-256-CBC with the given DEK.

    Args:
        input_path: Path to encrypted file
        output_path: Path for decrypted output
        dek: Data Encryption Key (32 bytes)
    """
    try:
        process = await asyncio.create_subprocess_exec(
            "openssl",
            "enc",
            "-d",
            "-aes-256-cbc",
            "-pbkdf2",
            "-iter",
            "100000",
            "-in",
            str(input_path),
            "-out",
            str(output_path),
            "-pass",
            "stdin",
            stdin=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        # DEK passed via stdin to avoid exposure in /proc/cmdline
        _, stderr = await process.communicate(input=dek.hex().encode())

        if process.returncode != 0:
            raise DecryptionError(f"File decryption failed: {stderr.decode()}")

    except Exception as e:
        if isinstance(e, DecryptionError):
            raise
        raise DecryptionError(f"File decryption error: {e}")


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

    # Encrypt the backup file
    encrypted_path = backup_path.with_suffix(backup_path.suffix + ".enc")
    await encrypt_file(backup_path, encrypted_path, dek)

    # Encrypt the DEK with public key
    encrypted_dek = await encrypt_dek(dek, public_key)

    # Save encrypted DEK alongside backup
    key_path = backup_path.with_suffix(backup_path.suffix + ".key")
    async with aiofiles.open(key_path, "wb") as f:
        await f.write(encrypted_dek)

    # Remove unencrypted backup
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
    # Create temp file for decrypted backup
    with tempfile.NamedTemporaryFile(suffix=".tar.gz", delete=False) as f:
        temp_path = Path(f.name)

    try:
        # Decrypt to temp
        await decrypt_backup(encrypted_path, key_path, private_key, temp_path)

        # List tar contents
        process = await asyncio.create_subprocess_exec(
            "tar",
            "-tzf",
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

### Steps

1. **Save your private key to a file** (if not already):
   ```bash
   cat > private_key.txt << 'EOF'
   AGE-SECRET-KEY-1XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
   EOF
   chmod 600 private_key.txt
   ```

2. **Decrypt the DEK (Data Encryption Key)**:
   ```bash
   age -d -i private_key.txt backup.tar.gz.key > dek.txt
   ```

3. **Decrypt the backup**:
   ```bash
   openssl enc -d -aes-256-cbc -pbkdf2 -iter 100000 \\
       -in backup.tar.gz.enc \\
       -out backup.tar.gz \\
       -pass file:dek.txt
   ```

4. **Extract the backup**:
   ```bash
   tar xzf backup.tar.gz
   ```

5. **Clean up**:
   ```bash
   rm dek.txt  # Don't leave the DEK lying around
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
- The encrypted backups are safe to store anywhere
- Each backup has a unique encryption key
"""
