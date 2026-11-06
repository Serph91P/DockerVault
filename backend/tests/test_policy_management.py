"""Policy management regressions: real file effects and persisted constraints."""

from pathlib import Path
from unittest.mock import patch

import pytest
from fastapi import HTTPException

from app.database import Backup, BackupTarget, RetentionPolicy
from app.retention import RetentionManager
from app.api.retention import delete_retention_policy


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "failed,expected", [(0, "success"), (1, "partial"), (2, "failure")]
)
async def test_cleanup_reports_actual_failed_deletes(
    test_db, tmp_path, failed, expected
):
    owned = tmp_path / "owned.tar.gz"
    owned.write_bytes(b"owned")
    orphans = [tmp_path / f"orphan{i}.tar.gz" for i in range(2)]
    for path in orphans:
        path.write_bytes(b"orphan")
    async with test_db() as session:
        target = BackupTarget(name="catalog", target_type="path")
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id, file_path=str(owned))
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    unlink = Path.unlink

    def fault(path, *args, **kwargs):
        # Explicit injected filesystem faults; catalog and actual successful unlinks are real.
        if path in orphans[:failed]:
            raise PermissionError("synthetic unlink failure")
        return unlink(path, *args, **kwargs)

    with (
        patch("app.retention.async_session", test_db),
        patch("app.retention.settings.BACKUP_BASE_PATH", str(tmp_path)),
        patch.object(Path, "unlink", fault),
    ):
        result = await RetentionManager().cleanup_orphaned_files()
    assert result["outcome"] == expected
    assert result["deleted"] == 2 - failed
    assert result["failed"] == failed
    assert result["freed_bytes"] == (2 - failed) * 6
    assert result["scan_complete"] is True
    assert result["protected"] == 1
    assert owned.read_bytes() == b"owned"
    assert [p.exists() for p in orphans] == [i < failed for i in range(2)]
    async with test_db() as session:
        assert (await session.get(Backup, backup_id)).file_path == str(owned)


@pytest.mark.asyncio
@pytest.mark.parametrize("exists", [False, True])
async def test_cleanup_empty_is_distinct_from_failure(test_db, tmp_path, exists):
    root = tmp_path / "backups"
    if exists:
        root.mkdir()
    with (
        patch("app.retention.async_session", test_db),
        patch("app.retention.settings.BACKUP_BASE_PATH", str(root)),
    ):
        result = await RetentionManager().cleanup_orphaned_files()
    assert result["outcome"] == "empty"
    assert result["failed"] == result["deleted"] == 0
    assert result["scan_complete"] is True


@pytest.mark.asyncio
async def test_cleanup_inaccessible_root_is_failure_not_empty(test_db, tmp_path):
    root = tmp_path / "inaccessible"
    root.mkdir()
    stat = Path.stat

    def denied(path, *args, **kwargs):
        if path == root:
            raise PermissionError("synthetic scan failure")
        return stat(path, *args, **kwargs)

    with (
        patch("app.retention.async_session", test_db),
        patch("app.retention.settings.BACKUP_BASE_PATH", str(root)),
        patch.object(Path, "stat", denied),
    ):
        result = await RetentionManager().cleanup_orphaned_files()
    assert result["outcome"] == "failure"
    assert result["scan_complete"] is False
    assert result["scan_errors"] == 1
    assert result["deleted"] == result["failed"] == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("removed", [False, True])
async def test_retention_delete_rejects_all_fk_assignments_without_changing_rows(
    test_db, removed
):
    from datetime import datetime, timezone

    async with test_db() as session:
        policy = RetentionPolicy(name="assigned")
        session.add(policy)
        await session.flush()
        target = BackupTarget(
            name="plan",
            target_type="path",
            retention_policy_id=policy.id,
            removed_at=datetime.now(timezone.utc) if removed else None,
        )
        session.add(target)
        await session.commit()
        policy_id, target_id = policy.id, target.id
    with patch("app.api.retention.async_session", test_db):
        with pytest.raises(HTTPException) as error:
            await delete_retention_policy(policy_id)
    assert error.value.status_code == 409
    async with test_db() as session:
        assert await session.get(RetentionPolicy, policy_id)
        assert (
            await session.get(BackupTarget, target_id)
        ).retention_policy_id == policy_id
