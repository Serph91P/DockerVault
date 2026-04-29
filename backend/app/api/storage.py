"""Remote Storage API endpoints"""

import asyncio
import logging
import os
import re
import tempfile
import shutil
from pathlib import Path
from typing import List, Optional

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from starlette.background import BackgroundTask

from ..database import RemoteStorage as RemoteStorageModel
from ..database import get_db
from ..remote_storage import StorageConfig, StorageType, storage_manager
from ..credential_encryption import decrypt_value, encrypt_value
from ..config import settings

router = APIRouter()
logger = logging.getLogger(__name__)


def _validate_ssh_key_path(path: str) -> str:
    """Validate that the SSH key path is within allowed directories."""
    real_path = os.path.realpath(path)
    allowed_dirs = [
        d.strip() for d in settings.ALLOWED_SSH_KEY_DIRS.split(",") if d.strip()
    ]
    for allowed in allowed_dirs:
        if real_path.startswith(
            os.path.realpath(allowed) + os.sep
        ) or real_path == os.path.realpath(allowed):
            return real_path
    raise HTTPException(
        status_code=400,
        detail=f"SSH key path must be within allowed directories: {allowed_dirs}",
    )


class StorageCreate(BaseModel):
    """Create a new remote storage configuration"""

    name: str
    storage_type: StorageType
    enabled: bool = True

    # Connection settings
    host: Optional[str] = None
    port: Optional[int] = None
    username: Optional[str] = None
    password: Optional[str] = None

    # Path settings
    base_path: str = "/backups"

    # SSH/SFTP specific
    ssh_key_path: Optional[str] = None

    # S3 specific
    s3_bucket: Optional[str] = None
    s3_region: Optional[str] = None
    s3_access_key: Optional[str] = None
    s3_secret_key: Optional[str] = None
    s3_endpoint_url: Optional[str] = None

    # WebDAV specific
    webdav_url: Optional[str] = None

    # Rclone specific
    rclone_remote: Optional[str] = None


class StorageUpdate(BaseModel):
    """Update remote storage configuration"""

    name: Optional[str] = None
    enabled: Optional[bool] = None
    host: Optional[str] = None
    port: Optional[int] = None
    username: Optional[str] = None
    password: Optional[str] = None
    base_path: Optional[str] = None
    ssh_key_path: Optional[str] = None
    s3_bucket: Optional[str] = None
    s3_region: Optional[str] = None
    s3_access_key: Optional[str] = None
    s3_secret_key: Optional[str] = None
    s3_endpoint_url: Optional[str] = None
    webdav_url: Optional[str] = None
    rclone_remote: Optional[str] = None


class StorageResponse(BaseModel):
    """Remote storage response"""

    id: int
    name: str
    storage_type: str
    enabled: bool
    host: Optional[str]
    port: Optional[int]
    username: Optional[str]
    base_path: str
    ssh_key_path: Optional[str]
    s3_bucket: Optional[str]
    s3_region: Optional[str]
    s3_endpoint_url: Optional[str]
    webdav_url: Optional[str]
    rclone_remote: Optional[str]
    created_at: str
    updated_at: str

    class Config:
        from_attributes = True


class StorageTestResult(BaseModel):
    """Result of connection test"""

    success: bool
    message: str


@router.get("", response_model=List[StorageResponse])
async def list_storage(db: AsyncSession = Depends(get_db)):
    """List all configured remote storage backends"""
    result = await db.execute(select(RemoteStorageModel))
    storages = result.scalars().all()
    return [
        StorageResponse(
            id=s.id,
            name=s.name,
            storage_type=s.storage_type,
            enabled=s.enabled,
            host=s.host,
            port=s.port,
            username=s.username,
            base_path=s.base_path,
            ssh_key_path=s.ssh_key_path,
            s3_bucket=s.s3_bucket,
            s3_region=s.s3_region,
            s3_endpoint_url=s.s3_endpoint_url,
            webdav_url=s.webdav_url,
            rclone_remote=s.rclone_remote,
            created_at=s.created_at.isoformat(),
            updated_at=s.updated_at.isoformat(),
        )
        for s in storages
    ]


