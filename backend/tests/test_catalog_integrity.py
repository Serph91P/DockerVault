"""Catalog identity must outlive a local file and never leak across reused IDs."""

from datetime import datetime, timedelta, timezone
from unittest.mock import patch

import pytest
from sqlalchemy import delete, func, select, text

from app.database import (
    Backup,
    BackupLog,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    RetentionPolicy,
)


@pytest.fixture
async def catalog(test_db, tmp_path):
    archive = tmp_path / "original.tar"
    archive.write_bytes(b"synthetic archive")
    key = tmp_path / "original.key"
    key.write_bytes(b"synthetic sidecar")
    remote = tmp_path / "remote" / "original" / "archive.tar"
    remote.parent.mkdir(parents=True)
    remote.write_bytes(archive.read_bytes())
    async with test_db() as session:
        target = BackupTarget(
            name="catalog", target_type="path", host_path=str(tmp_path)
        )
        storage = RemoteStorage(
            name="catalog storage",
            storage_type="local",
            base_path=str(tmp_path / "remote"),
        )
        session.add_all([target, storage])
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(archive),
            encrypted=True,
            encryption_key_path=str(key),
        )
        session.add(backup)
        await session.flush()
        session.add_all(
            [
                BackupLog(backup_id=backup.id, step="complete", message="original log"),
                BackupStorageSync(
                    backup_id=backup.id,
                    storage_id=storage.id,
                    remote_path="original/archive.tar",
                    sync_status="completed",
                ),
            ]
        )
        await session.commit()
        return backup.id, target.id, storage.id, archive, key


@pytest.mark.asyncio
async def test_all_sqlite_connections_enforce_foreign_keys(test_engine):
    async with test_engine.connect() as first, test_engine.connect() as second:
        for connection in (first, second):
            assert (
                await connection.execute(text("PRAGMA foreign_keys"))
            ).scalar_one() == 1


@pytest.mark.asyncio
async def test_bulk_sql_delete_cascades_logs_and_syncs(test_db, catalog):
    backup_id, target_id, _, _, _ = catalog
    async with test_db() as session:
        await session.execute(delete(Backup).where(Backup.id == backup_id))
        await session.commit()
        session.add(Backup(target_id=target_id))
        await session.commit()
        assert (
            await session.execute(select(func.count()).select_from(BackupLog))
        ).scalar_one() == 0
        assert (
            await session.execute(select(func.count()).select_from(BackupStorageSync))
        ).scalar_one() == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("bulk", [False, True])
async def test_local_delete_preserves_remote_recovery_catalog(
    async_client, test_db, catalog, bulk
):
    backup_id, target_id, _, archive, key = catalog
    endpoint = (
        f"/api/v1/backups?target_id={target_id}&"
        if bulk
        else f"/api/v1/backups/{backup_id}?"
    )
    with patch("app.api.backups.settings.BACKUP_BASE_PATH", str(archive.parent)):
        response = await async_client.delete(endpoint + "delete_remote=false")
    assert response.status_code == 200
    async with test_db() as session:
        backup = await session.get(Backup, backup_id)
        assert backup is not None
        assert backup.file_path is None
        assert (
            await session.execute(select(BackupStorageSync.remote_path))
        ).scalar_one() == "original/archive.tar"
    assert not archive.exists()
    assert key.exists()
    assert (
        archive.parent / "remote/original/archive.tar"
    ).read_bytes() == b"synthetic archive"
    retained = await async_client.get(f"/api/v1/backups/{backup_id}")
    assert retained.status_code == 200
    assert retained.json()["remote_synced"] is True
    # Exercise the real retained-copy download adapter, not only JSON discovery.
    import shutil
    from pathlib import Path
    from app.api.backups import _ensure_backup_locally
    from app.remote_storage import storage_manager

    with (
        patch.dict(storage_manager.backends, {}, clear=True),
        patch.dict(storage_manager.configs, {}, clear=True),
    ):
        downloaded, temporary = await _ensure_backup_locally(backup)
        try:
            assert Path(downloaded).read_bytes() == b"synthetic archive"
        finally:
            if temporary:
                shutil.rmtree(temporary)
    async with test_db() as session:
        fresh = Backup(target_id=target_id)
        session.add(fresh)
        await session.commit()
        fresh_id = fresh.id
    detail = await async_client.get(f"/api/v1/backups/{fresh_id}")
    logs = await async_client.get(f"/api/v1/backups/{fresh_id}/logs")
    assert detail.status_code == logs.status_code == 200
    assert {"remote_synced": detail.json()["remote_synced"], "logs": logs.json()} == {
        "remote_synced": False,
        "logs": [],
    }


