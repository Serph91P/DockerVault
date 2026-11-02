"""Retention promises keep-last recovery points independently of age/buckets."""

from datetime import datetime, timedelta, timezone
from unittest.mock import patch

import pytest
from sqlalchemy import select

from app.database import Backup, BackupStatus, BackupTarget, RetentionPolicy
from app.retention import RetentionManager


@pytest.mark.asyncio
@pytest.mark.parametrize("age_days", [0, 400])
async def test_keep_last_preserves_three_real_files_and_rows(
    test_db, tmp_path, age_days
):
    now = datetime.now(timezone.utc)
    async with test_db() as session:
        policy = RetentionPolicy(
            name="keep-three",
            keep_last=3,
            keep_daily=0,
            keep_weekly=0,
            keep_monthly=0,
            keep_yearly=0,
            max_age_days=365,
        )
        target = BackupTarget(
            name="retention", target_type="path", retention_policy=policy
        )
        session.add(target)
        await session.flush()
        paths = []
        for index in range(4):
            path = tmp_path / f"{index}.tar"
            path.write_bytes(f"snapshot-{index}".encode())
            paths.append(path)
            session.add(
                Backup(
                    target_id=target.id,
                    status=BackupStatus.COMPLETED,
                    file_path=str(path),
                    created_at=now - timedelta(days=age_days, minutes=index),
                )
            )
        await session.commit()
        target_id = target.id
    with patch("app.retention.async_session", test_db):
        result = await RetentionManager().apply_retention(target_id)
    assert result == {"kept": 3, "deleted": 1}
    assert [path.exists() for path in paths] == [True, True, True, False]
    assert [path.read_bytes() for path in paths[:3]] == [
        b"snapshot-0",
        b"snapshot-1",
        b"snapshot-2",
    ]
    async with test_db() as session:
        assert (
            await session.execute(
                select(Backup.file_path).order_by(Backup.created_at.desc())
            )
        ).scalars().all() == [str(path) for path in paths[:3]]


@pytest.mark.parametrize(
    "keep_last,keep_daily,expected",
    [(2, 2, {1, 2, 3}), (0, 0, {1}), (10, 0, {1, 2, 3, 4})],
)
def test_keep_last_unions_calendar_and_preserves_keep_one_fallback(
    keep_last, keep_daily, expected
):
    now = datetime.now(timezone.utc)
    backups = [
        Backup(id=index + 1, created_at=now - timedelta(hours=hours))
        for index, hours in enumerate([0, 1, 25, 49])
    ]
    policy = RetentionPolicy(
        keep_last=keep_last,
        keep_daily=keep_daily,
        keep_weekly=0,
        keep_monthly=0,
        keep_yearly=0,
        max_age_days=365,
    )
    assert RetentionManager()._select_backups_to_keep(backups, policy) == expected