@router.get("/types")
async def list_storage_types():
    """List available storage types with their required fields"""
    return {
        "local": {
            "name": "Local/Network Path",
            "description": "Local filesystem or mounted network storage (NFS, SMB)",
            "required": ["base_path"],
            "optional": [],
        },
        "ssh": {
            "name": "SSH/SFTP",
            "description": "Remote server via SSH with rsync",
            "required": ["host", "username", "base_path"],
            "optional": ["port", "ssh_key_path", "password"],
        },
        "webdav": {
            "name": "WebDAV",
            "description": "WebDAV compatible storage (Nextcloud, ownCloud, etc.)",
            "required": ["webdav_url", "base_path"],
            "optional": ["username", "password"],
        },
        "s3": {
            "name": "S3 Compatible",
            "description": "AWS S3, MinIO, Backblaze B2, Wasabi, etc.",
            "required": ["s3_bucket", "s3_access_key", "s3_secret_key"],
            "optional": ["s3_region", "s3_endpoint_url", "base_path"],
        },
        "ftp": {
            "name": "FTP/FTPS",
            "description": "FTP or FTPS server",
            "required": ["host", "username", "password", "base_path"],
            "optional": ["port"],
        },
        "rclone": {
            "name": "Rclone",
            "description": "Any rclone-supported backend (40+ providers)",
            "required": ["rclone_remote", "base_path"],
            "optional": [],
        },
    }


# ---------------------------------------------------------------------------
# SSH key management
# ---------------------------------------------------------------------------
#
# Generated keys live under ``/app/data/ssh_keys/`` (the first entry of the
# ``ALLOWED_SSH_KEY_DIRS`` setting). Names are restricted to a safe charset
# so the resulting paths cannot escape that directory.

_SSH_KEYS_DIR = Path("/app/data/ssh_keys")
_SSH_NAME_RE = re.compile(r"^[A-Za-z0-9_-]{1,40}$")


def _ssh_key_paths(name: str) -> tuple[Path, Path]:
    if not _SSH_NAME_RE.match(name):
        raise HTTPException(
            status_code=400,
            detail="Key name must be 1-40 chars: letters, digits, '_' or '-'.",
        )
    private = _SSH_KEYS_DIR / name
    public = _SSH_KEYS_DIR / f"{name}.pub"
    return private, public


class SSHKeyGenerateRequest(BaseModel):
    name: str = Field(..., description="Filename (no extension), e.g. 'hetzner_box'")
    comment: Optional[str] = Field(None, description="Comment embedded in the key")
    overwrite: bool = False


class SSHKeyInfo(BaseModel):
    name: str
    private_path: str
    public_path: str
    public_key: str
    fingerprint: Optional[str] = None
    created_at: float


class SSHKeyInstallRequest(BaseModel):
    host: str
    port: int = 22
    username: str
    password: str = Field(..., description="Password for the initial install")
    method: str = Field(
        "auto",
        description=(
            "How to install the key on the remote. "
            "'hetzner' = pipe pubkey to 'install-ssh-key' (Storage Box, port 23). "
            "'authorized_keys' = append to ~/.ssh/authorized_keys. "
            "'auto' = pick hetzner if host ends in your-storagebox.de, else authorized_keys."
        ),
    )


async def _run(
    cmd: List[str], env: Optional[dict] = None, input_bytes: Optional[bytes] = None
) -> tuple[int, str, str]:
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdin=asyncio.subprocess.PIPE if input_bytes is not None else None,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=env,
    )
    stdout, stderr = await proc.communicate(input=input_bytes)
    return (
        proc.returncode if proc.returncode is not None else -1,
        stdout.decode(errors="replace"),
        stderr.decode(errors="replace"),
    )


async def _read_key_info(name: str) -> SSHKeyInfo:
    private, public = _ssh_key_paths(name)
    if not private.is_file() or not public.is_file():
        raise HTTPException(status_code=404, detail="SSH key not found")
    pub_text = public.read_text().strip()
    rc, fp_out, _ = await _run(["ssh-keygen", "-lf", str(public)])
    fingerprint = fp_out.strip() if rc == 0 else None
    return SSHKeyInfo(
        name=name,
        private_path=str(private),
        public_path=str(public),
        public_key=pub_text,
        fingerprint=fingerprint,
        created_at=private.stat().st_mtime,
    )