@pytest.mark.asyncio
async def test_target_delete_rejects_loss_of_remote_catalog(
    async_client, test_db, catalog
):
    backup_id, target_id, _, archive, _ = catalog
    response = await async_client.delete(f"/api/v1/targets/{target_id}")
    assert response.status_code == 409
    assert archive.exists()
    async with test_db() as session:
        assert await session.get(Backup, backup_id) is not None


@pytest.mark.asyncio
async def test_storage_delete_rejects_loss_of_remote_catalog(
    async_client, test_db, catalog
):
    _, _, storage_id, _, _ = catalog
    response = await async_client.delete(f"/api/v1/storage/{storage_id}")
    assert response.status_code == 409
    async with test_db() as session:
        assert await session.get(RemoteStorage, storage_id) is not None


@pytest.mark.asyncio
async def test_retention_preserves_remote_recovery_catalog(test_db, catalog):
    from app.retention import RetentionManager

    backup_id, target_id, _, archive, key = catalog
    async with test_db() as session:
        policy = RetentionPolicy(
            name="none",
            keep_last=0,
            keep_daily=0,
            keep_weekly=0,
            keep_monthly=0,
            keep_yearly=0,
        )
        session.add(policy)
        await session.flush()
        target = await session.get(BackupTarget, target_id)
        target.retention_policy_id = policy.id
        original = await session.get(Backup, backup_id)
        original.created_at = datetime.now(timezone.utc) - timedelta(days=2)
        session.add(Backup(target_id=target_id, status=BackupStatus.COMPLETED))
        await session.commit()
    with patch("app.retention.async_session", test_db):
        await RetentionManager().apply_retention(target_id)
    async with test_db() as session:
        backup = await session.get(Backup, backup_id)
        assert backup is not None
        assert backup.file_path is None
        assert (
            await session.execute(select(func.count()).select_from(BackupStorageSync))
        ).scalar_one() == 1
    assert not archive.exists()
    assert key.exists()


@pytest.mark.asyncio
async def test_remote_delete_without_backend_keeps_recovery_identity(
    async_client, test_db, catalog
):
    backup_id, _, _, archive, key = catalog
    with patch("app.api.backups.settings.BACKUP_BASE_PATH", str(archive.parent)):
        response = await async_client.delete(f"/api/v1/backups/{backup_id}")
    assert response.status_code == 200
    assert "warning" in response.json()
    async with test_db() as session:
        assert await session.get(Backup, backup_id) is not None
        assert (
            await session.execute(select(func.count()).select_from(BackupStorageSync))
        ).scalar_one() == 1
    assert key.exists()
    assert (archive.parent / "remote/original/archive.tar").exists()


@pytest.mark.asyncio
async def test_legacy_orphan_metadata_is_quarantined_before_id_reuse(test_engine):
    import json
    from app import database

    async with test_engine.connect() as connection:
        await connection.execute(text("PRAGMA foreign_keys=OFF"))
        await connection.execute(
            text(
                "INSERT INTO backup_logs (id, backup_id, step, message) VALUES (91, 777, 'complete', 'orphan log')"
            )
        )
        await connection.execute(
            text(
                "INSERT INTO backup_storage_syncs (id, backup_id, storage_id, remote_path, sync_status) VALUES (92, 777, 888, 'orphan/archive.tar', 'completed')"
            )
        )
        await connection.commit()
        await connection.execute(text("PRAGMA foreign_keys=ON"))
        await connection.commit()
    with patch.object(database, "engine", test_engine):
        await database.run_migrations()
        await database.run_migrations()
    async with test_engine.connect() as connection:
        names = (
            (
                await connection.execute(
                    text("SELECT name FROM sqlite_master WHERE type='table'")
                )
            )
            .scalars()
            .all()
        )
        assert "catalog_quarantine" in names
        rows = (
            await connection.execute(
                text(
                    "SELECT table_name, row_json FROM catalog_quarantine ORDER BY table_name"
                )
            )
        ).all()
        assert [(table, json.loads(data)["backup_id"]) for table, data in rows] == [
            ("backup_logs", 777),
            ("backup_storage_syncs", 777),
        ]
        assert json.loads(rows[1][1])["remote_path"] == "orphan/archive.tar"
        assert (await connection.execute(text("PRAGMA foreign_key_check"))).all() == []
        assert (
            await connection.execute(text("SELECT count(*) FROM backup_logs"))
        ).scalar_one() == 0
        assert (
            await connection.execute(text("SELECT count(*) FROM backup_storage_syncs"))
        ).scalar_one() == 0
