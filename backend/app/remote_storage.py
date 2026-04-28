"""
Remote Storage Module - Upload backups to remote destinations

Supported backends:
- SSH/SFTP
- WebDAV
- S3 (AWS, MinIO, Backblaze B2, etc.)
- FTP/FTPS
- Rclone (40+ cloud providers)
- Local/Network paths (NFS, SMB mounted)
"""

import asyncio
import hashlib
import logging
import os
import random
import shlex
import shutil
from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, AsyncIterator, Dict, List, Optional

import aiofiles
import aiohttp

from app.config import settings

logger = logging.getLogger(__name__)


def _format_exception_message(exc: Exception) -> str:
    message = str(exc).strip()
    name = exc.__class__.__name__
    return f"{name}: {message}" if message else name


async def _stream_file_chunks(path: Path, chunk_size: int) -> AsyncIterator[bytes]:
    """Yield ``chunk_size``-byte blocks from ``path`` without loading all of it
    into memory. Used for streaming uploads that need a known Content-Length.
    """
    async with aiofiles.open(path, "rb") as f:
        while True:
            chunk = await f.read(chunk_size)
            if not chunk:
                break
            yield chunk


def _retry_wait_seconds(attempt: int, base: float = 1.0, cap: float = 60.0) -> float:
    """Exponential backoff with full jitter, capped at ``cap`` seconds."""
    upper = min(cap, base * (2**attempt))
    return random.uniform(0.0, upper)


class StorageType(str, Enum):
    LOCAL = "local"
    SSH = "ssh"
    SFTP = "sftp"
    WEBDAV = "webdav"
    S3 = "s3"
    FTP = "ftp"
    RCLONE = "rclone"


@dataclass
class StorageConfig:
    """Configuration for a remote storage backend"""

    id: int
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
    ssh_key_passphrase: Optional[str] = None

    # S3 specific
    s3_bucket: Optional[str] = None
    s3_region: Optional[str] = None
    s3_access_key: Optional[str] = None
    s3_secret_key: Optional[str] = None
    s3_endpoint_url: Optional[str] = None  # For MinIO, Backblaze, etc.

    # WebDAV specific
    webdav_url: Optional[str] = None

    # Rclone specific
    rclone_remote: Optional[str] = None  # Name of rclone remote config

    # Additional options
    extra_options: Dict[str, Any] = None

    def __post_init__(self):
        if self.extra_options is None:
            self.extra_options = {}


class StorageBackend(ABC):
    """Abstract base class for storage backends"""

    def __init__(self, config: StorageConfig):
        self.config = config

    @abstractmethod
    async def upload(self, local_path: Path, remote_path: str) -> bool:
        """Upload a file to remote storage"""
        pass

    @abstractmethod
    async def download(self, remote_path: str, local_path: Path) -> bool:
        """Download a file from remote storage"""
        pass

    @abstractmethod
    async def delete(self, remote_path: str) -> bool:
        """Delete a file from remote storage"""
        pass

    @abstractmethod
    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        """List files in remote directory"""
        pass

    @abstractmethod
    async def test_connection(self) -> Dict[str, Any]:
        """Test the connection to remote storage"""
        pass

    async def get_checksum(self, local_path: Path) -> str:
        """Calculate SHA256 checksum of local file"""
        sha256_hash = hashlib.sha256()
        async with aiofiles.open(local_path, "rb") as f:
            while chunk := await f.read(8192):
                sha256_hash.update(chunk)
        return sha256_hash.hexdigest()


