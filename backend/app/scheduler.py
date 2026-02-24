"""
Backup scheduler using APScheduler.
"""

import asyncio
import logging
from datetime import datetime, timedelta, timezone
from typing import Dict, List, Optional

from apscheduler.jobstores.memory import MemoryJobStore
from apscheduler.schedulers.asyncio import AsyncIOScheduler
from apscheduler.triggers.cron import CronTrigger
from croniter import croniter
from sqlalchemy import select, update
from sqlalchemy.orm import selectinload

from app.backup_engine import BackupType, backup_engine
from app.config import settings
from app.database import BackupSchedule, BackupTarget, async_session
from app.retention import retention_manager
from app.auth import cleanup_expired_sessions

logger = logging.getLogger(__name__)


class BackupScheduler:
    """Manages scheduled backup jobs."""

    def __init__(self):
        self.scheduler = AsyncIOScheduler(
            jobstores={"default": MemoryJobStore()},
            timezone=settings.TZ,
        )
        self._running = False

    async def start(self):
        """Start the scheduler and load existing jobs."""
        if self._running:
            return

        self.scheduler.start()
        self._running = True

        # Load existing schedules from database
        await self._load_schedules()

        # Schedule retention cleanup (daily at 3 AM)
        self.scheduler.add_job(
            self._run_retention_cleanup,
            CronTrigger(hour=3, minute=0),
            id="retention_cleanup",
            replace_existing=True,
        )

        # Schedule expired session cleanup (hourly)
        self.scheduler.add_job(
            self._run_session_cleanup,
            CronTrigger(minute=15),
            id="session_cleanup",
            replace_existing=True,
        )

        logger.info("Backup scheduler started")

    async def stop(self):
        """Stop the scheduler."""
        if self._running:
            self.scheduler.shutdown(wait=False)
            self._running = False
            logger.info("Backup scheduler stopped")

    async def _load_schedules(self):
        """Load all enabled schedules from database."""
        async with async_session() as session:
            # Load targets with schedule relationship
            result = await session.execute(
                select(BackupTarget)
                .where(BackupTarget.enabled.is_(True))
                .where(
                    (BackupTarget.schedule_id.isnot(None))
                    | (BackupTarget.schedule_cron.isnot(None))
                )
                .options(selectinload(BackupTarget.schedule))
            )
            targets = result.scalars().all()

            for target in targets:
                await self.add_schedule(target)

    def _get_target_cron(self, target: BackupTarget) -> Optional[str]:
        """Get the cron expression for a target (from schedule or legacy field)."""
        # Prefer schedule relationship
        if target.schedule and target.schedule.enabled:
            return target.schedule.cron_expression
        # Fall back to legacy schedule_cron
        return target.schedule_cron

    async def add_schedule(self, target: BackupTarget) -> bool:
        """Add or update a backup schedule."""
        cron_expr = self._get_target_cron(target)
        if not cron_expr:
            return False

        job_id = f"backup_target_{target.id}"

        try:
            # Parse cron expression
            trigger = CronTrigger.from_crontab(cron_expr)

            # Add job
            self.scheduler.add_job(
                self._run_scheduled_backup,
                trigger,
                args=[target.id],
                id=job_id,
                replace_existing=True,
                name=f"Backup: {target.name}",
            )

            # Update next run time in database
            next_run = self.get_next_run(cron_expr)
            async with async_session() as session:
                await session.execute(
                    update(BackupSchedule)
                    .where(BackupSchedule.target_id == target.id)
                    .values(next_run=next_run)
                )
                await session.commit()

            logger.info(f"Scheduled backup for target {target.id}: {cron_expr}")
            return True

        except Exception as e:
            logger.error(f"Failed to schedule backup for target {target.id}: {e}")
            return False

    async def remove_schedule(self, target_id: int) -> bool:
        """Remove a backup schedule."""
        job_id = f"backup_target_{target_id}"

        try:
            self.scheduler.remove_job(job_id)
            logger.info(f"Removed schedule for target {target_id}")
            return True
        except Exception as e:
            logger.warning(f"Failed to remove schedule for target {target_id}: {e}")
            return False

    async def _run_scheduled_backup(self, target_id: int):
        """Execute a scheduled backup."""
        logger.info(f"Running scheduled backup for target {target_id}")

        async with async_session() as session:
            result = await session.execute(
                select(BackupTarget)
                .where(BackupTarget.id == target_id)
                .options(selectinload(BackupTarget.schedule))
            )
            target = result.scalar_one_or_none()

            if not target:
                logger.error(f"Target {target_id} not found for scheduled backup")
                return

            if not target.enabled:
                logger.info(f"Target {target_id} is disabled, skipping backup")
                return

            # Check if schedule is enabled (if using schedule relationship)
            if target.schedule and not target.schedule.enabled:
                logger.info(f"Schedule for target {target_id} is disabled, skipping")
                return

        cron_expr = self._get_target_cron(target)

        try:
            # Create and run backup
            backup = await backup_engine.create_backup(target, BackupType.FULL)
            success = await backup_engine.run_backup(backup.id)

            if success:
                # Apply retention policy
                await retention_manager.apply_retention(target_id)
                logger.info(f"Scheduled backup {backup.id} completed successfully")
            else:
                logger.error(f"Scheduled backup {backup.id} failed")

            # Update last run time
            async with async_session() as session:
                await session.execute(
                    update(BackupSchedule)
                    .where(BackupSchedule.target_id == target_id)
                    .values(
                        last_run=datetime.now(timezone.utc),
                        next_run=self.get_next_run(cron_expr) if cron_expr else None,
                    )
                )
                await session.commit()

        except Exception as e:
            logger.error(f"Scheduled backup for target {target_id} failed: {e}")

    async def _run_retention_cleanup(self):
        """Run retention cleanup for all targets."""
        logger.info("Running retention cleanup")

        async with async_session() as session:
            result = await session.execute(select(BackupTarget))
            targets = result.scalars().all()

            for target in targets:
                try:
                    stats = await retention_manager.apply_retention(target.id)
                    if stats.get("deleted", 0) > 0:
                        logger.info(
                            f"Retention cleanup for {target.name}: "
                            f"deleted {stats['deleted']} backups"
                        )
                except Exception as e:
                    logger.error(f"Retention cleanup failed for {target.name}: {e}")

        # Also cleanup orphaned files
        await retention_manager.cleanup_orphaned_files()

    async def _run_session_cleanup(self):
        """Remove expired sessions from the database."""
        async with async_session() as db:
            count = await cleanup_expired_sessions(db)
            if count > 0:
                logger.info("Cleaned up %d expired sessions", count)

    def get_next_run(
        self, cron_expr: str, base_time: Optional[datetime] = None
    ) -> datetime:
        """Get next run time for a cron expression."""
        base = base_time or datetime.now()
        cron = croniter(cron_expr, base)
        return cron.get_next(datetime)

    def get_scheduled_jobs(self) -> List[Dict]:
        """Get all scheduled jobs."""
        jobs = []
        for job in self.scheduler.get_jobs():
            if job.id.startswith("backup_target_"):
                jobs.append(
                    {
                        "id": job.id,
                        "name": job.name,
                        "next_run": (
                            job.next_run_time.isoformat() if job.next_run_time else None
                        ),
                    }
                )
        return jobs

    async def trigger_backup_now(self, target_id: int) -> bool:
        """Trigger a backup immediately."""
        async with async_session() as session:
            result = await session.execute(
                select(BackupTarget).where(BackupTarget.id == target_id)
            )
            target = result.scalar_one_or_none()

            if not target:
                return False

        # Create and run backup in background
        backup = await backup_engine.create_backup(target, BackupType.FULL)
        asyncio.create_task(backup_engine.run_backup(backup.id))

        return True

    def estimate_backup_window(
        self,
        cron_expr: str,
        estimated_duration_seconds: int,
    ) -> Dict:
        """
        Estimate backup window and check for conflicts.
        """
        next_run = self.get_next_run(cron_expr)
        end_time = next_run + timedelta(seconds=estimated_duration_seconds)

        # Check for conflicts with other jobs
        conflicts = []
        for job in self.scheduler.get_jobs():
            if job.id.startswith("backup_target_") and job.next_run_time:
                job_start = job.next_run_time
                if job_start and next_run <= job_start <= end_time:
                    conflicts.append(
                        {
                            "job_id": job.id,
                            "job_name": job.name,
                            "scheduled_time": job_start.isoformat(),
                        }
                    )

        return {
            "start_time": next_run.isoformat(),
            "estimated_end_time": end_time.isoformat(),
            "duration_seconds": estimated_duration_seconds,
            "conflicts": conflicts,
        }