@router.get("/ssh-keys", response_model=List[SSHKeyInfo])
async def list_ssh_keys():
    """List SSH keypairs generated under ``/app/data/ssh_keys``."""
    if not _SSH_KEYS_DIR.is_dir():
        return []
    out: List[SSHKeyInfo] = []
    for pub in sorted(_SSH_KEYS_DIR.glob("*.pub")):
        name = pub.stem
        if not _SSH_NAME_RE.match(name):
            continue
        priv = _SSH_KEYS_DIR / name
        if not priv.is_file():
            continue
        try:
            out.append(await _read_key_info(name))
        except Exception as exc:  # noqa: BLE001 — skip broken entries
            logger.warning("Skipping malformed key %s: %s", name, exc)
    return out


@router.post("/ssh-keys", response_model=SSHKeyInfo)
async def generate_ssh_key(data: SSHKeyGenerateRequest):
    """Generate a new ed25519 keypair and store it on disk.

    The private key is created with mode 0600 under
    ``/app/data/ssh_keys/<name>`` so OpenSSH accepts it. The public key
    sits next to it as ``<name>.pub``. Use the install endpoint or
    download the public key to register it on the remote.
    """
    private, public = _ssh_key_paths(data.name)
    _SSH_KEYS_DIR.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(_SSH_KEYS_DIR, 0o700)
    except OSError:
        pass

    if private.exists() or public.exists():
        if not data.overwrite:
            raise HTTPException(
                status_code=409, detail="Key with this name already exists"
            )
        private.unlink(missing_ok=True)
        public.unlink(missing_ok=True)

    comment = data.comment or f"dockervault-{data.name}"
    rc, _stdout, stderr = await _run(
        [
            "ssh-keygen",
            "-t",
            "ed25519",
            "-N",
            "",  # no passphrase
            "-C",
            comment,
            "-f",
            str(private),
        ]
    )
    if rc != 0:
        raise HTTPException(
            status_code=500, detail=f"ssh-keygen failed: {stderr.strip()}"
        )

    # Belt-and-braces: enforce strict perms even if ssh-keygen didn't.
    try:
        os.chmod(private, 0o600)
        os.chmod(public, 0o644)
    except OSError as exc:
        logger.warning("Could not set perms on %s: %s", private, exc)

    return await _read_key_info(data.name)


@router.get("/ssh-keys/{name}", response_model=SSHKeyInfo)
async def get_ssh_key(name: str):
    return await _read_key_info(name)


@router.get("/ssh-keys/{name}/public")
async def download_ssh_public_key(name: str):
    _, public = _ssh_key_paths(name)
    if not public.is_file():
        raise HTTPException(status_code=404, detail="Public key not found")
    return FileResponse(
        path=str(public),
        media_type="text/plain",
        filename=f"{name}.pub",
    )


@router.delete("/ssh-keys/{name}")
async def delete_ssh_key(name: str):
    private, public = _ssh_key_paths(name)
    if not private.exists() and not public.exists():
        raise HTTPException(status_code=404, detail="SSH key not found")
    private.unlink(missing_ok=True)
    public.unlink(missing_ok=True)
    return {"success": True}