class LocalStorage(StorageBackend):
    """Local/Network storage (NFS, SMB mounted paths)"""

    def _safe_resolve(self, remote_path: str) -> Path:
        """Resolve remote_path under base_path, rejecting traversal attempts."""
        base = Path(self.config.base_path).resolve()
        resolved = (base / remote_path).resolve()
        if not str(resolved).startswith(str(base) + "/") and resolved != base:
            raise ValueError("Path traversal detected")
        return resolved

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            dest = self._safe_resolve(remote_path)
            dest.parent.mkdir(parents=True, exist_ok=True)

            # Use async copy
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, shutil.copy2, str(local_path), str(dest))

            logger.info(f"Copied {local_path} to {dest}")
            return True
        except Exception as e:
            logger.error(f"Local upload failed: {e}")
            return False

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            src = self._safe_resolve(remote_path)
            local_path.parent.mkdir(parents=True, exist_ok=True)

            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, shutil.copy2, str(src), str(local_path))

            return True
        except Exception as e:
            logger.error(f"Local download failed: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            path = self._safe_resolve(remote_path)
            if path.exists():
                path.unlink()
            return True
        except Exception as e:
            logger.error(f"Local delete failed: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            path = self._safe_resolve(remote_path)
            files = []
            if path.exists():
                for f in path.iterdir():
                    files.append(
                        {
                            "name": f.name,
                            "size": f.stat().st_size if f.is_file() else 0,
                            "is_dir": f.is_dir(),
                            "modified": f.stat().st_mtime,
                        }
                    )
            return files
        except Exception as e:
            logger.error(f"Local list failed: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        path = Path(self.config.base_path)
        try:
            path.mkdir(parents=True, exist_ok=True)
            test_file = path / ".backup_test"
            test_file.write_text("test")
            test_file.unlink()
            return {"success": True, "message": f"Path {path} is writable"}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("Local storage test failed: %s", message)
            return {"success": False, "message": message}


class SSHStorage(StorageBackend):
    """SSH/SFTP storage using rsync or scp"""

    # Persistent known_hosts file under the data volume so host key
    # entries survive container restarts. The first connection to a
    # new host is auto-accepted (TOFU) thanks to
    # ``StrictHostKeyChecking=accept-new``; subsequent connections
    # verify against the recorded fingerprint.
    KNOWN_HOSTS_FILE = "/app/data/ssh_known_hosts"

    def _common_ssh_options(self) -> List[str]:
        """SSH ``-o`` options applied to every ssh/rsync invocation.

        - ``StrictHostKeyChecking=accept-new``: trust-on-first-use.
          Without this, the first connection to a new host fails with
          ``Host key verification failed`` because the container has no
          pre-populated known_hosts file.
        - ``UserKnownHostsFile``: persistent path under ``/app/data`` so
          accepted host keys survive container restarts.
        - ``BatchMode=yes``: never prompt for a password — only when we
          authenticate with a key. With password auth we MUST allow
          ``sshpass`` to feed the prompt, so BatchMode is omitted.
        """
        try:
            Path(self.KNOWN_HOSTS_FILE).touch(exist_ok=True)
        except OSError:
            pass
        opts = [
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            f"UserKnownHostsFile={self.KNOWN_HOSTS_FILE}",
            "-o",
            "ConnectTimeout=30",
            "-o",
            "ServerAliveInterval=15",
        ]
        # Only enforce BatchMode when we're using key auth. With a
        # password we *need* the prompt so ``sshpass -e`` can answer it.
        if not self._uses_password_auth():
            opts.extend(["-o", "BatchMode=yes"])
        # Useful when the box only allows password auth (Hetzner Storage
        # Box subaccounts) — prevents OpenSSH from offering keys it
        # cannot use, which would otherwise eat through MaxAuthTries.
        if self._uses_password_auth():
            opts.extend(
                [
                    "-o",
                    "PreferredAuthentications=password",
                    "-o",
                    "PubkeyAuthentication=no",
                ]
            )
        return opts

    def _uses_password_auth(self) -> bool:
        """True if the user configured a password and no explicit key."""
        return bool(self.config.password) and not self.config.ssh_key_path

    def _wrap_with_sshpass(self, cmd: List[str]) -> tuple[List[str], dict]:
        """If using password auth, prepend ``sshpass -e`` and return the
        environment carrying ``SSHPASS``. Avoids putting the password on
        the command line where it would show up in ``ps``.
        """
        if not self._uses_password_auth():
            return cmd, {}
        env = {**os.environ, "SSHPASS": self.config.password or ""}
        return ["sshpass", "-e", *cmd], env

    def _get_ssh_options(self) -> List[str]:
        """Build SSH options for rsync's ``-e`` argument."""
        ssh_parts = list(self._common_ssh_options())
        if self.config.port:
            ssh_parts.extend(["-p", str(self.config.port)])
        if self.config.ssh_key_path:
            ssh_parts.extend(["-i", self.config.ssh_key_path])
        # rsync needs a single shell-quoted command string.
        return ["-e", "ssh " + " ".join(shlex.quote(p) for p in ssh_parts)]

    def _get_remote_path(self, remote_path: str) -> str:
        """Build full remote path with user@host prefix"""
        user_host = (
            f"{self.config.username}@{self.config.host}"
            if self.config.username
            else self.config.host
        )
        full_path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
        return f"{user_host}:{full_path}"

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            # Use rsync for efficient transfer. ``--mkpath`` (rsync 3.2.3+)
            # makes the receiver create any missing path components on its
            # own, so we no longer need an ``ssh mkdir -p`` round-trip.
            # That prep step would silently fail on shell-restricted
            # SSH endpoints like Hetzner Storage Box ("Command not
            # found."), leaving rsync to error out with
            # ``change_dir failed: No such file or directory``.
            cmd = ["rsync", "-avz", "--mkpath", "--progress"]
            cmd.extend(self._get_ssh_options())
            cmd.append(str(local_path))
            cmd.append(self._get_remote_path(remote_path))
            cmd, env = self._wrap_with_sshpass(cmd)

            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env or None,
            )
            stdout, stderr = await process.communicate()

            if process.returncode == 0:
                logger.info(f"SSH upload successful: {remote_path}")
                return True
            else:
                logger.error(f"SSH upload failed: {stderr.decode()}")
                return False
        except Exception as e:
            logger.error(f"SSH upload error: {e}")
            return False

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            local_path.parent.mkdir(parents=True, exist_ok=True)

            cmd = ["rsync", "-avz", "--progress"]
            cmd.extend(self._get_ssh_options())
            cmd.append(self._get_remote_path(remote_path))
            cmd.append(str(local_path))
            cmd, env = self._wrap_with_sshpass(cmd)

            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env or None,
            )
            stdout, stderr = await process.communicate()

            return process.returncode == 0
        except Exception as e:
            logger.error(f"SSH download error: {e}")
            return False

    def _build_ssh_command(self, remote_cmd: str) -> List[str]:
        """Build SSH command.

        Note: The remote_cmd should already have paths sanitized with shlex.quote()
        before being passed to this method.
        """
        cmd = ["ssh", *self._common_ssh_options()]
        if self.config.port:
            cmd.extend(["-p", str(self.config.port)])
        if self.config.ssh_key_path:
            cmd.extend(["-i", self.config.ssh_key_path])

        user_host = (
            f"{self.config.username}@{self.config.host}"
            if self.config.username
            else self.config.host
        )
        cmd.append(user_host)
        cmd.append(remote_cmd)
        return cmd

    async def delete(self, remote_path: str) -> bool:
        # Use the SFTP subsystem so deletion works on shell-restricted
        # SSH endpoints (Hetzner Storage Box, rssh, etc.) that reject
        # arbitrary commands. ``rm`` in sftp batch mode targets a single
        # remote file and is functionally equivalent to ``ssh ... rm -f``.
        try:
            full_path = f"{self.config.base_path}/{remote_path}"
            cmd, env = self._build_sftp_command()
            # ``-`` after rm means: continue even if the file does not
            # exist, mirroring ``rm -f`` semantics.
            batch = f"-rm {self._sftp_quote(full_path)}\nbye\n".encode()

            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env or None,
            )
            await process.communicate(input=batch)
            return process.returncode == 0
        except Exception as e:
            logger.error(f"SSH delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        # Use SFTP ``ls -l`` instead of an ssh+ls shell command so this
        # works on shell-restricted endpoints (Hetzner Storage Box).
        # Trade-off: the timestamp format is the standard ``Mon DD HH:MM``
        # (recent) or ``Mon DD  YYYY`` (older) — we parse both into epoch
        # seconds. For files with spaces in the name, sftp returns them
        # verbatim as the trailing field.
        try:
            full_path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
            cmd, env = self._build_sftp_command()
            batch = f"ls -l {self._sftp_quote(full_path)}\nbye\n".encode()

            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env or None,
            )
            stdout, _stderr = await process.communicate(input=batch)

            files: List[Dict[str, Any]] = []
            for raw_line in stdout.decode(errors="replace").splitlines():
                line = raw_line.rstrip()
                if not line or line.startswith("sftp>") or line.startswith("total "):
                    continue
                # Expected format: ``-rw-r--r--  1 owner group  1234 Mon DD HH:MM name``
                parts = line.split(None, 8)
                if len(parts) < 9 or len(parts[0]) < 1 or parts[0][0] not in "-dlbcps":
                    continue
                try:
                    size = int(parts[4])
                except (ValueError, IndexError):
                    size = 0
                name = parts[8]
                if name in (".", ".."):
                    continue
                modified = self._parse_sftp_timestamp(parts[5], parts[6], parts[7])
                files.append(
                    {
                        "name": name,
                        "size": size,
                        "is_dir": parts[0].startswith("d"),
                        "modified": modified,
                    }
                )
            return files
        except Exception as e:
            logger.error(f"SSH list error: {e}")
            return []

    @staticmethod
    def _sftp_quote(path: str) -> str:
        """Quote a path for an sftp batch line.

        sftp's batch parser does not understand POSIX shell escaping; it
        treats double-quoted strings as a single argument. Embedded
        double quotes are escaped with a backslash.
        """
        return '"' + path.replace("\\", "\\\\").replace('"', '\\"') + '"'

    def _build_sftp_command(self) -> tuple[List[str], dict]:
        """Assemble an ``sftp -b -`` invocation reusing the same auth /
        host-key options as ssh, wrapped with sshpass when needed.
        """
        cmd = ["sftp", "-b", "-", *self._common_ssh_options()]
        if self.config.port:
            cmd.extend(["-P", str(self.config.port)])
        if self.config.ssh_key_path:
            cmd.extend(["-i", self.config.ssh_key_path])
        user_host = (
            f"{self.config.username}@{self.config.host}"
            if self.config.username
            else self.config.host
        )
        cmd.append(user_host)
        return self._wrap_with_sshpass(cmd)

    @staticmethod
    def _parse_sftp_timestamp(month: str, day: str, time_or_year: str) -> float:
        """Parse an ``ls -l`` timestamp triple into epoch seconds.

        Returns 0.0 if the input cannot be interpreted (best-effort).
        """
        from datetime import datetime

        now = datetime.now()
        try:
            if ":" in time_or_year:
                # Recent file: year is omitted, assume current year but
                # roll back if the result lies in the future.
                dt = datetime.strptime(
                    f"{now.year} {month} {day} {time_or_year}", "%Y %b %d %H:%M"
                )
                if dt > now:
                    dt = dt.replace(year=now.year - 1)
            else:
                dt = datetime.strptime(f"{time_or_year} {month} {day}", "%Y %b %d")
            return dt.timestamp()
        except ValueError:
            return 0.0

    async def test_connection(self) -> Dict[str, Any]:
        # Use sftp instead of `ssh ... echo` because shell-restricted
        # SSH endpoints (Hetzner Storage Box, rssh, etc.) reject
        # arbitrary commands with "Command not found." but still serve
        # the SFTP subsystem just fine. `sftp -b -` reads a batch script
        # from stdin; a single `bye` exits cleanly after the auth +
        # subsystem handshake, which is exactly what we want to verify.
        try:
            cmd, env = self._build_sftp_command()

            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env or None,
            )
            stdout, stderr = await process.communicate(input=b"bye\n")

            if process.returncode == 0:
                return {"success": True, "message": "SSH/SFTP connection successful"}
            else:
                details = stderr.decode().strip() or stdout.decode().strip()
                message = details or (
                    f"SSH/SFTP connection failed (exit code {process.returncode})"
                )
                logger.warning("SSH storage test failed: %s", message)
                return {"success": False, "message": message}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("SSH storage test failed: %s", message)
            return {"success": False, "message": message}


