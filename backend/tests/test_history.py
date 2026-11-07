"""History queries use the full catalog, not filesystem observations on a page."""

from datetime import datetime, timezone

import pytest

from app.database import (
    Backup,
    BackupStatus,
    BackupTarget,
    BackupType,
    BackupStorageSync,
    RemoteStorage,
)


@pytest.mark.asyncio
async def test_history_pages_full_catalog_with_stable_insert_boundary(
    async_client, db_session
):
    target = BackupTarget(
        name="Retained source",
        target_type="path",
        host_path="/fixture",
        removed_at=datetime.now(timezone.utc),
    )
    db_session.add(target)
    await db_session.flush()
    stamp = datetime(2025, 1, 1)
    rows = [
        Backup(
            target_id=target.id,
            backup_type=BackupType.FULL,
            status=BackupStatus.COMPLETED,
            file_path=f"/missing/archive-{i}.tar",
            file_size=i,
            created_at=stamp,
        )
        for i in range(507)
    ]
    db_session.add_all(rows)
    await db_session.commit()
    first = await async_client.get("/api/v1/backups/history", params={"limit": 100})
    assert first.status_code == 200
    page = first.json()
    assert page["total"] == 507
    assert page["items"][0]["id"] == rows[-1].id
    assert page["items"][0]["plan_removed"] is True
    snapshot = page["snapshot"]
    db_session.add(Backup(target_id=target.id, status=BackupStatus.FAILED))
    await db_session.commit()
    seen = []
    for offset in range(0, 507, 100):
        result = await async_client.get(
            "/api/v1/backups/history",
            params={"limit": 100, "offset": offset, "snapshot": snapshot},
        )
        assert result.status_code == 200
        value = result.json()
        assert value["total"] == 507
        seen.extend(row["id"] for row in value["items"])
    assert seen == [row.id for row in reversed(rows)]
    search = (
        await async_client.get(
            "/api/v1/backups/history", params={"q": "archive-0.tar", "view": "archives"}
        )
    ).json()
    assert search["total"] == 1
    assert search["items"][0]["id"] == rows[0].id
    assert search["items"][0]["local_state"] == "missing"
    assert (
        await async_client.get("/api/v1/backups/history", params={"q": "no match"})
    ).json()["total"] == 0
    assert isinstance((await async_client.get("/api/v1/backups")).json(), list)


@pytest.mark.asyncio
async def test_history_filters_archive_identity_and_details(
    async_client, db_session, tmp_path
):
    target = BackupTarget(name="Source", target_type="path", host_path="/fixture")
    remote = RemoteStorage(
        name="Offsite", storage_type="local", base_path=str(tmp_path), enabled=True
    )
    db_session.add_all([target, remote])
    await db_session.flush()
    path = tmp_path / "real.tar"
    path.write_bytes(b"fixture")
    good = Backup(
        target_id=target.id,
        status=BackupStatus.COMPLETED,
        file_path=str(path),
        file_size=7,
        encrypted=True,
    )
    failed = Backup(
        target_id=target.id,
        status=BackupStatus.FAILED,
        error_message="No archive created",
    )
    db_session.add_all([good, failed])
    await db_session.flush()
    db_session.add(
        BackupStorageSync(backup_id=good.id, storage_id=remote.id, sync_status="failed")
    )
    await db_session.commit()
    archives = (
        await async_client.get(
            "/api/v1/backups/history",
            params={
                "view": "archives",
                "source": "path",
                "location": "local",
                "sort": "size",
                "direction": "asc",
            },
        )
    ).json()
    assert archives["total"] == 1
    assert archives["items"][0]["id"] == good.id
    detail = await async_client.get(f"/api/v1/backups/history/{good.id}")
    assert detail.status_code == 200
    record = detail.json()
    assert record["local_state"] == "available"
    assert record["archive_state"] == "recorded"
    assert record["remote_sync_details"][0]["storage_name"] == "Offsite"
    assert record["can_sync"] is True
    assert record["source_type"] == "path"
    assert record["source_identity"] == "/fixture"
    assert (
        await async_client.get("/api/v1/backups/history", params={"q": "/fixture"})
    ).json()["total"] == 2
    no_archive = (await async_client.get(f"/api/v1/backups/history/{failed.id}")).json()
    assert no_archive["archive_state"] == "never_created"
    assert no_archive["can_restore"] is False
    db_session.add(
        BackupStorageSync(
            backup_id=failed.id, storage_id=remote.id, sync_status="failed"
        )
    )
    await db_session.commit()
    failed_sync_only = (
        await async_client.get(f"/api/v1/backups/history/{failed.id}")
    ).json()
    assert failed_sync_only["archive_state"] == "never_created"
    assert (
        await async_client.get("/api/v1/backups/history", params={"view": "archives"})
    ).json()["total"] == 1
    failed.file_path = str(path)
    failed.file_size = 7
    await db_session.commit()
    failed_artifact = (
        await async_client.get(f"/api/v1/backups/history/{failed.id}")
    ).json()
    assert (
        failed_artifact["can_restore"] is False
    )  # Native browse/restore require a completed run.
    assert (await async_client.get("/api/v1/backups/history/999999")).status_code == 404
    for params in (
        {"source": "unknown"},
        {"limit": 1001},
        {"sort": "bad"},
        {"after": "invalid"},
        {"location": "../bad"},
    ):
        assert (
            await async_client.get("/api/v1/backups/history", params=params)
        ).status_code == 422
    assert (
        await async_client.get(
            "/api/v1/backups/history", params={"before": "2000-01-01T00:00:00Z"}
        )
    ).json()["total"] == 0
    assert (
        await async_client.get("/api/v1/backups/history", params={"status": "failed"})
    ).json()["total"] == 1
    assert (
        await async_client.get(
            "/api/v1/backups/history", params={"location": str(remote.id)}
        )
    ).json()[
        "total"
    ] == 2  # Both the failed attempt and the archive name this destination.
    good.file_path = None
    remote.enabled = False
    sync = await db_session.get(BackupStorageSync, 1)
    sync.sync_status = "completed"
    await db_session.commit()
    unavailable = (await async_client.get(f"/api/v1/backups/history/{good.id}")).json()
    assert unavailable["can_restore"] is False
    assert unavailable["remote_sync_details"][0]["availability"] == "inaccessible"