@router.post("/ssh-keys/{name}/install")
async def install_ssh_key(name: str, data: SSHKeyInstallRequest):
    """Install the public key on a remote SSH server using a one-time password.

    Two strategies:

    - ``hetzner`` — pipes the public key to Hetzner Storage Box's
      ``install-ssh-key`` helper (only works on the SSH-enabled
      port 23 of ``*.your-storagebox.de``).
    - ``authorized_keys`` — generic fallback that appends the key to
      ``~/.ssh/authorized_keys`` after creating ``~/.ssh`` with the
      correct perms.

    The password is consumed once via ``sshpass`` (env var, never
    on the command line) and is not persisted anywhere.
    """
    _, public = _ssh_key_paths(name)
    if not public.is_file():
        raise HTTPException(status_code=404, detail="Public key not found")
    pub_text = public.read_text().strip() + "\n"

    method = data.method
    if method == "auto":
        method = (
            "hetzner"
            if data.host.endswith(".your-storagebox.de") and data.port == 23
            else "authorized_keys"
        )

    base_ssh = [
        "ssh",
        "-p",
        str(data.port),
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "UserKnownHostsFile=/app/data/ssh_known_hosts",
        "-o",
        "ConnectTimeout=30",
        "-o",
        "PreferredAuthentications=password",
        "-o",
        "PubkeyAuthentication=no",
        "-o",
        "NumberOfPasswordPrompts=1",
        f"{data.username}@{data.host}",
    ]
    env = {**os.environ, "SSHPASS": data.password}

    if method == "hetzner":
        cmd = ["sshpass", "-e", *base_ssh, "install-ssh-key"]
    elif method == "authorized_keys":
        # Use a single remote shell pipeline; reads stdin and appends if
        # the key isn't already present. POSIX-only utilities so it works
        # on virtually any sshd target.
        remote_script = (
            "set -e; "
            "umask 077; "
            "mkdir -p ~/.ssh; "
            "chmod 700 ~/.ssh; "
            "touch ~/.ssh/authorized_keys; "
            "chmod 600 ~/.ssh/authorized_keys; "
            "key=$(cat); "
            'grep -qxF "$key" ~/.ssh/authorized_keys || printf "%s\\n" "$key" >> ~/.ssh/authorized_keys'
        )
        cmd = ["sshpass", "-e", *base_ssh, "sh", "-c", remote_script]
    else:
        raise HTTPException(status_code=400, detail=f"Unknown install method: {method}")

    rc, stdout, stderr = await _run(cmd, env=env, input_bytes=pub_text.encode())
    if rc != 0:
        msg = (stderr or stdout).strip() or f"ssh exit code {rc}"
        # Strip noise like the password warning so users see the real error
        msg_lines = [
            line
            for line in msg.splitlines()
            if "warning" not in line.lower() and "permanently added" not in line.lower()
        ]
        clean = "\n".join(msg_lines).strip() or msg
        logger.warning(
            "install-ssh-key failed for %s@%s: %s", data.username, data.host, clean
        )
        raise HTTPException(status_code=400, detail=f"Install failed: {clean}")

    logger.info(
        "Installed key %s on %s@%s:%d via %s",
        name,
        data.username,
        data.host,
        data.port,
        method,
    )
    return {
        "success": True,
        "method": method,
        "message": (stdout or "Key installed").strip()[:500],
    }


@router.post("", response_model=StorageResponse)
async def create_storage(data: StorageCreate, db: AsyncSession = Depends(get_db)):
    """Create a new remote storage configuration"""
    validated_ssh_key_path = None
    if data.ssh_key_path:
        validated_ssh_key_path = _validate_ssh_key_path(data.ssh_key_path)

    storage = RemoteStorageModel(
        name=data.name,
        storage_type=data.storage_type.value,
        enabled=data.enabled,
        host=data.host,
        port=data.port,
        username=data.username,
        password=encrypt_value(data.password) if data.password else data.password,
        base_path=data.base_path,
        ssh_key_path=validated_ssh_key_path,
        s3_bucket=data.s3_bucket,
        s3_region=data.s3_region,
        s3_access_key=encrypt_value(data.s3_access_key)
        if data.s3_access_key
        else data.s3_access_key,
        s3_secret_key=encrypt_value(data.s3_secret_key)
        if data.s3_secret_key
        else data.s3_secret_key,
        s3_endpoint_url=data.s3_endpoint_url,
        webdav_url=data.webdav_url,
        rclone_remote=data.rclone_remote,
    )

    db.add(storage)
    await db.commit()
    await db.refresh(storage)

    # Register with manager
    config = _db_to_config(storage)
    storage_manager.add_storage(config)

    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote,
        created_at=storage.created_at.isoformat(),
        updated_at=storage.updated_at.isoformat(),
    )


@router.get("/{storage_id}", response_model=StorageResponse)
async def get_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Get a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote,
        created_at=storage.created_at.isoformat(),
        updated_at=storage.updated_at.isoformat(),
    )