class WebDAVStorage(StorageBackend):
    """WebDAV storage"""

    # Chunk size for streaming downloads (64 KB)
    DOWNLOAD_CHUNK_SIZE = 64 * 1024

    @property
    def MAX_RETRIES(self) -> int:  # noqa: N802 — keep public name for tests
        return max(1, settings.STORAGE_MAX_RETRIES)

    def _get_session(self, timeout_seconds: int = 300) -> aiohttp.ClientSession:
        """Create aiohttp session with auth"""
        auth = None
        if self.config.username and self.config.password:
            auth = aiohttp.BasicAuth(self.config.username, self.config.password)
        timeout = aiohttp.ClientTimeout(total=timeout_seconds, connect=30)
        return aiohttp.ClientSession(auth=auth, timeout=timeout)

    def _get_url(self, remote_path: str) -> str:
        """Build full WebDAV URL"""
        base = self.config.webdav_url.rstrip("/")
        path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
        return f"{base}{path}"

    async def _verify_remote_size(
        self, session: aiohttp.ClientSession, remote_path: str, expected: int
    ) -> bool:
        """Best-effort post-upload verification via HEAD Content-Length.

        Returns True if size matches OR if the server doesn't expose it
        (we don't want to fail uploads on servers without Content-Length).
        """
        try:
            url = self._get_url(remote_path)
            async with session.head(url) as resp:
                if resp.status not in (200, 204):
                    logger.warning(
                        "WebDAV verify HEAD %s for %s", resp.status, remote_path
                    )
                    return False
                cl = resp.headers.get("Content-Length")
                if cl is None:
                    return True
                actual = int(cl)
                if actual != expected:
                    logger.error(
                        "WebDAV size mismatch for %s: expected %d, remote %d",
                        remote_path,
                        expected,
                        actual,
                    )
                    return False
                return True
        except Exception as e:  # noqa: BLE001 — verification must not crash upload
            logger.warning("WebDAV verify error for %s: %s", remote_path, e)
            return True

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        file_size = local_path.stat().st_size
        # Scale timeout: minimum 300s, or 60s per 100 MB
        dynamic_timeout = max(300, int(file_size / (100 * 1024 * 1024)) * 60 + 120)

        # HTTP status codes that are permanent failures (no point retrying)
        NON_RETRYABLE = {400, 401, 403, 405, 409, 413, 422, 507}

        # Threshold below which we read the whole file into memory.
        # Avoids a known race in aiohttp where a retried request that
        # passes an async generator as ``data`` can raise
        # ``RuntimeError('anext(): asynchronous generator is already running')``
        # — small ``.key`` sidecars hit this every single time.
        SMALL_FILE_THRESHOLD = 4 * 1024 * 1024  # 4 MiB

        max_retries = self.MAX_RETRIES
        last_error: Exception | None = None
        for attempt in range(1, max_retries + 1):
            file_handle = None  # ensure we can close it on exception
            try:
                async with self._get_session(
                    timeout_seconds=dynamic_timeout
                ) as session:
                    parent_path = os.path.dirname(remote_path)
                    if parent_path:
                        await self._create_dirs(session, parent_path)

                    url = self._get_url(remote_path)

                    # Always send an explicit Content-Length so aiohttp
                    # cannot fall back to chunked transfer encoding (which
                    # some WebDAV proxies — e.g. Hetzner Storage Box's
                    # nginx — reject with HTTP 413).
                    headers = {
                        "Content-Length": str(file_size),
                        "Content-Type": "application/octet-stream",
                    }

                    if file_size <= SMALL_FILE_THRESHOLD:
                        # Read entirely into memory. Safe for tiny files
                        # (sidecar keys are ~200 B) and avoids the async
                        # generator retry race entirely.
                        body = local_path.read_bytes()
                    else:
                        # Stream large files via a sync file handle; aiohttp
                        # reads it in a thread executor without buffering
                        # the entire content. Avoids the async-generator
                        # ``anext()`` reentrancy bug seen on retries.
                        file_handle = open(local_path, "rb")
                        body = file_handle

                    async with session.put(
                        url,
                        data=body,
                        headers=headers,
                    ) as resp:
                        if resp.status in (200, 201, 204):
                            if settings.STORAGE_VERIFY_AFTER_UPLOAD:
                                if not await self._verify_remote_size(
                                    session, remote_path, file_size
                                ):
                                    last_error = RuntimeError(
                                        "Remote size mismatch after upload"
                                    )
                                    # Mismatch → retry (could be transient).
                                    raise last_error
                            logger.info(
                                "WebDAV upload successful: %s (%.1f MB, attempt %d)",
                                remote_path,
                                file_size / (1024 * 1024),
                                attempt,
                            )
                            return True
                        else:
                            body = await resp.text()
                            last_error = RuntimeError(
                                f"HTTP {resp.status}: {body[:200]}"
                            )
                            logger.warning(
                                "WebDAV upload HTTP %d for %s (attempt %d/%d): %s",
                                resp.status,
                                remote_path,
                                attempt,
                                max_retries,
                                body[:200],
                            )
                            if resp.status in NON_RETRYABLE:
                                if resp.status == 413:
                                    logger.error(
                                        "WebDAV upload permanently rejected (HTTP 413) "
                                        "for %s — file is %.1f GiB; the WebDAV server "
                                        "(typically Hetzner Storage Box / nginx) refuses "
                                        "single PUTs of this size. Use SFTP/SSH for "
                                        "this target, or split the backup.",
                                        remote_path,
                                        file_size / (1024 * 1024 * 1024),
                                    )
                                else:
                                    logger.error(
                                        "WebDAV upload permanently rejected (HTTP %d) for %s — not retrying",
                                        resp.status,
                                        remote_path,
                                    )
                                return False
            except Exception as e:
                last_error = e
                logger.warning(
                    "WebDAV upload error for %s (attempt %d/%d): %s",
                    remote_path,
                    attempt,
                    max_retries,
                    e,
                )
            finally:
                if file_handle is not None:
                    try:
                        file_handle.close()
                    except Exception:  # noqa: BLE001 — best-effort cleanup
                        pass

            if attempt < max_retries:
                wait = _retry_wait_seconds(attempt)
                logger.info("Retrying WebDAV upload in %.1fs...", wait)
                await asyncio.sleep(wait)

        logger.error(
            "WebDAV upload failed after %d attempts for %s: %s",
            max_retries,
            remote_path,
            last_error,
        )
        return False

    async def _create_dirs(self, session: aiohttp.ClientSession, path: str):
        """Create directories recursively via MKCOL"""
        parts = path.split("/")
        current = ""
        for part in parts:
            if part:
                current = f"{current}/{part}"
                url = self._get_url(current)
                async with session.request("MKCOL", url):
                    pass  # Ignore errors (directory might exist)

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            local_path.parent.mkdir(parents=True, exist_ok=True)

            async with self._get_session(timeout_seconds=600) as session:
                url = self._get_url(remote_path)
                async with session.get(url) as resp:
                    if resp.status == 200:
                        async with aiofiles.open(local_path, "wb") as f:
                            async for chunk in resp.content.iter_chunked(
                                self.DOWNLOAD_CHUNK_SIZE
                            ):
                                await f.write(chunk)
                        return True
                    return False
        except Exception as e:
            logger.error(f"WebDAV download error: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            async with self._get_session() as session:
                url = self._get_url(remote_path)
                async with session.delete(url) as resp:
                    return resp.status in (200, 204, 404)
        except Exception as e:
            logger.error(f"WebDAV delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            import xml.etree.ElementTree as ET
            from email.utils import parsedate_to_datetime

            async with self._get_session() as session:
                url = self._get_url(remote_path)
                # Ensure trailing slash so the server treats it as a collection
                if not url.endswith("/"):
                    url += "/"
                headers = {"Depth": "1", "Content-Type": "application/xml"}
                body = (
                    '<?xml version="1.0" encoding="utf-8"?>'
                    '<d:propfind xmlns:d="DAV:">'
                    "<d:prop>"
                    "<d:getcontentlength/>"
                    "<d:resourcetype/>"
                    "<d:getlastmodified/>"
                    "</d:prop>"
                    "</d:propfind>"
                )

                async with session.request(
                    "PROPFIND", url, data=body, headers=headers
                ) as resp:
                    if resp.status != 207:
                        logger.warning(
                            "WebDAV PROPFIND returned %s for %s", resp.status, url
                        )
                        return []

                    text = await resp.text()

            # Parse the Multi-Status XML response
            ns = {"d": "DAV:"}
            root = ET.fromstring(text)
            files: List[Dict[str, Any]] = []

            # Build the "self" href so we can skip the collection itself
            request_path = url.split("://", 1)[-1].split("/", 1)[-1].rstrip("/")

            for response_el in root.findall("d:response", ns):
                href_el = response_el.find("d:href", ns)
                if href_el is None or href_el.text is None:
                    continue

                href = href_el.text.rstrip("/")
                # Skip the collection entry itself (first entry is the queried dir)
                href_decoded = href.rstrip("/")
                if href_decoded == request_path or href_decoded == "/" + request_path:
                    continue

                # Extract the file/directory name from the href
                name = href.rsplit("/", 1)[-1]
                if not name:
                    continue

                # URL-decode the name
                from urllib.parse import unquote

                name = unquote(name)

                propstat = response_el.find("d:propstat", ns)
                if propstat is None:
                    continue
                prop = propstat.find("d:prop", ns)
                if prop is None:
                    continue

                # Check if directory
                resource_type = prop.find("d:resourcetype", ns)
                is_dir = (
                    resource_type is not None
                    and resource_type.find("d:collection", ns) is not None
                )

                # Get size
                size_el = prop.find("d:getcontentlength", ns)
                size = 0
                if size_el is not None and size_el.text:
                    try:
                        size = int(size_el.text)
                    except ValueError:
                        size = 0

                # Get modified date as Unix timestamp
                modified_el = prop.find("d:getlastmodified", ns)
                modified: float = 0
                if modified_el is not None and modified_el.text:
                    try:
                        dt = parsedate_to_datetime(modified_el.text)
                        modified = dt.timestamp()
                    except (ValueError, TypeError):
                        modified = 0

                files.append(
                    {
                        "name": name,
                        "size": size,
                        "is_dir": is_dir,
                        "modified": modified,
                    }
                )

            return files
        except Exception as e:
            logger.error(f"WebDAV list error: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        try:
            async with self._get_session() as session:
                url = self._get_url("")
                async with session.request(
                    "PROPFIND", url, headers={"Depth": "0"}
                ) as resp:
                    if resp.status in (200, 207):
                        return {
                            "success": True,
                            "message": "WebDAV connection successful",
                        }
                    else:
                        details = (await resp.text()).strip()
                        if len(details) > 200:
                            details = f"{details[:200]}…"
                        hint = (
                            " Check WebDAV URL and base_path."
                            if resp.status == 404
                            else ""
                        )
                        message = (
                            f"HTTP {resp.status}: {details}{hint}"
                            if details
                            else f"HTTP {resp.status}.{hint}"
                        )
                        logger.warning("WebDAV storage test failed: %s", message)
                        return {"success": False, "message": message}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("WebDAV storage test failed: %s", message)
            return {"success": False, "message": message}


class S3Storage(StorageBackend):
    """S3-compatible storage (AWS, MinIO, Backblaze B2, etc.)"""

    def __init__(self, config: StorageConfig):
        super().__init__(config)
        self._client = None

    async def _get_client(self):
        """Get or create S3 client"""
        if self._client is None:
            try:
                import aioboto3
                from botocore.config import Config as BotoConfig

                session = aioboto3.Session()

                endpoint_url = self.config.s3_endpoint_url
                if not endpoint_url and self.config.host:
                    endpoint_url = f"https://{self.config.host}"

                boto_config = BotoConfig(
                    connect_timeout=30,
                    read_timeout=300,
                    retries={"max_attempts": 3},
                )

                self._client = await session.client(
                    "s3",
                    region_name=self.config.s3_region or "us-east-1",
                    aws_access_key_id=self.config.s3_access_key,
                    aws_secret_access_key=self.config.s3_secret_key,
                    endpoint_url=endpoint_url,
                    config=boto_config,
                ).__aenter__()
            except ImportError:
                logger.error("aioboto3 not installed. Run: pip install aioboto3")
                raise
        return self._client

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            client = await self._get_client()
            key = f"{self.config.base_path}/{remote_path}".lstrip("/")
            file_size = local_path.stat().st_size
            bucket = self.config.s3_bucket
            multipart_threshold = max(
                5 * 1024 * 1024, settings.S3_MULTIPART_THRESHOLD_BYTES
            )
            chunk_size = max(5 * 1024 * 1024, settings.S3_MULTIPART_CHUNK_SIZE)

            if file_size < multipart_threshold:
                # Small file: stream the body via an aiofiles handle so the
                # full payload is never materialised in memory.
                async with aiofiles.open(local_path, "rb") as f:
                    await client.put_object(Bucket=bucket, Key=key, Body=f)
                logger.info(
                    "S3 upload successful: %s (%.1f MB)",
                    key,
                    file_size / (1024 * 1024),
                )
                return True

            # Large file: explicit multipart upload, streaming each part
            # from disk. Aborts the upload on any failure so we don't leak
            # incomplete multipart uploads (and storage costs).
            create = await client.create_multipart_upload(Bucket=bucket, Key=key)
            upload_id = create["UploadId"]
            parts: list[dict] = []
            try:
                async with aiofiles.open(local_path, "rb") as f:
                    part_number = 1
                    while True:
                        data = await f.read(chunk_size)
                        if not data:
                            break
                        result = await client.upload_part(
                            Bucket=bucket,
                            Key=key,
                            PartNumber=part_number,
                            UploadId=upload_id,
                            Body=data,
                        )
                        parts.append(
                            {"ETag": result["ETag"], "PartNumber": part_number}
                        )
                        part_number += 1
                await client.complete_multipart_upload(
                    Bucket=bucket,
                    Key=key,
                    UploadId=upload_id,
                    MultipartUpload={"Parts": parts},
                )
            except Exception:
                try:
                    await client.abort_multipart_upload(
                        Bucket=bucket, Key=key, UploadId=upload_id
                    )
                except Exception as abort_exc:  # noqa: BLE001
                    logger.warning(
                        "S3 abort_multipart_upload failed for %s: %s",
                        key,
                        abort_exc,
                    )
                raise

            logger.info(
                "S3 multipart upload successful: %s (%.1f MB, %d parts)",
                key,
                file_size / (1024 * 1024),
                len(parts),
            )
            return True
        except Exception as e:
            logger.error(f"S3 upload error: {e}")
            return False

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            client = await self._get_client()
            key = f"{self.config.base_path}/{remote_path}".lstrip("/")

            local_path.parent.mkdir(parents=True, exist_ok=True)

            response = await client.get_object(Bucket=self.config.s3_bucket, Key=key)
            async with aiofiles.open(local_path, "wb") as f:
                await f.write(await response["Body"].read())
            return True
        except Exception as e:
            logger.error(f"S3 download error: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            client = await self._get_client()
            key = f"{self.config.base_path}/{remote_path}".lstrip("/")

            await client.delete_object(Bucket=self.config.s3_bucket, Key=key)
            return True
        except Exception as e:
            logger.error(f"S3 delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            client = await self._get_client()
            prefix = f"{self.config.base_path}/{remote_path}".lstrip("/")

            response = await client.list_objects_v2(
                Bucket=self.config.s3_bucket, Prefix=prefix
            )

            files = []
            for obj in response.get("Contents", []):
                modified = 0
                if obj.get("LastModified"):
                    try:
                        modified = obj["LastModified"].timestamp()
                    except (AttributeError, TypeError):
                        modified = 0
                files.append(
                    {
                        "name": obj["Key"].split("/")[-1],
                        "size": obj["Size"],
                        "is_dir": False,
                        "modified": modified,
                    }
                )
            return files
        except Exception as e:
            logger.error(f"S3 list error: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        try:
            client = await self._get_client()
            await client.head_bucket(Bucket=self.config.s3_bucket)
            return {
                "success": True,
                "message": f"S3 bucket '{self.config.s3_bucket}' accessible",
            }
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("S3 storage test failed: %s", message)
            return {"success": False, "message": message}


class RcloneStorage(StorageBackend):
    """Rclone storage - supports 40+ cloud providers"""

    async def _run_rclone(self, args: List[str]) -> tuple[int, str, str]:
        """Run rclone command"""
        cmd = ["rclone", "--timeout", "300s", "--contimeout", "30s"] + args

        process = await asyncio.create_subprocess_exec(
            *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
        )
        stdout, stderr = await process.communicate()
        return process.returncode, stdout.decode(), stderr.decode()

    def _get_remote_path(self, remote_path: str) -> str:
        """Build rclone remote path"""
        path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
        return f"{self.config.rclone_remote}:{path}"

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            dest = self._get_remote_path(remote_path)

            # Create parent directory
            parent = os.path.dirname(dest)
            await self._run_rclone(["mkdir", parent])

            # Copy file
            code, stdout, stderr = await self._run_rclone(
                ["copyto", str(local_path), dest, "--progress"]
            )

            if code == 0:
                logger.info(f"Rclone upload successful: {remote_path}")
                return True
            else:
                logger.error(f"Rclone upload failed: {stderr}")
                return False
        except Exception as e:
            logger.error(f"Rclone upload error: {e}")
            return False

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            local_path.parent.mkdir(parents=True, exist_ok=True)
            src = self._get_remote_path(remote_path)

            code, stdout, stderr = await self._run_rclone(
                ["copyto", src, str(local_path), "--progress"]
            )
            return code == 0
        except Exception as e:
            logger.error(f"Rclone download error: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            path = self._get_remote_path(remote_path)
            code, _, _ = await self._run_rclone(["deletefile", path])
            return code == 0
        except Exception as e:
            logger.error(f"Rclone delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            path = self._get_remote_path(remote_path)
            code, stdout, stderr = await self._run_rclone(["lsjson", path])

            if code == 0:
                import json
                from datetime import datetime as _dt

                items = json.loads(stdout)
                result = []
                for item in items:
                    modified = 0
                    mod_time = item.get("ModTime", "")
                    if mod_time:
                        try:
                            dt = _dt.fromisoformat(mod_time.replace("Z", "+00:00"))
                            modified = dt.timestamp()
                        except (ValueError, TypeError):
                            modified = 0
                    result.append(
                        {
                            "name": item["Name"],
                            "size": item.get("Size", 0),
                            "is_dir": item["IsDir"],
                            "modified": modified,
                        }
                    )
                return result
            return []
        except Exception as e:
            logger.error(f"Rclone list error: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        try:
            code, stdout, stderr = await self._run_rclone(
                ["lsd", f"{self.config.rclone_remote}:"]
            )
            if code == 0:
                return {
                    "success": True,
                    "message": (
                        f"Rclone remote '{self.config.rclone_remote}' accessible"
                    ),
                }
            else:
                details = stderr.strip() or stdout.strip()
                message = details or f"Rclone connection failed (exit code {code})"
                logger.warning("Rclone storage test failed: %s", message)
                return {"success": False, "message": message}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("Rclone storage test failed: %s", message)
            return {"success": False, "message": message}


class FTPStorage(StorageBackend):
    """FTP/FTPS storage"""

    async def _connect(self):
        """Create FTP connection"""
        import aioftp

        client = aioftp.Client()
        await client.connect(self.config.host, port=self.config.port or 21)
        # Set socket timeout for data operations
        client.socket.settimeout(60)
        if self.config.username:
            await client.login(self.config.username, self.config.password or "")
        return client

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            client = await self._connect()

            try:
                full_path = f"{self.config.base_path}/{remote_path}"

                # Create directories
                parent = os.path.dirname(full_path)
                try:
                    await client.make_directory(parent)
                except Exception:
                    pass

                # Upload file
                await client.upload(local_path, full_path)
                logger.info(f"FTP upload successful: {remote_path}")
                return True
            finally:
                await client.quit()
        except Exception as e:
            logger.error(f"FTP upload error: {e}")
            return False

    async def download(self, remote_path: str, local_path: Path) -> bool:
        try:
            client = await self._connect()

            try:
                local_path.parent.mkdir(parents=True, exist_ok=True)
                full_path = f"{self.config.base_path}/{remote_path}"
                await client.download(full_path, local_path)
                return True
            finally:
                await client.quit()
        except Exception as e:
            logger.error(f"FTP download error: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            client = await self._connect()

            try:
                full_path = f"{self.config.base_path}/{remote_path}"
                await client.remove_file(full_path)
                return True
            finally:
                await client.quit()
        except Exception as e:
            logger.error(f"FTP delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            client = await self._connect()

            try:
                from datetime import datetime as _dt

                full_path = f"{self.config.base_path}/{remote_path}"
                files = []
                async for path, info in client.list(full_path):
                    modified = 0
                    modify_str = info.get("modify", "")
                    if modify_str:
                        try:
                            # aioftp MDTM format: YYYYMMDDHHMMSS[.sss]
                            dt = _dt.strptime(str(modify_str)[:14], "%Y%m%d%H%M%S")
                            modified = dt.timestamp()
                        except (ValueError, TypeError):
                            modified = 0
                    size = info.get("size", 0)
                    try:
                        size = int(size)
                    except (ValueError, TypeError):
                        size = 0
                    files.append(
                        {
                            "name": path.name,
                            "size": size,
                            "is_dir": info.get("type") == "dir",
                            "modified": modified,
                        }
                    )
                return files
            finally:
                await client.quit()
        except Exception as e:
            logger.error(f"FTP list error: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        try:
            client = await self._connect()
            await client.quit()
            return {"success": True, "message": "FTP connection successful"}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("FTP storage test failed: %s", message)
            return {"success": False, "message": message}


class RemoteStorageManager:
    """Manager for handling multiple remote storage backends"""

    _backends = {
        StorageType.LOCAL: LocalStorage,
        StorageType.SSH: SSHStorage,
        StorageType.SFTP: SSHStorage,  # Same implementation
        StorageType.WEBDAV: WebDAVStorage,
        StorageType.S3: S3Storage,
        StorageType.FTP: FTPStorage,
        StorageType.RCLONE: RcloneStorage,
    }

    def __init__(self):
        self.configs: Dict[int, StorageConfig] = {}
        self.backends: Dict[int, StorageBackend] = {}

    def add_storage(self, config: StorageConfig) -> StorageBackend:
        """Add a storage backend"""
        backend_class = self._backends.get(config.storage_type)
        if not backend_class:
            raise ValueError(f"Unknown storage type: {config.storage_type}")

        backend = backend_class(config)
        self.configs[config.id] = config
        self.backends[config.id] = backend
        return backend

    def get_backend(self, storage_id: int) -> Optional[StorageBackend]:
        """Get a storage backend by ID"""
        return self.backends.get(storage_id)

    def remove_storage(self, storage_id: int):
        """Remove a storage backend"""
        self.configs.pop(storage_id, None)
        self.backends.pop(storage_id, None)

    async def upload_to_all(
        self,
        local_path: Path,
        remote_path: str,
        storage_ids: Optional[List[int]] = None,
    ) -> Dict[int, bool]:
        """Upload a file to multiple storage backends"""
        results = {}

        targets = storage_ids or list(self.backends.keys())

        tasks = []
        for storage_id in targets:
            backend = self.backends.get(storage_id)
            if backend and self.configs[storage_id].enabled:
                tasks.append((storage_id, backend.upload(local_path, remote_path)))

        for storage_id, task in tasks:
            try:
                results[storage_id] = await task
            except Exception as e:
                logger.error(f"Upload to storage {storage_id} failed: {e}")
                results[storage_id] = False

        return results

    async def sync_backup(
        self,
        local_backup_path: Path,
        target_name: str,
        backup_filename: str,
        storage_ids: Optional[List[int]] = None,
    ) -> Dict[int, bool]:
        """Sync a backup file to remote storage(s)"""
        remote_path = f"{target_name}/{backup_filename}"
        return await self.upload_to_all(local_backup_path, remote_path, storage_ids)


# Global instance
storage_manager = RemoteStorageManager()
