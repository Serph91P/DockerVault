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
import shlex
import shutil
from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Dict, List, Optional

import aiofiles
import aiohttp

logger = logging.getLogger(__name__)


def _format_exception_message(exc: Exception) -> str:
    message = str(exc).strip()
    name = exc.__class__.__name__
    return f"{name}: {message}" if message else name


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

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            dest = Path(self.config.base_path) / remote_path
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
            src = Path(self.config.base_path) / remote_path
            local_path.parent.mkdir(parents=True, exist_ok=True)

            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, shutil.copy2, str(src), str(local_path))

            return True
        except Exception as e:
            logger.error(f"Local download failed: {e}")
            return False

    async def delete(self, remote_path: str) -> bool:
        try:
            path = Path(self.config.base_path) / remote_path
            if path.exists():
                path.unlink()
            return True
        except Exception as e:
            logger.error(f"Local delete failed: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            path = Path(self.config.base_path) / remote_path
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

    def _get_ssh_options(self) -> List[str]:
        """Build SSH options for commands"""
        opts = []
        if self.config.port:
            opts.extend(["-e", f"ssh -p {self.config.port}"])
        if self.config.ssh_key_path:
            opts.extend(["-e", f"ssh -i {self.config.ssh_key_path}"])
        return opts

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
            # Create remote directory first
            remote_dir = os.path.dirname(f"{self.config.base_path}/{remote_path}")
            # Sanitize the path to prevent command injection
            safe_dir = shlex.quote(remote_dir)
            mkdir_cmd = self._build_ssh_command(f"mkdir -p {safe_dir}")

            process = await asyncio.create_subprocess_exec(
                *mkdir_cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            await process.communicate()

            # Use rsync for efficient transfer
            cmd = ["rsync", "-avz", "--progress"]
            cmd.extend(self._get_ssh_options())
            cmd.append(str(local_path))
            cmd.append(self._get_remote_path(remote_path))

            process = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
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

            process = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
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
        cmd = ["ssh"]
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
        try:
            full_path = f"{self.config.base_path}/{remote_path}"
            # Sanitize the path to prevent command injection
            safe_path = shlex.quote(full_path)
            cmd = self._build_ssh_command(f"rm -f {safe_path}")

            process = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            await process.communicate()
            return process.returncode == 0
        except Exception as e:
            logger.error(f"SSH delete error: {e}")
            return False

    async def list_files(self, remote_path: str = "") -> List[Dict[str, Any]]:
        try:
            full_path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
            # Sanitize the path to prevent command injection
            safe_path = shlex.quote(full_path)
            cmd = self._build_ssh_command(f"ls -la {safe_path}")

            process = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            stdout, stderr = await process.communicate()

            files = []
            for line in stdout.decode().split("\n")[1:]:  # Skip total line
                parts = line.split()
                if len(parts) >= 9:
                    files.append(
                        {
                            "name": parts[-1],
                            "size": int(parts[4]) if parts[4].isdigit() else 0,
                            "is_dir": parts[0].startswith("d"),
                            "permissions": parts[0],
                        }
                    )
            return files
        except Exception as e:
            logger.error(f"SSH list error: {e}")
            return []

    async def test_connection(self) -> Dict[str, Any]:
        try:
            cmd = self._build_ssh_command("echo 'Connection successful'")

            process = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            stdout, stderr = await process.communicate()

            if process.returncode == 0:
                return {"success": True, "message": "SSH connection successful"}
            else:
                details = stderr.decode().strip() or stdout.decode().strip()
                message = details or (
                    f"SSH connection failed (exit code {process.returncode})"
                )
                logger.warning("SSH storage test failed: %s", message)
                return {"success": False, "message": message}
        except Exception as e:
            message = _format_exception_message(e)
            logger.warning("SSH storage test failed: %s", message)
            return {"success": False, "message": message}


class WebDAVStorage(StorageBackend):
    """WebDAV storage"""

    def _get_session(self) -> aiohttp.ClientSession:
        """Create aiohttp session with auth"""
        auth = None
        if self.config.username and self.config.password:
            auth = aiohttp.BasicAuth(self.config.username, self.config.password)
        return aiohttp.ClientSession(auth=auth)

    def _get_url(self, remote_path: str) -> str:
        """Build full WebDAV URL"""
        base = self.config.webdav_url.rstrip("/")
        path = f"{self.config.base_path}/{remote_path}".replace("//", "/")
        return f"{base}{path}"

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            async with self._get_session() as session:
                # Create parent directories (MKCOL)
                parent_path = os.path.dirname(remote_path)
                if parent_path:
                    await self._create_dirs(session, parent_path)

                url = self._get_url(remote_path)
                async with aiofiles.open(local_path, "rb") as f:
                    data = await f.read()

                async with session.put(url, data=data) as resp:
                    if resp.status in (200, 201, 204):
                        logger.info(f"WebDAV upload successful: {remote_path}")
                        return True
                    else:
                        logger.error(f"WebDAV upload failed: {resp.status}")
                        return False
        except Exception as e:
            logger.error(f"WebDAV upload error: {e}")
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

            async with self._get_session() as session:
                url = self._get_url(remote_path)
                async with session.get(url) as resp:
                    if resp.status == 200:
                        async with aiofiles.open(local_path, "wb") as f:
                            await f.write(await resp.read())
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
            async with self._get_session() as session:
                url = self._get_url(remote_path)
                headers = {"Depth": "1"}
                body = """<?xml version="1.0"?>
                <d:propfind xmlns:d="DAV:">
                    <d:prop>
                        <d:getcontentlength/>
                        <d:resourcetype/>
                        <d:getlastmodified/>
                    </d:prop>
                </d:propfind>"""

                async with session.request(
                    "PROPFIND", url, data=body, headers=headers
                ) as resp:
                    if resp.status == 207:
                        # Parse XML response (simplified)
                        _ = await resp.text()
                        # Would need proper XML parsing here
                        return []
                    return []
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
                        message = (
                            f"HTTP {resp.status}: {details}"
                            if details
                            else f"HTTP {resp.status}"
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

                session = aioboto3.Session()

                endpoint_url = self.config.s3_endpoint_url
                if not endpoint_url and self.config.host:
                    endpoint_url = f"https://{self.config.host}"

                self._client = await session.client(
                    "s3",
                    region_name=self.config.s3_region or "us-east-1",
                    aws_access_key_id=self.config.s3_access_key,
                    aws_secret_access_key=self.config.s3_secret_key,
                    endpoint_url=endpoint_url,
                ).__aenter__()
            except ImportError:
                logger.error("aioboto3 not installed. Run: pip install aioboto3")
                raise
        return self._client

    async def upload(self, local_path: Path, remote_path: str) -> bool:
        try:
            client = await self._get_client()
            key = f"{self.config.base_path}/{remote_path}".lstrip("/")

            async with aiofiles.open(local_path, "rb") as f:
                data = await f.read()

            await client.put_object(Bucket=self.config.s3_bucket, Key=key, Body=data)
            logger.info(f"S3 upload successful: {key}")
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
                files.append(
                    {
                        "name": obj["Key"].split("/")[-1],
                        "size": obj["Size"],
                        "is_dir": False,
                        "modified": obj["LastModified"].isoformat(),
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
        cmd = ["rclone"] + args

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

                items = json.loads(stdout)
                return [
                    {
                        "name": item["Name"],
                        "size": item.get("Size", 0),
                        "is_dir": item["IsDir"],
                        "modified": item.get("ModTime", ""),
                    }
                    for item in items
                ]
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
                full_path = f"{self.config.base_path}/{remote_path}"
                files = []
                async for path, info in client.list(full_path):
                    files.append(
                        {
                            "name": path.name,
                            "size": info.get("size", 0),
                            "is_dir": info.get("type") == "dir",
                            "modified": info.get("modify", ""),
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
