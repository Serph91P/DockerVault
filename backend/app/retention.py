"""
Retention policy management.
Implements Grandfather-Father-Son (GFS) backup retention strategy.
"""

import logging
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Dict, List

from sqlalchemy import delete, select

from app.config import settings
from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    RetentionPolicy,
    async_session,
)

logger = logging.getLogger(__name__)


class RetentionManager:
    """Manages backup retention based on policies."""

    async def apply_retention(self, target_id: int) -> Dict[str, int]:
        """
        Apply retention policy to a target's backups.
        Returns dict with counts of kept and deleted backups.
        """
        async with async_session() as session:
            # Get target with retention policy
            result = await session.execute(
                select(BackupTarget).where(BackupTarget.id == target_id)
            )
            target = result.scalar_one_or_none()

            if not target:
                return {"kept": 0, "deleted": 0, "error": "Target not found"}

            # Get retention policy
            if target.retention_policy_id:
                result = await session.execute(
                    select(RetentionPolicy).where(
                        RetentionPolicy.id == target.retention_policy_id
                    )
                )
                policy = result.scalar_one_or_none()
            else:
                # Use default policy
                result = await session.execute(
                    select(RetentionPolicy).where(RetentionPolicy.name == "default")
                )
                policy = result.scalar_one_or_none()

            if not policy:
                logger.warning(f"No retention policy found for target {target_id}")
                return {"kept": 0, "deleted": 0, "error": "No policy found"}

            # Get all completed backups for this target
            result = await session.execute(
                select(Backup)
                .where(
                    Backup.target_id == target_id,
                    Backup.status == BackupStatus.COMPLETED,
                )
                .order_by(Backup.created_at.desc())
            )
            backups = result.scalars().all()

            if not backups:
                return {"kept": 0, "deleted": 0}

            # Determine which backups to keep using GFS strategy
            to_keep = self._select_backups_to_keep(backups, policy)
            to_delete = [b for b in backups if b.id not in to_keep]

            # Delete backups
            deleted_count = 0
            for backup in to_delete:
                try:
                    # Delete backup file and encryption key sidecar
                    if backup.file_path and os.path.exists(backup.file_path):
                        os.remove(backup.file_path)
                        logger.info(f"Deleted backup file: {backup.file_path}")
                    # Clean up .key sidecar for encrypted backups
                    if backup.encryption_key_path and os.path.exists(
                        backup.encryption_key_path
                    ):
                        os.remove(backup.encryption_key_path)
                        logger.info(
                            f"Deleted encryption key: {backup.encryption_key_path}"
                        )

                    # Delete record
                    await session.execute(delete(Backup).where(Backup.id == backup.id))
                    deleted_count += 1
                except Exception as e:
                    logger.error(f"Failed to delete backup {backup.id}: {e}")

            await session.commit()

            return {
                "kept": len(to_keep),
                "deleted": deleted_count,
            }

    def _select_backups_to_keep(
        self,
        backups: List[Backup],
        policy: RetentionPolicy,
    ) -> set:
        """
        Select which backups to keep based on GFS strategy.
        Returns set of backup IDs to keep.
        """
        now = datetime.now(timezone.utc)
        max_age = now - timedelta(days=policy.max_age_days)

        to_keep = set()

        # Group backups by time periods
        daily: Dict[str, Backup] = {}  # YYYY-MM-DD -> newest backup
        weekly: Dict[str, Backup] = {}  # YYYY-WW -> newest backup
        monthly: Dict[str, Backup] = {}  # YYYY-MM -> newest backup
        yearly: Dict[str, Backup] = {}  # YYYY -> newest backup

        for backup in backups:
            # SQLite stores datetimes without tzinfo; treat them as UTC
            created_at = backup.created_at
            if created_at.tzinfo is None:
                created_at = created_at.replace(tzinfo=timezone.utc)
            if created_at < max_age:
                continue

            created = backup.created_at

            # Daily key
            day_key = created.strftime("%Y-%m-%d")
            if day_key not in daily:
                daily[day_key] = backup

            # Weekly key (ISO week)
            week_key = created.strftime("%Y-W%V")
            if week_key not in weekly:
                weekly[week_key] = backup

            # Monthly key
            month_key = created.strftime("%Y-%m")
            if month_key not in monthly:
                monthly[month_key] = backup

            # Yearly key
            year_key = created.strftime("%Y")
            if year_key not in yearly:
                yearly[year_key] = backup

        # Keep N most recent from each category
        def keep_n_most_recent(backups_dict: Dict[str, Backup], n: int):
            sorted_keys = sorted(backups_dict.keys(), reverse=True)
            for key in sorted_keys[:n]:
                to_keep.add(backups_dict[key].id)

        keep_n_most_recent(daily, policy.keep_daily)
        keep_n_most_recent(weekly, policy.keep_weekly)
        keep_n_most_recent(monthly, policy.keep_monthly)
        keep_n_most_recent(yearly, policy.keep_yearly)

        # Always keep the most recent backup
        if backups:
            to_keep.add(backups[0].id)

        return to_keep

    async def get_retention_stats(self, target_id: int) -> Dict:
        """Get retention statistics for a target."""
        async with async_session() as session:
            result = await session.execute(
                select(Backup)
                .where(
                    Backup.target_id == target_id,
                    Backup.status == BackupStatus.COMPLETED,
                )
                .order_by(Backup.created_at.desc())
            )
            backups = result.scalars().all()

            if not backups:
                return {
                    "total_backups": 0,
                    "total_size_bytes": 0,
                    "oldest_backup": None,
                    "newest_backup": None,
                }

            total_size = sum(b.file_size or 0 for b in backups)

            return {
                "total_backups": len(backups),
                "total_size_bytes": total_size,
                "total_size_human": self._format_size(total_size),
                "oldest_backup": (
                    backups[-1].created_at.isoformat() if backups else None
                ),
                "newest_backup": backups[0].created_at.isoformat() if backups else None,
            }

    def _format_size(self, size_bytes: int) -> str:
        """Format size in human readable format."""
        for unit in ["B", "KB", "MB", "GB", "TB"]:
            if size_bytes < 1024:
                return f"{size_bytes:.2f} {unit}"
            size_bytes /= 1024
        return f"{size_bytes:.2f} PB"

    async def cleanup_orphaned_files(self) -> Dict[str, int]:
        """
        Clean up backup files that don't have corresponding database records.
        """
        backup_dir = Path(settings.BACKUP_BASE_PATH)

        if not backup_dir.exists():
            return {"deleted": 0, "freed_bytes": 0}

        async with async_session() as session:
            # Get all known backup file paths
            result = await session.execute(
                select(Backup.file_path).where(Backup.file_path.isnot(None))
            )
            known_paths = set(row[0] for row in result.fetchall())

        deleted = 0
        freed_bytes = 0

        # Walk through backup directory
        for file_path in backup_dir.rglob("*.tar*"):
            if str(file_path) not in known_paths:
                try:
                    size = file_path.stat().st_size
                    file_path.unlink()
                    deleted += 1
                    freed_bytes += size
                    logger.info(f"Deleted orphaned backup: {file_path}")
                except Exception as e:
                    logger.error(f"Failed to delete orphaned file {file_path}: {e}")

        return {
            "deleted": deleted,
            "freed_bytes": freed_bytes,
            "freed_human": self._format_size(freed_bytes),
        }


# Global retention manager
retention_manager = RetentionManager()