@router.put("/{storage_id}", response_model=StorageResponse)
async def update_storage(
    storage_id: int, data: StorageUpdate, db: AsyncSession = Depends(get_db)
):
    """Update a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    for field, value in data.model_dump(exclude_unset=True).items():
        if isinstance(value, str) and value == "":
            continue
        if field in ("password", "s3_access_key", "s3_secret_key") and value:
            value = encrypt_value(value)
        if field == "ssh_key_path" and value:
            value = _validate_ssh_key_path(value)
        setattr(storage, field, value)

    await db.commit()
    await db.refresh(storage)

    # Update manager
    storage_manager.remove_storage(storage_id)
    config = _db_to_config(storage)
    storage_manager.add_storage(config)

    return StorageResponse(
        id=storage.id,
        name=storage.name,
        storage_type=storage.storage_type,
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote,
        created_at=storage.created_at.isoformat(),
        updated_at=storage.updated_at.isoformat(),
    )


@router.delete("/{storage_id}")
async def delete_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Delete a remote storage configuration"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    await db.delete(storage)
    await db.commit()

    storage_manager.remove_storage(storage_id)

    return {"message": "Storage deleted"}


@router.post("/{storage_id}/test", response_model=StorageTestResult)
async def test_storage(storage_id: int, db: AsyncSession = Depends(get_db)):
    """Test connection to remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    # Ensure backend is registered
    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    try:
        result = await backend.test_connection()
    except Exception as exc:
        logger.exception("Remote storage test failed for %s", storage_id)
        message = str(exc).strip() or exc.__class__.__name__
        return StorageTestResult(success=False, message=message)

    message = result.get("message") or "Connection test failed"
    return StorageTestResult(success=bool(result.get("success")), message=message)


def _validate_remote_path(path: str) -> str:
    """Validate and normalize a remote file path to prevent traversal."""
    normalized = os.path.normpath(path).lstrip("/\\")
    if ".." in normalized.split(os.sep):
        raise HTTPException(status_code=400, detail="Invalid file path")
    return normalized


def _get_local_backend():
    """Create a LocalStorage backend for the backup directory."""
    from ..remote_storage import LocalStorage, StorageConfig, StorageType

    config = StorageConfig(
        id=0,
        name="Local Backups",
        storage_type=StorageType.LOCAL,
        base_path=settings.BACKUP_BASE_PATH,
    )
    return LocalStorage(config)


class BulkDeleteRequest(BaseModel):
    paths: list[str]


class BulkDownloadRequest(BaseModel):
    paths: list[str]


@router.get("/local/files")
async def list_local_files(path: str = ""):
    """List files in the local backup directory."""
    backend = _get_local_backend()
    safe_path = _validate_remote_path(path) if path else ""
    files = await backend.list_files(safe_path)
    return {"files": files, "path": safe_path}


@router.get("/local/files/download")
async def download_local_file(path: str):
    """Download a file from the local backup directory."""
    backend = _get_local_backend()
    safe_path = _validate_remote_path(path)
    filename = os.path.basename(safe_path)

    temp_dir = tempfile.mkdtemp()
    local_path = Path(temp_dir) / filename

    def cleanup():
        shutil.rmtree(temp_dir, ignore_errors=True)

    try:
        success = await backend.download(safe_path, local_path)
        if not success or not local_path.exists():
            cleanup()
            raise HTTPException(status_code=500, detail="Download failed")

        return FileResponse(
            path=str(local_path),
            filename=filename,
            media_type="application/octet-stream",
            background=BackgroundTask(cleanup),
        )
    except HTTPException:
        raise
    except Exception as e:
        cleanup()
        logger.error(f"Local download failed: {e}")
        raise HTTPException(status_code=500, detail="Download failed")


@router.delete("/local/files")
async def delete_local_file(path: str):
    """Delete a file from the local backup directory."""
    backend = _get_local_backend()
    safe_path = _validate_remote_path(path)

    try:
        success = await backend.delete(safe_path)
        if not success:
            raise HTTPException(status_code=500, detail="Delete failed")
        return {"message": f"Deleted {safe_path}"}
    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Local delete failed: {e}")
        raise HTTPException(status_code=500, detail="Delete failed")


