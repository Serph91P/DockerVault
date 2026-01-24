"""
Backup engine - handles the actual backup process.
"""

import asyncio
import os
import tarfile
import hashlib
import gzip
import shutil
from datetime import datetime
from pathlib import Path
from typing import Optional, List, Dict, Any, Callable
import logging

from app.config import settings
from app.docker_client import docker_client, ContainerInfo
from app.database import Backup, BackupTarget, BackupStatus, BackupType, async_session
from sqlalchemy import select, update

logger = logging.getLogger(__name__)


class BackupEngine:
    """Handles backup operations."""
    
    def __init__(self):
        self.active_backups: Dict[int, asyncio.Task] = {}
        self.progress_callbacks: List[Callable] = []
    
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
                metadata={
                    "target_name": target.name,
                    "target_type": target.target_type,
                },
            )
            session.add(backup)
            await session.commit()
            await session.refresh(backup)
            return backup
    
    async def run_backup(self, backup_id: int) -> bool:
        """Run a backup by ID."""
        async with async_session() as session:
            result = await session.execute(
                select(Backup).where(Backup.id == backup_id)
            )
            backup = result.scalar_one_or_none()
            
            if not backup:
                logger.error(f"Backup {backup_id} not found")
                return False
            
            # Get target
            result = await session.execute(
                select(BackupTarget).where(BackupTarget.id == backup.target_id)
            )
            target = result.scalar_one_or_none()
            
            if not target:
                logger.error(f"Target for backup {backup_id} not found")
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
                containers_to_stop = await docker_client.get_dependency_order(containers_to_stop)
                
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
                await self._notify_progress(backup_id, 85, "Running post-backup hook...")
                await self._run_hook(target.post_backup_command)
            
            # Restart containers in reverse order
            if containers_to_stop:
                await self._notify_progress(backup_id, 90, "Starting containers...")
                for container_name in reversed(containers_to_stop):
                    if original_states.get(container_name) == "running":
                        await docker_client.start_container(container_name)
            
            # Update backup record
            async with async_session() as session:
                await session.execute(
                    update(Backup)
                    .where(Backup.id == backup_id)
                    .values(
                        status=BackupStatus.COMPLETED,
                        file_path=backup_path,
                        file_size=file_size,
                        checksum=checksum,
                        completed_at=datetime.utcnow(),
                        duration_seconds=int(
                            (datetime.utcnow() - backup.started_at).total_seconds()
                        ),
                    )
                )
                await session.commit()
            
            await self._notify_progress(backup_id, 100, "Backup completed!")
            logger.info(f"Backup {backup_id} completed successfully")
            return True
            
        except Exception as e:
            logger.error(f"Backup {backup_id} failed: {e}")
            
            # Try to restart containers on failure
            for container_name in reversed(containers_to_stop):
                if original_states.get(container_name) == "running":
                    try:
                        await docker_client.start_container(container_name)
                    except Exception as restart_error:
                        logger.error(f"Failed to restart {container_name}: {restart_error}")
            
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
            volume = next(
                (v for v in volumes if v.name == target.volume_name),
                None
            )
            if not volume:
                raise ValueError(f"Volume {target.volume_name} not found")
            source_path = volume.mountpoint
        elif target.target_type == "path":
            source_path = target.host_path
        elif target.target_type == "container":
            # Get all volume mounts from container
            containers = await docker_client.list_containers()
            container = next(
                (c for c in containers if c.name == target.container_name),
                None
            )
            if not container:
                raise ValueError(f"Container {target.container_name} not found")
            
            # Create combined backup of all volumes
            source_paths = []
            for mount in container.mounts:
                if mount.get("type") == "volume":
                    volumes = await docker_client.list_volumes()
                    volume = next(
                        (v for v in volumes if v.name == mount.get("name")),
                        None
                    )
                    if volume:
                        source_paths.append((mount.get("destination"), volume.mountpoint))
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
        with tarfile.open(dest, mode, compresslevel=settings.COMPRESSION_LEVEL if compress else None) as tar:
            tar.add(source, arcname=os.path.basename(source))
    
    def _create_multi_source_tar(
        self,
        sources: List[tuple],  # [(archive_name, source_path), ...]
        dest: str,
        compress: bool = True,
    ):
        """Create tar archive from multiple sources."""
        mode = "w:gz" if compress else "w"
        with tarfile.open(dest, mode, compresslevel=settings.COMPRESSION_LEVEL if compress else None) as tar:
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
        """Run a pre/post backup hook command."""
        process = await asyncio.create_subprocess_shell(
            command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()
        
        if process.returncode != 0:
            raise Exception(f"Hook command failed: {stderr.decode()}")
        
        logger.info(f"Hook output: {stdout.decode()}")
    
    async def restore_backup(self, backup_id: int, target_path: Optional[str] = None) -> bool:
        """Restore a backup."""
        async with async_session() as session:
            result = await session.execute(
                select(Backup).where(Backup.id == backup_id)
            )
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
            volume = next(
                (v for v in volumes if v.name == target.volume_name),
                None
            )
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
                    except:
                        pass
            
            return False
    
    def _extract_tar(self, source: str, dest: str, mode: str):
        """Extract tar archive."""
        with tarfile.open(source, mode) as tar:
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
