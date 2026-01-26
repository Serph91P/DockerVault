"""
Backup engine - handles the actual backup process.
"""

import asyncio
import hashlib
import logging
import os
import shlex
import shutil
import tarfile
import time
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from sqlalchemy import select, update

from app.config import settings
from app.database import Backup, BackupStatus, BackupTarget, BackupType, async_session
from app.docker_client import docker_client

logger = logging.getLogger(__name__)


@dataclass
class BackupMetrics:
    """Metrics for backup operations."""

    total_backups: int = 0
    successful_backups: int = 0
    failed_backups: int = 0
    total_bytes_backed_up: int = 0
    target_durations: Dict[int, List[float]] = field(
        default_factory=lambda: defaultdict(list)
    )
    target_sizes: Dict[int, List[int]] = field(
        default_factory=lambda: defaultdict(list)
    )
    last_backup_time: Optional[datetime] = None

    @property
    def success_rate(self) -> float:
        """Calculate overall success rate."""
        if self.total_backups == 0:
            return 0.0
        return (self.successful_backups / self.total_backups) * 100

    def record_backup(
        self,
        target_id: int,
        duration: float,
        size: int,
        success: bool,
    ):
        """Record metrics for a completed backup."""
        self.total_backups += 1
        if success:
            self.successful_backups += 1
            self.total_bytes_backed_up += size
        else:
            self.failed_backups += 1

        self.target_durations[target_id].append(duration)
        self.target_sizes[target_id].append(size)
        self.last_backup_time = datetime.utcnow()

        # Keep only last 100 entries per target to limit memory
        if len(self.target_durations[target_id]) > 100:
            self.target_durations[target_id] = self.target_durations[target_id][-100:]
        if len(self.target_sizes[target_id]) > 100:
            self.target_sizes[target_id] = self.target_sizes[target_id][-100:]

    def get_average_duration(self, target_id: int) -> Optional[float]:
        """Get average backup duration for a target."""
        durations = self.target_durations.get(target_id, [])
        if not durations:
            return None
        return sum(durations) / len(durations)

    def get_average_size(self, target_id: int) -> Optional[int]:
        """Get average backup size for a target."""
        sizes = self.target_sizes.get(target_id, [])
        if not sizes:
            return None
        return int(sum(sizes) / len(sizes))

    def to_dict(self) -> Dict[str, Any]:
        """Convert metrics to dictionary for API response."""
        return {
            "total_backups": self.total_backups,
            "successful_backups": self.successful_backups,
            "failed_backups": self.failed_backups,
            "success_rate": round(self.success_rate, 2),
            "total_bytes_backed_up": self.total_bytes_backed_up,
            "last_backup_time": (
                self.last_backup_time.isoformat() if self.last_backup_time else None
            ),
        }