@router.post("/local/files/bulk-delete")
async def bulk_delete_local_files(request: BulkDeleteRequest):
    """Delete multiple files from local backup directory."""
    backend = _get_local_backend()

    results = []
    for path in request.paths:
        safe_path = _validate_remote_path(path)
        try:
            success = await backend.delete(safe_path)
            results.append({"path": path, "success": success})
        except Exception as e:
            results.append({"path": path, "success": False, "error": str(e)})

    succeeded = sum(1 for r in results if r["success"])
    failed = len(results) - succeeded
    return {"results": results, "succeeded": succeeded, "failed": failed}


@router.post("/local/files/bulk-download")
async def bulk_download_local_files(request: BulkDownloadRequest):
    """Download multiple local files as a zip archive."""
    import zipfile as zf_mod

    if not request.paths:
        raise HTTPException(status_code=400, detail="No files specified")

    backend = _get_local_backend()
    temp_dir = tempfile.mkdtemp()
    zip_path = Path(temp_dir) / "download.zip"

    def cleanup():
        shutil.rmtree(temp_dir, ignore_errors=True)

    try:
        first_path = request.paths[0]
        folder_name = first_path.split("/")[0] if "/" in first_path else "download"

        with zf_mod.ZipFile(zip_path, "w", zf_mod.ZIP_DEFLATED) as zf:
            for path in request.paths:
                safe_path = _validate_remote_path(path)
                filename = os.path.basename(safe_path)
                temp_file = Path(temp_dir) / filename
                success = await backend.download(safe_path, temp_file)
                if success and temp_file.exists():
                    zf.write(temp_file, filename)
                    temp_file.unlink()

        return FileResponse(
            path=str(zip_path),
            filename=f"{folder_name}.zip",
            media_type="application/zip",
            background=BackgroundTask(cleanup),
        )
    except HTTPException:
        raise
    except Exception as e:
        cleanup()
        logger.error(f"Local bulk download failed: {e}")
        raise HTTPException(status_code=500, detail="Bulk download failed")


@router.get("/{storage_id}/files")
async def list_files(
    storage_id: int, path: str = "", db: AsyncSession = Depends(get_db)
):
    """List files in remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    safe_path = _validate_remote_path(path) if path else path
    files = await backend.list_files(safe_path)
    return {"files": files, "path": safe_path}


@router.get("/{storage_id}/files/download")
async def download_file(storage_id: int, path: str, db: AsyncSession = Depends(get_db)):
    """Download a file from remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    safe_path = _validate_remote_path(path)
    filename = os.path.basename(safe_path)

    # Download to a temp file
    temp_dir = tempfile.mkdtemp()
    local_path = Path(temp_dir) / filename

    def cleanup():
        shutil.rmtree(temp_dir, ignore_errors=True)

    try:
        success = await backend.download(safe_path, local_path)
        if not success or not local_path.exists():
            cleanup()
            raise HTTPException(status_code=500, detail="Download failed")

        return FileResponse(
            path=str(local_path),
            filename=filename,
            media_type="application/octet-stream",
            background=BackgroundTask(cleanup),
        )
    except HTTPException:
        raise
    except Exception as e:
        cleanup()
        logger.error(f"Remote storage download failed: {e}")
        raise HTTPException(status_code=500, detail="Download failed")


@router.delete("/{storage_id}/files")
async def delete_file(storage_id: int, path: str, db: AsyncSession = Depends(get_db)):
    """Delete a file from remote storage"""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    safe_path = _validate_remote_path(path)

    try:
        success = await backend.delete(safe_path)
        if not success:
            raise HTTPException(status_code=500, detail="Delete failed")
        return {"message": f"Deleted {safe_path}"}
    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Remote storage delete failed: {e}")
        raise HTTPException(status_code=500, detail="Delete failed")


