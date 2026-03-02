"""
Backup engine - handles the actual backup process.
"""

import asyncio
import hashlib
import json
import logging
import os
import re
import shlex
import shutil
import subprocess
import tarfile
import time
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from sqlalchemy import select, update

from app.config import settings
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    BackupType,
    EncryptionConfig,
    async_session,
)
from app.docker_client import docker_client
from app.encryption import (
    DecryptionError,
    EncryptionError,
    decrypt_backup,
    encrypt_backup,
)

# Path to the tar worker script that runs as root via sudo.
# This allows reading Docker volume files owned by arbitrary UIDs.
_TAR_WORKER = os.path.join(os.path.dirname(__file__), "tar_worker.py")
_PYTHON = "/opt/venv/bin/python3"

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
        self.last_backup_time = datetime.now(timezone.utc)

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
        self._target_locks: Dict[int, asyncio.Lock] = {}
        self._queue: asyncio.Queue = asyncio.Queue()
        self._queue_worker_task: Optional[asyncio.Task] = None

    def _get_target_lock(self, target_id: int) -> asyncio.Lock:
        """Get or create a per-target lock to prevent overlapping backups."""
        if target_id not in self._target_locks:
            self._target_locks[target_id] = asyncio.Lock()
        return self._target_locks[target_id]

    async def start_queue_worker(self) -> None:
        """Start the background queue worker that processes backup jobs."""
        if self._queue_worker_task is None or self._queue_worker_task.done():
            self._queue_worker_task = asyncio.create_task(self._queue_loop())

    async def _queue_loop(self) -> None:
        """Process queued backup jobs.

        Each job is spawned as a task so different targets can run
        concurrently while the per-target lock and global semaphore
        control overlap.
        """
        while True:
            backup_id = await self._queue.get()
            task = asyncio.create_task(self._run_queued_backup(backup_id))
            self.active_backups[backup_id] = task

    async def _run_queued_backup(self, backup_id: int) -> None:
        """Run a single queued backup and clean up tracking state."""
        try:
            await self.run_backup(backup_id)
        except Exception as exc:
            logger.error("Queued backup %d failed: %s", backup_id, exc)
        finally:
            self.active_backups.pop(backup_id, None)
            self._queue.task_done()

    async def enqueue(self, backup_id: int) -> None:
        """Add a backup to the processing queue."""
        await self._queue.put(backup_id)

    async def run_batch_sequential(self, backup_ids: list[int]) -> None:
        """Run a list of backups strictly one after another.

        Used by *trigger-all* so that each target completes its full
        stop → backup → start cycle before the next one begins.
        The backups still go through the per-target lock and global
        semaphore, just like individually enqueued jobs.
        """
        for backup_id in backup_ids:
            try:
                await self.run_backup(backup_id)
            except Exception as exc:
                logger.error("Batch backup %d failed: %s", backup_id, exc)

    async def shutdown(self, timeout: int = 30) -> None:
        """Wait for in-progress backups to finish, then stop the queue worker."""
        # Cancel the queue worker so no new jobs are picked up
        if self._queue_worker_task and not self._queue_worker_task.done():
            self._queue_worker_task.cancel()
            try:
                await self._queue_worker_task
            except asyncio.CancelledError:
                pass

        if not self.active_backups:
            return

        logger.info(
            "Waiting up to %ds for %d active backup(s) to finish...",
            timeout,
            len(self.active_backups),
        )
        tasks = list(self.active_backups.values())
        done, pending = await asyncio.wait(tasks, timeout=timeout)

        if pending:
            logger.warning(
                "Cancelling %d backup(s) that didn't finish within timeout",
                len(pending),
            )
            for task in pending:
                task.cancel()
            await asyncio.gather(*pending, return_exceptions=True)

        logger.info("Backup engine shutdown complete")

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

        elif target.target_type == "stack" and target.stack_name:
            stacks = await docker_client.get_stacks()
            if not any(s.name == target.stack_name for s in stacks):
                issues.append(f"Stack '{target.stack_name}' not found")
                logger.warning(
                    "Validation failed: stack not found",
                    extra={
                        "target_id": target.id,
                        "target_name": target.name,
                        "stack_name": target.stack_name,
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

        Acquires a per-target lock first (prevents overlapping backups for the
        same target), then a global semaphore (limits total concurrency).

        Args:
            backup_id: ID of the backup to run
            skip_validation: Skip pre-backup validation (for testing)

        Returns:
            True if backup succeeded, False otherwise
        """
        # Determine target_id to acquire per-target lock
        async with async_session() as session:
            result = await session.execute(select(Backup).where(Backup.id == backup_id))
            backup_peek = result.scalar_one_or_none()
            if not backup_peek:
                logger.error("Backup %d not found", backup_id)
                return False
            peek_target_id = backup_peek.target_id

        target_lock = self._get_target_lock(peek_target_id)

        async with target_lock:
            return await self._run_backup_inner(backup_id, skip_validation)

    async def _run_backup_inner(
        self, backup_id: int, skip_validation: bool = False
    ) -> bool:
        """Inner backup logic, called with a per-target lock already held."""
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
                        backup.completed_at = datetime.now(timezone.utc)
                        await session.commit()
                        await self._notify_progress(backup_id, -1, error_msg)

                        # Record failed backup metrics
                        duration = time.time() - start_time
                        self.metrics.record_backup(target_id, duration, 0, False)
                        return False

                # Update status to running
                backup.status = BackupStatus.RUNNING
                backup.started_at = datetime.now(timezone.utc)
                await session.commit()

            await self._notify_progress(backup_id, 0, "Starting backup...")

        try:
            # Determine containers to stop
            containers_to_stop = []
            original_states = {}
            start_order = []  # Order for restarting containers

            if target.target_type == "stack" and target.stack_name:
                # For stack backups, stop all stack containers in dependency order
                stacks = await docker_client.get_stacks()
                stack = next((s for s in stacks if s.name == target.stack_name), None)
                if stack and stack.stop_order:
                    containers_to_stop = stack.stop_order
                    start_order = stack.start_order
                    logger.info(
                        f"Stack backup will stop containers: {containers_to_stop}"
                    )
            elif target.stop_container and target.container_name:
                containers_to_stop.append(target.container_name)

            # Add manual dependencies
            if target.dependencies:
                for dep in target.dependencies:
                    if dep not in containers_to_stop:
                        containers_to_stop.append(dep)

            # Get dependency order for safe stopping (if not already ordered from stack)
            if containers_to_stop and not start_order:
                containers_to_stop = await docker_client.get_dependency_order(
                    containers_to_stop
                )
                start_order = list(reversed(containers_to_stop))

            if containers_to_stop:
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

            # Encrypt backup if enabled
            encrypted = False
            encryption_key_path = None

            encryption_config = await self._get_encryption_config()
            if encryption_config and encryption_config.encryption_enabled:
                await self._notify_progress(backup_id, 82, "Encrypting backup...")
                try:
                    result = await encrypt_backup(
                        Path(backup_path),
                        encryption_config.public_key,
                    )
                    backup_path = str(result.encrypted_path)
                    encryption_key_path = str(result.key_path)
                    encrypted = True
                    # Recalculate size for encrypted file
                    file_size = os.path.getsize(backup_path)
                    logger.info(f"Backup encrypted: {backup_path}")
                except EncryptionError as e:
                    logger.error(f"Encryption failed: {e}")
                    # Continue with unencrypted backup
                    await self._notify_progress(
                        backup_id, 83, f"Encryption failed, backup unencrypted: {e}"
                    )

            # Run post-backup hook
            if target.post_backup_command:
                await self._notify_progress(
                    backup_id, 85, "Running post-backup hook..."
                )
                await self._run_hook(target.post_backup_command)

            # Restart containers in start order (respecting dependencies)
            if containers_to_stop:
                await self._notify_progress(backup_id, 90, "Starting containers...")
                restart_order = (
                    start_order if start_order else reversed(containers_to_stop)
                )
                for container_name in restart_order:
                    if original_states.get(container_name) == "running":
                        await docker_client.start_container(container_name)

            # Update backup record
            async with async_session() as session:
                duration_seconds = int(
                    (datetime.now(timezone.utc) - backup.started_at).total_seconds()
                )
                await session.execute(
                    update(Backup)
                    .where(Backup.id == backup_id)
                    .values(
                        status=BackupStatus.COMPLETED,
                        file_path=backup_path,
                        file_size=file_size,
                        checksum=checksum,
                        completed_at=datetime.now(timezone.utc),
                        duration_seconds=duration_seconds,
                        encrypted=encrypted,
                        encryption_key_path=encryption_key_path,
                    )
                )
                await session.commit()

            # Sync to remote storage if configured
            if target.remote_storage_ids:
                await self._sync_to_remote(
                    backup_id, target, backup_path, encrypted, encryption_key_path
                )

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

            # Try to restart containers on failure (use start_order if available)
            restart_order = (
                start_order if start_order else list(reversed(containers_to_stop))
            )
            for container_name in restart_order:
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
                        completed_at=datetime.now(timezone.utc),
                    )
                )
                await session.commit()

            # Record failed backup metrics
            duration = time.time() - start_time
            if target_id:
                self.metrics.record_backup(target_id, duration, 0, False)

            await self._notify_progress(backup_id, -1, f"Backup failed: {e}")
            return False

    async def _sync_to_remote(
        self,
        backup_id: int,
        target: BackupTarget,
        backup_path: str,
        encrypted: bool = False,
        encryption_key_path: str | None = None,
    ) -> None:
        """Sync a completed backup to configured remote storage backends.

        Failures are logged but do NOT mark the backup as failed — the
        local backup is still valid.
        """
        from app.remote_storage import StorageConfig, StorageType, storage_manager

        await self._notify_progress(backup_id, 95, "Syncing to remote storage...")

        try:
            # Ensure backends are initialised from the database
            async with async_session() as session:
                from app.database import RemoteStorage as RemoteStorageModel

                for storage_id in target.remote_storage_ids:
                    if storage_manager.get_backend(storage_id):
                        continue
                    result = await session.execute(
                        select(RemoteStorageModel).where(
                            RemoteStorageModel.id == storage_id
                        )
                    )
                    storage = result.scalar_one_or_none()
                    if not storage or not storage.enabled:
                        continue

                    from app.credential_encryption import decrypt_value

                    config = StorageConfig(
                        id=storage.id,
                        name=storage.name,
                        storage_type=StorageType(storage.storage_type),
                        enabled=storage.enabled,
                        host=storage.host,
                        port=storage.port,
                        username=storage.username,
                        password=(
                            decrypt_value(storage.password)
                            if storage.password
                            else storage.password
                        ),
                        base_path=storage.base_path,
                        ssh_key_path=storage.ssh_key_path,
                        s3_bucket=storage.s3_bucket,
                        s3_region=storage.s3_region,
                        s3_access_key=(
                            decrypt_value(storage.s3_access_key)
                            if storage.s3_access_key
                            else storage.s3_access_key
                        ),
                        s3_secret_key=(
                            decrypt_value(storage.s3_secret_key)
                            if storage.s3_secret_key
                            else storage.s3_secret_key
                        ),
                        s3_endpoint_url=storage.s3_endpoint_url,
                        webdav_url=storage.webdav_url,
                        rclone_remote=storage.rclone_remote,
                    )
                    storage_manager.add_storage(config)

            local_path = Path(backup_path)
            safe_name = self._sanitize_path_name(target.name)
            results = await storage_manager.sync_backup(
                local_path,
                safe_name,
                local_path.name,
                target.remote_storage_ids,
            )

            # Also upload .key sidecar for encrypted backups
            if encrypted and encryption_key_path:
                key_local = Path(encryption_key_path)
                if key_local.exists():
                    try:
                        await storage_manager.sync_backup(
                            key_local,
                            safe_name,
                            key_local.name,
                            target.remote_storage_ids,
                        )
                        logger.info(
                            "Uploaded .key sidecar for backup %s",
                            backup_id,
                        )
                    except Exception as e:
                        logger.warning(
                            "Failed to upload .key sidecar for backup %s: %s",
                            backup_id,
                            e,
                        )

            # Record sync results in BackupStorageSync table
            remote_path = f"{safe_name}/{local_path.name}"
            async with async_session() as session:
                for storage_id, success in results.items():
                    sync_record = BackupStorageSync(
                        backup_id=backup_id,
                        storage_id=storage_id,
                        remote_path=remote_path,
                        synced_at=datetime.now(timezone.utc) if success else None,
                        sync_status="completed" if success else "failed",
                        error_message=None if success else "Upload failed",
                    )
                    session.add(sync_record)
                await session.commit()

            succeeded = sum(1 for v in results.values() if v)
            failed = sum(1 for v in results.values() if not v)

            if failed:
                logger.warning(
                    "Remote sync partially failed for backup %s: %s/%s succeeded",
                    backup_id,
                    succeeded,
                    succeeded + failed,
                )
                await self._notify_progress(
                    backup_id,
                    97,
                    f"Remote sync: {succeeded}/{succeeded + failed} succeeded",
                )
            else:
                logger.info(
                    "Backup %s synced to %s remote storage(s)",
                    backup_id,
                    succeeded,
                )
                await self._notify_progress(
                    backup_id, 97, f"Synced to {succeeded} remote storage(s)"
                )

        except Exception as e:
            logger.error(
                "Remote storage sync failed for backup %s: %s",
                backup_id,
                e,
                exc_info=True,
            )
            await self._notify_progress(backup_id, 97, f"Remote sync failed: {e}")

    @staticmethod
    def _sanitize_path_name(name: str) -> str:
        """Sanitize a target name for safe use in filesystem paths.

        Replaces characters that are invalid or problematic on common
        filesystems (ext4, NTFS, SMB/CIFS, NFS) with underscores and
        collapses consecutive underscores.
        """
        # Replace characters invalid on Windows/SMB/CIFS or problematic in paths
        safe = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", name)
        # Collapse multiple underscores
        safe = re.sub(r"_+", "_", safe)
        # Strip leading/trailing whitespace and underscores
        safe = safe.strip(" _")
        # Fallback if name is completely empty after sanitization
        return safe or "unnamed_target"

    @staticmethod
    def _resolve_backup_path(file_path: str) -> str | None:
        """Resolve a backup file path, handling legacy paths with unsafe characters.

        Old backups may reference paths containing characters like ':' that are
        now sanitised.  Tries the exact stored path first, then a sanitised
        variant so that legacy DB records still work.

        Returns the first existing path, or None.
        """
        if os.path.exists(file_path):
            return file_path

        sanitized = re.sub(r'[<>:"|?*]', "_", file_path)
        sanitized = re.sub(r"_+", "_", sanitized)
        if sanitized != file_path and os.path.exists(sanitized):
            logger.info(
                "Resolved backup via sanitised path: %s -> %s",
                file_path,
                sanitized,
            )
            return sanitized

        return None

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

            # Get selected volumes (empty list = all volumes)
            selected_volumes = target.selected_volumes or []

            # Create combined backup of all volumes
            source_paths = []
            for mount in container.mounts:
                if mount.get("type") == "volume":
                    volume_name = mount.get("name")
                    # Filter by selected_volumes if specified
                    if selected_volumes and volume_name not in selected_volumes:
                        continue
                    volumes = await docker_client.list_volumes()
                    volume = next((v for v in volumes if v.name == volume_name), None)
                    if volume:
                        source_paths.append(
                            (mount.get("destination"), volume.mountpoint)
                        )
        elif target.target_type == "stack":
            # Stack backup - collect all volumes from all containers in the stack
            stacks = await docker_client.get_stacks()
            stack = next((s for s in stacks if s.name == target.stack_name), None)
            if not stack:
                raise ValueError(f"Stack {target.stack_name} not found")

            # Get selected volumes (empty list = all volumes)
            selected_volumes = target.selected_volumes or []

            # Collect all volumes from the stack
            source_paths = []
            volumes = await docker_client.list_volumes()
            for volume_name in stack.volumes:
                # Filter by selected_volumes if specified
                if selected_volumes and volume_name not in selected_volumes:
                    continue
                volume = next((v for v in volumes if v.name == volume_name), None)
                if volume:
                    source_paths.append((f"volumes/{volume_name}", volume.mountpoint))

            if not source_paths:
                raise ValueError(f"Stack {target.stack_name} has no volumes to backup")
        else:
            raise ValueError(f"Unknown target type: {target.target_type}")

        # Create backup directory
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
        safe_name = self._sanitize_path_name(target.name)
        backup_dir = Path(settings.BACKUP_BASE_PATH) / safe_name
        backup_dir.mkdir(parents=True, exist_ok=True)

        filename = f"{safe_name}_{timestamp}.tar"
        if target.compression_enabled:
            filename += ".gz"

        backup_path = backup_dir / filename

        # Get path filters
        include_paths = target.include_paths or []
        exclude_paths = target.exclude_paths or []
        per_volume_rules = target.per_volume_rules or {}

        # Create tarball
        loop = asyncio.get_event_loop()

        if target.target_type in ("container", "stack") and source_paths:
            await loop.run_in_executor(
                None,
                lambda: self._create_multi_source_tar(
                    source_paths,
                    str(backup_path),
                    target.compression_enabled,
                    include_paths,
                    exclude_paths,
                    per_volume_rules,
                ),
            )
        else:
            await loop.run_in_executor(
                None,
                lambda: self._create_tar(
                    source_path,
                    str(backup_path),
                    target.compression_enabled,
                    include_paths,
                    exclude_paths,
                ),
            )

        return str(backup_path)

    def _should_include_path(
        self,
        path: str,
        include_paths: List[str],
        exclude_paths: List[str],
    ) -> bool:
        """Check if a path should be included in the backup.

        Args:
            path: The path to check (relative to volume/source root, NOT archive root)
            include_paths: List of paths/patterns to include (empty = all)
            exclude_paths: List of paths/patterns to exclude

        Returns:
            True if the path should be included, False otherwise
        """
        import fnmatch

        # Normalize path
        path = path.lstrip("/")

        # Check excludes first
        for pattern in exclude_paths:
            pattern = pattern.lstrip("/")
            if fnmatch.fnmatch(path, pattern) or path.startswith(
                pattern.rstrip("*").rstrip("/") + "/"
            ):
                return False

        # If include_paths is specified, path must match at least one
        if include_paths:
            for pattern in include_paths:
                pattern = pattern.lstrip("/")
                if fnmatch.fnmatch(path, pattern) or path.startswith(
                    pattern.rstrip("*").rstrip("/") + "/"
                ):
                    return True
            return False

        return True

    def _is_parent_of_included(
        self,
        dir_path: str,
        include_paths: List[str],
    ) -> bool:
        """Check if dir_path is a parent directory of any include path pattern.

        This ensures directories leading to included content are traversed
        even when include_paths filtering would otherwise skip them.
        """
        dir_path = dir_path.lstrip("/").rstrip("/") + "/"

        for pattern in include_paths:
            pattern = pattern.lstrip("/")
            # Extract the literal prefix before any wildcard characters
            literal_prefix = ""
            for char in pattern:
                if char in "*?[":
                    break
                literal_prefix += char

            # Directory is a parent of the pattern's target location
            if literal_prefix.startswith(dir_path):
                return True
            # Directory is within or at the pattern's target location
            if dir_path.startswith(literal_prefix):
                return True

        return False

    def _add_source_to_tar(
        self,
        tar: tarfile.TarFile,
        source_path: str,
        arcname_base: str,
        include_paths: List[str],
        exclude_paths: List[str],
    ) -> List[str]:
        """Add a source directory to a tar archive with filtering and error handling.

        Walks directories manually instead of using tar.add() to:
        1. Catch PermissionError/OSError per file (skip instead of failing)
        2. Match include/exclude against paths relative to the source root
           (not the archive path which has an arcname prefix)
        3. Properly handle directory traversal for include_paths

        Returns:
            List of skipped file paths due to permission errors.
        """
        skipped_files: List[str] = []
        has_filters = bool(include_paths or exclude_paths)

        for dirpath, dirnames, filenames in os.walk(source_path):
            # Calculate path relative to source root
            rel_dir = os.path.relpath(dirpath, source_path)
            if rel_dir == ".":
                rel_dir = ""

            # Archive path includes the arcname prefix
            arc_dir = os.path.join(arcname_base, rel_dir) if rel_dir else arcname_base

            # Prune excluded directories in-place (prevents recursion)
            if exclude_paths:
                dirnames[:] = [
                    d
                    for d in dirnames
                    if self._should_include_path(
                        os.path.join(rel_dir, d) if rel_dir else d, [], exclude_paths
                    )
                ]

            # For include paths: skip directories that can't contain included files
            if include_paths and rel_dir:
                if not self._should_include_path(
                    rel_dir, include_paths, []
                ) and not self._is_parent_of_included(rel_dir, include_paths):
                    dirnames.clear()
                    continue

            # Try to add directory entry
            try:
                tarinfo = tar.gettarinfo(dirpath, arcname=arc_dir)
                tar.addfile(tarinfo)
            except (PermissionError, OSError) as e:
                logger.warning(f"Cannot read directory {dirpath}, skipping: {e}")
                skipped_files.append(dirpath)
                dirnames.clear()
                continue

            # Add files
            for filename in filenames:
                filepath = os.path.join(dirpath, filename)
                file_rel = os.path.join(rel_dir, filename) if rel_dir else filename
                arcpath = os.path.join(arc_dir, filename)

                # Check include/exclude against relative path (no archive prefix)
                if has_filters and not self._should_include_path(
                    file_rel, include_paths, exclude_paths
                ):
                    continue

                try:
                    tarinfo = tar.gettarinfo(filepath, arcname=arcpath)
                    if tarinfo.isreg():
                        with open(filepath, "rb") as f:
                            tar.addfile(tarinfo, f)
                    else:
                        tar.addfile(tarinfo)
                except (PermissionError, OSError) as e:
                    logger.warning(f"Cannot read file {filepath}, skipping: {e}")
                    skipped_files.append(filepath)
                    continue

        return skipped_files

    # ── Tar creation (delegates to tar_worker via sudo) ──

    _sudo_checked: Optional[bool] = None

    @classmethod
    def _sudo_available(cls) -> bool:
        """Return True when ``sudo`` can invoke the tar worker as root.

        The result is cached on first call.

        The sudoers rule uses a ``*`` glob that requires at least one
        argument after the script path, so we pass a dummy ``{}`` here.
        """
        if cls._sudo_checked is not None:
            return cls._sudo_checked
        try:
            # -n = non-interactive, -l = list allowed commands.
            # We pass a dummy argument so the sudoers glob ``*``
            # (which expects at least one arg after the script) matches.
            result = subprocess.run(
                ["sudo", "-n", "-l", _PYTHON, _TAR_WORKER, "{}"],
                capture_output=True,
                timeout=5,
            )
            cls._sudo_checked = result.returncode == 0
            if not cls._sudo_checked:
                logger.warning(
                    "sudo -l check failed (rc=%d): %s",
                    result.returncode,
                    (result.stderr or b"").decode(errors="replace").strip(),
                )
        except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
            logger.warning("sudo -l probe raised %s: %s", type(exc).__name__, exc)
            cls._sudo_checked = False
        return cls._sudo_checked

    def _run_tar_worker(self, config: dict) -> List[str]:
        """Run tar_worker.py via sudo and return list of skipped files.

        Raises RuntimeError when the worker reports an error.
        """
        result = subprocess.run(
            ["sudo", "-n", _PYTHON, _TAR_WORKER, json.dumps(config)],
            capture_output=True,
            text=True,
            timeout=3600,
        )
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(
                f"tar_worker produced invalid output "
                f"(exit {result.returncode}): {result.stderr or result.stdout}"
            )

        if payload.get("status") != "ok":
            raise RuntimeError(
                f"tar_worker failed: {payload.get('message', 'unknown error')}"
            )

        return payload.get("skipped_files", [])

    def _create_tar(
        self,
        source: str,
        dest: str,
        compress: bool = True,
        include_paths: Optional[List[str]] = None,
        exclude_paths: Optional[List[str]] = None,
    ):
        """Create tar archive with path filtering and graceful permission error handling.

        Delegates to ``tar_worker.py`` via sudo so Docker volume files
        owned by arbitrary UIDs (e.g. mongodb:999) can be read.
        Falls back to in-process tarfile when sudo is unavailable.
        """
        include_paths = include_paths or []
        exclude_paths = exclude_paths or []
        arcname_base = os.path.basename(source)

        if self._sudo_available():
            config = {
                "dest": dest,
                "compress": compress,
                "sources": [[arcname_base, source]],
                "include_paths": include_paths,
                "exclude_paths": exclude_paths,
                "per_volume_rules": {},
                # For host-path targets the source is outside Docker
                # volume dirs — tell tar_worker to allow it.
                "allowed_sources": [source],
            }
            skipped = self._run_tar_worker(config)
        else:
            logger.warning(
                "sudo not available — falling back to in-process tar "
                "(files owned by other UIDs may be skipped)"
            )
            mode = "w:gz" if compress else "w"
            with tarfile.open(dest, mode, compresslevel=6 if compress else None) as tar:
                skipped = self._add_source_to_tar(
                    tar, source, arcname_base, include_paths, exclude_paths
                )

        if skipped:
            logger.info(
                "Backup completed with %d skipped file(s) due to permission errors",
                len(skipped),
            )

    def _create_multi_source_tar(
        self,
        sources: List[tuple],  # [(archive_name, source_path), ...]
        dest: str,
        compress: bool = True,
        include_paths: Optional[List[str]] = None,
        exclude_paths: Optional[List[str]] = None,
        per_volume_rules: Optional[Dict[str, Dict[str, List[str]]]] = None,
    ):
        """Create tar archive from multiple sources with optional path filtering.

        Per-volume rules override global include/exclude for specific volumes.
        Delegates to ``tar_worker.py`` via sudo for elevated reading
        privileges, with in-process fallback.
        """
        include_paths = include_paths or []
        exclude_paths = exclude_paths or []
        per_volume_rules = per_volume_rules or {}

        if self._sudo_available():
            config = {
                "dest": dest,
                "compress": compress,
                "sources": [[a, s] for a, s in sources],
                "include_paths": include_paths,
                "exclude_paths": exclude_paths,
                "per_volume_rules": per_volume_rules,
            }
            skipped = self._run_tar_worker(config)
        else:
            logger.warning(
                "sudo not available — falling back to in-process tar "
                "(files owned by other UIDs may be skipped)"
            )
            skipped = []
            mode = "w:gz" if compress else "w"
            with tarfile.open(dest, mode, compresslevel=6 if compress else None) as tar:
                for archive_name, source_path in sources:
                    if not os.path.exists(source_path):
                        continue

                    volume_key = archive_name.lstrip("/")
                    volume_basename = os.path.basename(volume_key)
                    vol_rules = per_volume_rules.get(
                        volume_key
                    ) or per_volume_rules.get(volume_basename)

                    if vol_rules:
                        vol_include = vol_rules.get("include_paths", [])
                        vol_exclude = vol_rules.get("exclude_paths", [])
                    else:
                        vol_include = include_paths
                        vol_exclude = exclude_paths

                    skipped.extend(
                        self._add_source_to_tar(
                            tar,
                            source_path,
                            volume_key,
                            vol_include,
                            vol_exclude,
                        )
                    )

        if skipped:
            logger.info(
                "Backup completed with %d skipped file(s) due to permission errors",
                len(skipped),
            )

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
        Only commands in the ALLOWED_HOOK_COMMANDS allowlist are permitted.
        """
        try:
            # Parse command into safe argument list
            args = shlex.split(command)
        except ValueError as e:
            raise Exception(f"Invalid hook command syntax: {e}")

        if not args:
            raise Exception("Empty hook command")

        allowed = settings.ALLOWED_HOOK_COMMANDS.split(",")
        if args[0] not in allowed:
            raise Exception(
                f"Hook command '{args[0]}' is not in the allowed commands list: {allowed}"
            )

        logger.info(f"Executing hook command: {args[0]} (full: {command})")

        process = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                process.communicate(),
                timeout=settings.HOOK_COMMAND_TIMEOUT,
            )
        except asyncio.TimeoutError:
            process.kill()
            await process.wait()
            raise Exception(
                f"Hook command timed out after {settings.HOOK_COMMAND_TIMEOUT}s"
            )

        if process.returncode != 0:
            raise Exception(f"Hook command failed: {stderr.decode()}")

        logger.info(f"Hook output: {stdout.decode()}")

    async def restore_backup(
        self,
        backup_id: int,
        target_path: Optional[str] = None,
        private_key: Optional[str] = None,
    ) -> bool:
        """Restore a backup.

        Args:
            backup_id: ID of the backup to restore
            target_path: Optional custom restore path
            private_key: Required for encrypted backups
        """
        is_encrypted = False
        encryption_key_path = None

        async with async_session() as session:
            result = await session.execute(select(Backup).where(Backup.id == backup_id))
            backup = result.scalar_one_or_none()

            if not backup:
                logger.error(f"Backup {backup_id} not found")
                return False

            if backup.status != BackupStatus.COMPLETED:
                logger.error(f"Backup {backup_id} is not completed")
                return False

            # Check if backup is encrypted
            is_encrypted = backup.encrypted and backup.encryption_key_path
            encryption_key_path = backup.encryption_key_path

            if is_encrypted and not private_key:
                logger.error("Private key required for encrypted backup")
                return False

            # Get target
            result = await session.execute(
                select(BackupTarget).where(BackupTarget.id == backup.target_id)
            )
            target = result.scalar_one_or_none()

        if not backup.file_path:
            logger.error("Backup file path is empty for backup %s", backup.id)
            return False

        # Resolve the file path (handle legacy paths with unsafe chars)
        resolved_path = self._resolve_backup_path(backup.file_path)
        if not resolved_path:
            logger.error("Backup file not found: %s", backup.file_path)
            return False
        backup_file = resolved_path

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

            # Handle encrypted backups
            backup_file_to_extract = backup_file
            temp_decrypted_file = None

            if is_encrypted:
                try:
                    encrypted_path = Path(backup_file)
                    key_path = Path(encryption_key_path)

                    # Decrypt to temp file
                    decrypted_path = await decrypt_backup(
                        encrypted_path, key_path, private_key
                    )
                    backup_file_to_extract = str(decrypted_path)
                    temp_decrypted_file = decrypted_path
                    logger.info(f"Decrypted backup to {decrypted_path}")
                except DecryptionError as e:
                    logger.error(f"Decryption failed: {e}")
                    raise ValueError(f"Failed to decrypt backup: {e}")

            # Extract backup
            loop = asyncio.get_event_loop()

            is_compressed = backup_file_to_extract.endswith(".gz")
            mode = "r:gz" if is_compressed else "r"

            await loop.run_in_executor(
                None,
                lambda: self._extract_tar(backup_file_to_extract, restore_path, mode),
            )

            # Clean up temp decrypted file
            if temp_decrypted_file and temp_decrypted_file.exists():
                temp_decrypted_file.unlink()

            # Restart containers
            for container_name in reversed(containers_to_stop):
                if original_states.get(container_name) == "running":
                    await docker_client.start_container(container_name)

            logger.info(f"Restored backup {backup_id} to {restore_path}")
            return True

        except Exception as e:
            logger.error(f"Restore failed: {e}")

            # Clean up temp decrypted file on error
            if temp_decrypted_file and temp_decrypted_file.exists():
                try:
                    temp_decrypted_file.unlink()
                except Exception:
                    pass

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
        abs_dest = os.path.realpath(dest)

        with tarfile.open(source, mode) as tar:
            # Validate all members before extraction
            for member in tar.getmembers():
                member_path = os.path.join(dest, member.name)
                abs_member = os.path.realpath(member_path)

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
                    abs_link = os.path.realpath(link_path)
                    if not abs_link.startswith(abs_dest + os.sep):
                        raise ValueError(
                            f"Symlink escape detected in archive: "
                            f"{member.name} -> {member.linkname}"
                        )

            # Safe to extract after validation
            import sys

            if sys.version_info >= (3, 12):
                tar.extractall(dest, filter="data")
            else:
                tar.extractall(dest)

    async def _get_encryption_config(self) -> Optional[EncryptionConfig]:
        """Get encryption configuration if set up."""
        async with async_session() as session:
            result = await session.execute(select(EncryptionConfig).limit(1))
            config = result.scalar_one_or_none()
            if config and config.setup_completed:
                return config
            return None

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