class BackupEngine:
    """Handles backup operations."""

    def __init__(self):
        self.active_backups: Dict[int, asyncio.Task] = {}
        self.progress_callbacks: List[Callable] = []
        self.metrics = BackupMetrics()
        self._backup_semaphore = asyncio.Semaphore(2)  # Max concurrent backups

    def add_progress_callback(self, callback: Callable):
        """Add a callback for progress updates."""
        self.progress_callbacks.append(callback)

    def remove_progress_callback(self, callback: Callable):
        """Remove a progress callback."""
        if callback in self.progress_callbacks:
            self.progress_callbacks.remove(callback)

    async def _notify_progress(self, backup_id: int, progress: float, message: str):
        """Notify all callbacks of progress update."""
        for callback in self.progress_callbacks:
            try:
                await callback(backup_id, progress, message)
            except Exception as e:
                logger.error(f"Progress callback error: {e}")

    async def validate_backup_prerequisites(
        self,
        target: BackupTarget,
    ) -> List[str]:
        """Validate backup prerequisites and return any issues.

        Checks:
        - Container/volume/path existence
        - Available disk space
        - Permission issues

        Args:
            target: The backup target to validate

        Returns:
            List of issue descriptions, empty if validation passes
        """
        issues = []

        if target.target_type == "container" and target.container_name:
            containers = await docker_client.list_containers()
            if not any(c.name == target.container_name for c in containers):
                issues.append(f"Container '{target.container_name}' not found")
                logger.warning(
                    "Validation failed: container not found",
                    extra={
                        "target_id": target.id,
                        "target_name": target.name,
                        "container_name": target.container_name,
                    },
                )

        elif target.target_type == "volume" and target.volume_name:
            volumes = await docker_client.list_volumes()
            if not any(v.name == target.volume_name for v in volumes):
                issues.append(f"Volume '{target.volume_name}' not found")
                logger.warning(
                    "Validation failed: volume not found",
                    extra={
                        "target_id": target.id,
                        "target_name": target.name,
                        "volume_name": target.volume_name,
                    },
                )

        elif target.target_type == "path" and target.host_path:
            if not os.path.exists(target.host_path):
                issues.append(f"Path '{target.host_path}' not found")
                logger.warning(
                    "Validation failed: path not found",
                    extra={
                        "target_id": target.id,
                        "target_name": target.name,
                        "host_path": target.host_path,
                    },
                )
            elif not os.access(target.host_path, os.R_OK):
                issues.append(f"Path '{target.host_path}' is not readable")
                logger.warning(
                    "Validation failed: path not readable",
                    extra={
                        "target_id": target.id,
                        "target_name": target.name,
                        "host_path": target.host_path,
                    },
                )

        # Check available disk space in backup directory
        backup_dir = Path(settings.BACKUP_BASE_PATH)
        try:
            backup_dir.mkdir(parents=True, exist_ok=True)
            stat = shutil.disk_usage(backup_dir)
            # Warn if less than 1GB free
            min_free_bytes = 1024 * 1024 * 1024  # 1 GB
            if stat.free < min_free_bytes:
                issues.append(
                    f"Low disk space in backup directory: "
                    f"{stat.free / (1024**3):.2f} GB free"
                )
                logger.warning(
                    "Low disk space in backup directory",
                    extra={
                        "target_id": target.id,
                        "free_bytes": stat.free,
                        "min_required_bytes": min_free_bytes,
                    },
                )
        except Exception as e:
            issues.append(f"Cannot check disk space: {e}")
            logger.error(
                "Failed to check disk space",
                extra={"error": str(e), "error_type": type(e).__name__},
            )

        # Validate dependencies if specified
        if target.dependencies:
            containers = await docker_client.list_containers()
            container_names = {c.name for c in containers}
            for dep in target.dependencies:
                if dep not in container_names:
                    issues.append(f"Dependency container '{dep}' not found")
                    logger.warning(
                        "Validation failed: dependency not found",
                        extra={
                            "target_id": target.id,
                            "target_name": target.name,
                            "dependency": dep,
                        },
                    )

        return issues

    async def create_backup(
        self,
        target: BackupTarget,
        backup_type: BackupType = BackupType.FULL,
    ) -> Backup:
        """Create a new backup for the target."""
        async with async_session() as session:
            backup = Backup(
                target_id=target.id,
                backup_type=backup_type,
                status=BackupStatus.PENDING,
                backup_metadata={
                    "target_name": target.name,
                    "target_type": target.target_type,
                },
            )
            session.add(backup)
            await session.commit()
            await session.refresh(backup)
            return backup

    async def run_backup(self, backup_id: int, skip_validation: bool = False) -> bool:
        """Run a backup by ID.

        Uses a semaphore to limit concurrent backups based on
        MAX_CONCURRENT_BACKUPS setting.

        Args:
            backup_id: ID of the backup to run
            skip_validation: Skip pre-backup validation (for testing)

        Returns:
            True if backup succeeded, False otherwise
        """
        start_time = time.time()
        file_size = 0
        target_id = None

        async with self._backup_semaphore:
            async with async_session() as session:
                result = await session.execute(
                    select(Backup).where(Backup.id == backup_id)
                )
                backup = result.scalar_one_or_none()

                if not backup:
                    logger.error(
                        f"Backup {backup_id} not found",
                        extra={"backup_id": backup_id},
                    )
                    return False

                # Get target
                result = await session.execute(
                    select(BackupTarget).where(BackupTarget.id == backup.target_id)
                )
                target = result.scalar_one_or_none()

                if not target:
                    logger.error(
                        f"Target for backup {backup_id} not found",
                        extra={
                            "backup_id": backup_id,
                            "target_id": backup.target_id,
                        },
                    )
                    return False

                target_id = target.id

                # Validate prerequisites unless skipped
                if not skip_validation:
                    validation_issues = await self.validate_backup_prerequisites(target)
                    if validation_issues:
                        error_msg = f"Validation failed: {'; '.join(validation_issues)}"
                        logger.error(
                            f"Backup {backup_id} validation failed",
                            extra={
                                "backup_id": backup_id,
                                "target_id": target.id,
                                "target_name": target.name,
                                "issues": validation_issues,
                            },
                        )
                        backup.status = BackupStatus.FAILED
                        backup.error_message = error_msg
                        backup.completed_at = datetime.utcnow()
                        await session.commit()
                        await self._notify_progress(backup_id, -1, error_msg)

                        # Record failed backup metrics
                        duration = time.time() - start_time
                        self.metrics.record_backup(target_id, duration, 0, False)
                        return False

                # Update status to running
                backup.status = BackupStatus.RUNNING
                backup.started_at = datetime.utcnow()
                await session.commit()

            await self._notify_progress(backup_id, 0, "Starting backup...")

        try:
            # Determine containers to stop
            containers_to_stop = []
            original_states = {}

            if target.stop_container and target.container_name:
                containers_to_stop.append(target.container_name)

            # Add dependencies
            if target.dependencies:
                containers_to_stop.extend(target.dependencies)

            # Get dependency order for safe stopping
            if containers_to_stop:
                containers_to_stop = await docker_client.get_dependency_order(
                    containers_to_stop
                )

                # Store original states
                for container_name in containers_to_stop:
                    state = await docker_client.get_container_state(container_name)
                    original_states[container_name] = state

                # Stop containers in order
                await self._notify_progress(backup_id, 10, "Stopping containers...")
                for i, container_name in enumerate(containers_to_stop):
                    if original_states.get(container_name) == "running":
                        await docker_client.stop_container(container_name)
                        progress = 10 + (20 * (i + 1) / len(containers_to_stop))
                        await self._notify_progress(
                            backup_id, progress, f"Stopped {container_name}"
                        )

            # Run pre-backup hook
            if target.pre_backup_command:
                await self._notify_progress(backup_id, 30, "Running pre-backup hook...")
                await self._run_hook(target.pre_backup_command)

            # Perform the actual backup
            await self._notify_progress(backup_id, 35, "Creating backup archive...")

            backup_path = await self._create_backup_archive(target, backup_id)

            await self._notify_progress(backup_id, 80, "Calculating checksum...")

            # Calculate file size and checksum
            file_size = os.path.getsize(backup_path)
            checksum = await self._calculate_checksum(backup_path)

            # Run post-backup hook
            if target.post_backup_command:
                await self._notify_progress(
                    backup_id, 85, "Running post-backup hook..."
                )
                await self._run_hook(target.post_backup_command)

            # Restart containers in reverse order
            if containers_to_stop:
                await self._notify_progress(backup_id, 90, "Starting containers...")
                for container_name in reversed(containers_to_stop):
                    if original_states.get(container_name) == "running":
                        await docker_client.start_container(container_name)

            # Update backup record
            async with async_session() as session:
                duration_seconds = int(
                    (datetime.utcnow() - backup.started_at).total_seconds()
                )
                await session.execute(
                    update(Backup)
                    .where(Backup.id == backup_id)
                    .values(
                        status=BackupStatus.COMPLETED,
                        file_path=backup_path,
                        file_size=file_size,
                        checksum=checksum,
                        completed_at=datetime.utcnow(),
                        duration_seconds=duration_seconds,
                    )
                )
                await session.commit()

            # Record successful backup metrics
            duration = time.time() - start_time
            self.metrics.record_backup(target_id, duration, file_size, True)

            await self._notify_progress(backup_id, 100, "Backup completed!")
            logger.info(
                f"Backup {backup_id} completed successfully",
                extra={
                    "backup_id": backup_id,
                    "target_id": target_id,
                    "target_name": target.name,
                    "file_size": file_size,
                    "duration_seconds": duration_seconds,
                    "backup_path": backup_path,
                },
            )
            return True

        except Exception as e:
            logger.error(
                f"Backup {backup_id} failed: {e}",
                extra={
                    "backup_id": backup_id,
                    "target_id": target_id,
                    "target_name": target.name if target else None,
                    "error_type": type(e).__name__,
                    "error": str(e),
                },
            )

            # Try to restart containers on failure
            for container_name in reversed(containers_to_stop):
                if original_states.get(container_name) == "running":
                    try:
                        await docker_client.start_container(container_name)
                    except Exception as restart_error:
                        logger.error(
                            f"Failed to restart {container_name}: {restart_error}",
                            extra={
                                "container_name": container_name,
                                "error": str(restart_error),
                            },
                        )

            # Update backup record with error
            async with async_session() as session:
                await session.execute(
                    update(Backup)
                    .where(Backup.id == backup_id)
                    .values(
                        status=BackupStatus.FAILED,
                        error_message=str(e),
                        completed_at=datetime.utcnow(),
                    )
                )
                await session.commit()

            # Record failed backup metrics
            duration = time.time() - start_time
            if target_id:
                self.metrics.record_backup(target_id, duration, 0, False)

            await self._notify_progress(backup_id, -1, f"Backup failed: {e}")
            return False

    async def _create_backup_archive(
        self,
        target: BackupTarget,
        backup_id: int,
    ) -> str:
        """Create the backup archive."""
        # Determine source path
        if target.target_type == "volume":
            # Volume backup - get mountpoint
            volumes = await docker_client.list_volumes()
            volume = next((v for v in volumes if v.name == target.volume_name), None)
            if not volume:
                raise ValueError(f"Volume {target.volume_name} not found")
            source_path = volume.mountpoint
        elif target.target_type == "path":
            source_path = target.host_path
        elif target.target_type == "container":
            # Get all volume mounts from container
            containers = await docker_client.list_containers()
            container = next(
                (c for c in containers if c.name == target.container_name), None
            )
            if not container:
                raise ValueError(f"Container {target.container_name} not found")

            # Create combined backup of all volumes
            source_paths = []
            for mount in container.mounts:
                if mount.get("type") == "volume":
                    volumes = await docker_client.list_volumes()
                    volume = next(
                        (v for v in volumes if v.name == mount.get("name")), None
                    )
                    if volume:
                        source_paths.append(
                            (mount.get("destination"), volume.mountpoint)
                        )
        else:
            raise ValueError(f"Unknown target type: {target.target_type}")

        # Create backup directory
        timestamp = datetime.utcnow().strftime("%Y%m%d_%H%M%S")
        backup_dir = Path(settings.BACKUP_BASE_PATH) / target.name
        backup_dir.mkdir(parents=True, exist_ok=True)

        filename = f"{target.name}_{timestamp}.tar"
        if target.compression_enabled:
            filename += ".gz"

        backup_path = backup_dir / filename

        # Create tarball
        loop = asyncio.get_event_loop()

        if target.target_type == "container" and source_paths:
            await loop.run_in_executor(
                None,
                lambda: self._create_multi_source_tar(
                    source_paths,
                    str(backup_path),
                    target.compression_enabled,
                ),
            )
        else:
            await loop.run_in_executor(
                None,
                lambda: self._create_tar(
                    source_path,
                    str(backup_path),
                    target.compression_enabled,
                ),
            )

        return str(backup_path)

    def _create_tar(self, source: str, dest: str, compress: bool = True):
        """Create tar archive."""
        mode = "w:gz" if compress else "w"
        with tarfile.open(dest, mode, compresslevel=6 if compress else None) as tar:
            tar.add(source, arcname=os.path.basename(source))

    def _create_multi_source_tar(
        self,
        sources: List[tuple],  # [(archive_name, source_path), ...]
        dest: str,
        compress: bool = True,
    ):
        """Create tar archive from multiple sources."""
        mode = "w:gz" if compress else "w"
        with tarfile.open(dest, mode, compresslevel=6 if compress else None) as tar:
            for archive_name, source_path in sources:
                if os.path.exists(source_path):
                    tar.add(source_path, arcname=archive_name.lstrip("/"))

    async def _calculate_checksum(self, file_path: str) -> str:
        """Calculate SHA256 checksum of file."""
        loop = asyncio.get_event_loop()

        def calc():
            sha256 = hashlib.sha256()
            with open(file_path, "rb") as f:
                for chunk in iter(lambda: f.read(8192), b""):
                    sha256.update(chunk)
            return sha256.hexdigest()

        return await loop.run_in_executor(None, calc)

    async def _run_hook(self, command: str):
        """Run a pre/post backup hook command safely.

        Uses shlex.split to parse the command into arguments, preventing
        shell injection attacks by avoiding shell=True.
        """
        try:
            # Parse command into safe argument list
            args = shlex.split(command)
        except ValueError as e:
            raise Exception(f"Invalid hook command syntax: {e}")

        if not args:
            raise Exception("Empty hook command")

        process = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            raise Exception(f"Hook command failed: {stderr.decode()}")

        logger.info(f"Hook output: {stdout.decode()}")

    async def restore_backup(
        self, backup_id: int, target_path: Optional[str] = None
    ) -> bool:
        """Restore a backup."""
        async with async_session() as session:
            result = await session.execute(select(Backup).where(Backup.id == backup_id))
            backup = result.scalar_one_or_none()

            if not backup:
                logger.error(f"Backup {backup_id} not found")
                return False

            if backup.status != BackupStatus.COMPLETED:
                logger.error(f"Backup {backup_id} is not completed")
                return False

            # Get target
            result = await session.execute(
                select(BackupTarget).where(BackupTarget.id == backup.target_id)
            )
            target = result.scalar_one_or_none()

        if not backup.file_path or not os.path.exists(backup.file_path):
            logger.error(f"Backup file not found: {backup.file_path}")
            return False

        # Determine restore path
        if target_path:
            restore_path = target_path
        elif target.target_type == "volume":
            volumes = await docker_client.list_volumes()
            volume = next((v for v in volumes if v.name == target.volume_name), None)
            if not volume:
                raise ValueError(f"Volume {target.volume_name} not found")
            restore_path = volume.mountpoint
        elif target.target_type == "path":
            restore_path = target.host_path
        else:
            raise ValueError("Cannot determine restore path")

        # Stop containers if needed
        containers_to_stop = []
        original_states = {}

        if target.stop_container and target.container_name:
            containers_to_stop.append(target.container_name)

        if target.dependencies:
            containers_to_stop.extend(target.dependencies)

        try:
            # Stop containers
            for container_name in containers_to_stop:
                state = await docker_client.get_container_state(container_name)
                original_states[container_name] = state
                if state == "running":
                    await docker_client.stop_container(container_name)

            # Extract backup
            loop = asyncio.get_event_loop()

            is_compressed = backup.file_path.endswith(".gz")
            mode = "r:gz" if is_compressed else "r"

            await loop.run_in_executor(
                None,
                lambda: self._extract_tar(backup.file_path, restore_path, mode),
            )

            # Restart containers
            for container_name in reversed(containers_to_stop):
                if original_states.get(container_name) == "running":
                    await docker_client.start_container(container_name)

            logger.info(f"Restored backup {backup_id} to {restore_path}")
            return True

        except Exception as e:
            logger.error(f"Restore failed: {e}")

            # Try to restart containers
            for container_name in reversed(containers_to_stop):
                if original_states.get(container_name) == "running":
                    try:
                        await docker_client.start_container(container_name)
                    except Exception:
                        pass

            return False

    def _extract_tar(self, source: str, dest: str, mode: str):
        """Extract tar archive safely.

        Validates that all extracted files stay within the destination
        directory to prevent path traversal attacks (CVE-2007-4559).
        """
        abs_dest = os.path.abspath(dest)

        with tarfile.open(source, mode) as tar:
            # Validate all members before extraction
            for member in tar.getmembers():
                member_path = os.path.join(dest, member.name)
                abs_member = os.path.abspath(member_path)

                # Check for path traversal
                if (
                    not abs_member.startswith(abs_dest + os.sep)
                    and abs_member != abs_dest
                ):
                    raise ValueError(
                        f"Path traversal detected in archive: {member.name}"
                    )

                # Check for absolute paths in archive
                if os.path.isabs(member.name):
                    raise ValueError(
                        f"Absolute path detected in archive: {member.name}"
                    )

                # Check for suspicious symlinks
                if member.issym() or member.islnk():
                    link_path = os.path.join(
                        dest, os.path.dirname(member.name), member.linkname
                    )
                    abs_link = os.path.abspath(link_path)
                    if not abs_link.startswith(abs_dest + os.sep):
                        raise ValueError(
                            f"Symlink escape detected in archive: "
                            f"{member.name} -> {member.linkname}"
                        )

            # Safe to extract after validation
            tar.extractall(dest)

    async def estimate_backup_duration(self, target: BackupTarget) -> int:
        """Estimate backup duration in seconds based on historical data."""
        async with async_session() as session:
            # Get last 5 successful backups
            result = await session.execute(
                select(Backup)
                .where(
                    Backup.target_id == target.id,
                    Backup.status == BackupStatus.COMPLETED,
                    Backup.duration_seconds.isnot(None),
                )
                .order_by(Backup.created_at.desc())
                .limit(5)
            )
            backups = result.scalars().all()

            if not backups:
                # Default estimate: 60 seconds
                return 60

            # Average duration
            avg_duration = sum(b.duration_seconds for b in backups) / len(backups)

            # Add 20% buffer
            return int(avg_duration * 1.2)


# Global backup engine instance
backup_engine = BackupEngine()
