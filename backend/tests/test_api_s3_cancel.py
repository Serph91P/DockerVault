"""Real S3 multipart abort must remain reachable through the shared sync API."""

import asyncio
from unittest.mock import patch

import pytest
from sqlalchemy import select

from app.config import settings
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
)
from app.remote_storage import S3Storage, storage_manager
from tests import test_s3_protocol
from tests.test_remote_object_ownership import test_engine  # noqa: F401

s3_service = test_s3_protocol.s3_service


@pytest.mark.asyncio
@pytest.mark.parametrize("caller", ["provider", "api"])
async def test_caller_cancellation_reaches_s3_abort(
    caller, s3_service, async_client, test_db, tmp_path, monkeypatch
):
    backend, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    monkeypatch.setattr(S3Storage, "MULTIPART_CLEANUP_TIMEOUT", 0.2)
    source = tmp_path / "snapshot.tar"
    source.write_bytes(b"x" * (6 * 1024 * 1024))
    async with test_db() as session:
        storage = RemoteStorage(
            name="owned-s3-integration",
            storage_type="s3",
            base_path="backups",
            s3_bucket="owned-bucket",
            s3_endpoint_url=backend.config.s3_endpoint_url,
        )
        target = BackupTarget(
            name="owned-target", target_type="path", host_path=str(tmp_path)
        )
        session.add_all([storage, target])
        await session.flush()
        backup = Backup(
            target_id=target.id, status=BackupStatus.COMPLETED, file_path=str(source)
        )
        session.add(backup)
        await session.commit()
        sid, bid = storage.id, backup.id
    assert sid == backend.config.id
    state.mode = "hold"
    with (
        patch.dict(storage_manager.backends, {sid: backend}, clear=True),
        patch.dict(storage_manager.configs, {sid: backend.config}, clear=True),
        patch("app.config.settings.BACKUP_BASE_PATH", str(tmp_path)),
    ):
        operation = (
            backend.upload(source, "owned-target/snapshot.tar")
            if caller == "provider"
            else async_client.post(f"/api/v1/storage/{sid}/sync/{bid}")
        )
        task = asyncio.create_task(operation)
        try:
            await asyncio.wait_for(state.entered.wait(), 5)
            assert state.uploads and not task.done()
            task.cancel()
            done, _ = await asyncio.wait([task], timeout=2)
            abort_sent = state.abort_entered.is_set()
            remaining_uploads, objects_before_release = (
                list(state.uploads),
                list(state.objects),
            )
        finally:
            state.release.set()
            state.abort_release.set()
            _, pending = await asyncio.wait([task], timeout=10)
            assert not pending
            outcomes = await asyncio.gather(task, return_exceptions=True)
        async with test_db() as session:
            rows = list((await session.scalars(select(BackupStorageSync))).all())
        assert abort_sent, "Caller cancellation was swallowed before multipart abort"
        assert done and isinstance(outcomes[0], asyncio.CancelledError)
        assert not remaining_uploads and not objects_before_release
        assert not state.uploads and not state.objects
        assert not any(row.sync_status == "completed" for row in rows)
