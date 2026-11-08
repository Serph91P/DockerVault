"""Draft tests exercise the effective configuration without changing the catalog."""

import asyncio
from unittest.mock import AsyncMock

import pytest
from sqlalchemy import select

from app.api import storage as api
from app.credential_encryption import encrypt_value
from app.database import RemoteStorage
from app.remote_storage import RemoteStorageManager, storage_manager


@pytest.mark.parametrize(
    "provider,method",
    [
        ("s3", "_get_client"),
        ("webdav", "_get_session"),
        ("ftp", "_connect"),
        ("rclone", "_run_rclone"),
        ("ssh", "_build_sftp_command"),
    ],
)
async def test_connection_errors_do_not_log_credentials(
    monkeypatch, caplog, provider, method
):
    from app.remote_storage import RemoteStorageManager, StorageConfig, StorageType

    manager = RemoteStorageManager()
    item = manager.add_storage(
        StorageConfig(id=1, name="Synthetic", storage_type=StorageType(provider))
    )
    if provider in ("ssh", "webdav"):

        def fail(*args, **kwargs):
            raise RuntimeError("synthetic-private-endpoint-token")
    else:

        async def fail(*args, **kwargs):
            raise RuntimeError("synthetic-private-endpoint-token")

    monkeypatch.setattr(item, method, fail)
    result = await item.test_connection()
    assert result["success"] is False
    assert "synthetic-private-endpoint-token" not in caplog.text
    await manager.close()


async def test_create_draft_real_local_no_rows(async_client, db_session, tmp_path):
    root = tmp_path / "draft"
    managers = dict(storage_manager.backends)
    response = await async_client.post(
        "/api/v1/storage/test-config",
        json={
            "mode": "create",
            "config": {
                "name": "Draft",
                "storage_type": "local",
                "base_path": str(root),
            },
        },
    )
    assert response.status_code == 200, response.text
    assert response.json()["success"] is True
    assert list(root.iterdir()) == []
    assert (await db_session.execute(select(RemoteStorage))).scalars().all() == []
    assert storage_manager.backends == managers


@pytest.mark.parametrize("secret", [None, "", "replacement"])
async def test_edit_effective_secret_copy(
    async_client, db_session, monkeypatch, secret
):
    row = RemoteStorage(
        name="Draft",
        storage_type="ssh",
        host="example.invalid",
        username="synthetic",
        base_path="/data",
        password=encrypt_value("stored"),
    )
    db_session.add(row)
    await db_session.commit()
    original = row.password
    seen = []
    close = AsyncMock()
    real_add = RemoteStorageManager.add_storage

    def add(manager, config):
        seen.append(config)
        backend = real_add(manager, config)
        backend.test_connection = AsyncMock(
            return_value={"success": True, "message": "private endpoint"}
        )
        return backend

    monkeypatch.setattr(RemoteStorageManager, "add_storage", add)
    monkeypatch.setattr(RemoteStorageManager, "close", close)
    response = await async_client.post(
        "/api/v1/storage/test-config",
        json={
            "mode": "edit",
            "storage_id": row.id,
            "config": {"password": secret, "host": "changed.invalid"},
        },
    )
    assert response.status_code == 200, response.text
    assert seen[0].password == ("stored" if secret == "" else secret)
    assert seen[0].host == "changed.invalid"
    await db_session.refresh(row)
    assert row.password == original
    assert row.host == "example.invalid"
    assert "private endpoint" not in response.text
    close.assert_awaited_once()


@pytest.mark.parametrize(
    "payload",
    [
        {
            "mode": "create",
            "storage_id": 1,
            "config": {"name": "Bad", "storage_type": "local"},
        },
        {"mode": "edit", "storage_id": 0, "config": {}},
        {"mode": "edit", "storage_id": 1, "config": {"storage_type": "s3"}},
        {"mode": "edit", "config": {}},
        {"mode": "other", "config": {}},
    ],
)
async def test_draft_rejects_wrong_shape(async_client, payload):
    response = await async_client.post("/api/v1/storage/test-config", json=payload)
    assert response.status_code == 422


async def test_edit_missing_and_invalid_key(async_client, db_session):
    missing = await async_client.post(
        "/api/v1/storage/test-config",
        json={"mode": "edit", "storage_id": 999, "config": {}},
    )
    assert missing.status_code == 404
    invalid = await async_client.post(
        "/api/v1/storage/test-config",
        json={
            "mode": "create",
            "config": {
                "name": "Key",
                "storage_type": "ssh",
                "host": "example.invalid",
                "username": "synthetic",
                "ssh_key_path": "/outside/key",
            },
        },
    )
    assert invalid.status_code == 400


@pytest.mark.parametrize(
    "failure", [RuntimeError("private credential"), asyncio.CancelledError()]
)
async def test_private_manager_closed_on_failure_or_cancel(monkeypatch, failure):
    manager = RemoteStorageManager()
    backend = AsyncMock()
    backend.test_connection.side_effect = failure
    manager.add_storage = lambda config: backend
    manager.close = AsyncMock()
    monkeypatch.setattr(api, "RemoteStorageManager", lambda: manager)
    data = api.StorageCreateTest(
        mode="create", config={"name": "Test", "storage_type": "local"}
    )
    if isinstance(failure, asyncio.CancelledError):
        with pytest.raises(asyncio.CancelledError):
            await api.test_storage_config(data, AsyncMock())
    else:
        result = await api.test_storage_config(data, AsyncMock())
        assert not result.success
        assert "private" not in result.message
    manager.close.assert_awaited_once()