@router.post("/{storage_id}/files/bulk-delete")
async def bulk_delete_files(
    storage_id: int,
    request: BulkDeleteRequest,
    db: AsyncSession = Depends(get_db),
):
    """Delete multiple files from remote storage."""
    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    results = []
    for path in request.paths:
        safe_path = _validate_remote_path(path)
        try:
            success = await backend.delete(safe_path)
            results.append({"path": path, "success": success})
        except Exception as e:
            results.append({"path": path, "success": False, "error": str(e)})

    succeeded = sum(1 for r in results if r["success"])
    failed = len(results) - succeeded
    return {"results": results, "succeeded": succeeded, "failed": failed}


@router.post("/{storage_id}/files/bulk-download")
async def bulk_download_files(
    storage_id: int,
    request: BulkDownloadRequest,
    db: AsyncSession = Depends(get_db),
):
    """Download multiple files as a zip archive."""
    import zipfile as zf_mod

    if not request.paths:
        raise HTTPException(status_code=400, detail="No files specified")

    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    temp_dir = tempfile.mkdtemp()
    zip_path = Path(temp_dir) / "download.zip"

    def cleanup():
        shutil.rmtree(temp_dir, ignore_errors=True)

    try:
        first_path = request.paths[0]
        folder_name = first_path.split("/")[0] if "/" in first_path else "download"

        with zf_mod.ZipFile(zip_path, "w", zf_mod.ZIP_DEFLATED) as zf:
            for path in request.paths:
                safe_path = _validate_remote_path(path)
                filename = os.path.basename(safe_path)
                temp_file = Path(temp_dir) / filename
                success = await backend.download(safe_path, temp_file)
                if success and temp_file.exists():
                    zf.write(temp_file, filename)
                    temp_file.unlink()

        return FileResponse(
            path=str(zip_path),
            filename=f"{folder_name}.zip",
            media_type="application/zip",
            background=BackgroundTask(cleanup),
        )
    except HTTPException:
        raise
    except Exception as e:
        cleanup()
        logger.error(f"Bulk download failed: {e}")
        raise HTTPException(status_code=500, detail="Bulk download failed")


@router.post("/{storage_id}/sync/{backup_id}")
async def sync_backup_to_storage(
    storage_id: int, backup_id: int, db: AsyncSession = Depends(get_db)
):
    """Manually sync a specific backup to remote storage"""
    from pathlib import Path

    from ..database import Backup, BackupTarget

    storage = await db.get(RemoteStorageModel, storage_id)
    if not storage:
        raise HTTPException(status_code=404, detail="Storage not found")

    backup = await db.get(Backup, backup_id)
    if not backup:
        raise HTTPException(status_code=404, detail="Backup not found")

    if not backup.file_path:
        raise HTTPException(status_code=400, detail="Backup has no file")

    target = await db.get(BackupTarget, backup.target_id)

    backend = storage_manager.get_backend(storage_id)
    if not backend:
        config = _db_to_config(storage)
        backend = storage_manager.add_storage(config)

    local_path = Path(backup.file_path)
    remote_path = f"{target.name}/{local_path.name}"

    success = await backend.upload(local_path, remote_path)

    if success:
        return {
            "message": f"Backup synced to {storage.name}",
            "remote_path": remote_path,
        }
    else:
        raise HTTPException(status_code=500, detail="Sync failed")


def _db_to_config(storage: RemoteStorageModel) -> StorageConfig:
    """Convert database model to StorageConfig"""
    return StorageConfig(
        id=storage.id,
        name=storage.name,
        storage_type=StorageType(storage.storage_type),
        enabled=storage.enabled,
        host=storage.host,
        port=storage.port,
        username=storage.username,
        password=decrypt_value(storage.password)
        if storage.password
        else storage.password,
        base_path=storage.base_path,
        ssh_key_path=storage.ssh_key_path,
        s3_bucket=storage.s3_bucket,
        s3_region=storage.s3_region,
        s3_access_key=decrypt_value(storage.s3_access_key)
        if storage.s3_access_key
        else storage.s3_access_key,
        s3_secret_key=decrypt_value(storage.s3_secret_key)
        if storage.s3_secret_key
        else storage.s3_secret_key,
        s3_endpoint_url=storage.s3_endpoint_url,
        webdav_url=storage.webdav_url,
        rclone_remote=storage.rclone_remote,
    )
