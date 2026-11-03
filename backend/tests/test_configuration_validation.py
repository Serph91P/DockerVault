"""Validate effective configuration before SQLite/adapter mutations."""

import pytest
from sqlalchemy import select

from app.database import BackupTarget, RemoteStorage as RemoteStorageModel
from app.remote_storage import RemoteStorageManager
from tests.test_remote_object_ownership import test_engine  # noqa: F401

VALID = {
    "local": {"base_path": "/owned"},
    "ssh": {"host": "backup.example.test", "username": "owned", "base_path": "/owned"},
    "sftp": {"host": "backup.example.test", "username": "owned", "base_path": "/owned"},
    "ftp": {
        "host": "backup.example.test",
        "username": "owned",
        "password": "synthetic-only",
        "base_path": "/owned",
    },
    "webdav": {"webdav_url": "https://backup.example.test/dav", "base_path": "/owned"},
    "s3": {
        "s3_bucket": "owned-bucket",
        "s3_access_key": "synthetic-only",
        "s3_secret_key": "synthetic-only",
    },
    "rclone": {"rclone_remote": "owned", "base_path": "/owned"},
}
REQUIRED = [(kind, field) for kind, fields in VALID.items() for field in fields]


@pytest.fixture
def registry(monkeypatch):
    from app.api import storage

    manager = RemoteStorageManager()
    monkeypatch.setattr(storage, "storage_manager", manager)
    return manager


@pytest.mark.asyncio
@pytest.mark.parametrize("kind,field", REQUIRED)
@pytest.mark.parametrize("operation", ["create", "update"])
async def test_every_provider_required_field_before_commit(
    async_client, test_db, registry, kind, field, operation
):
    payload = {"name": "owned", "storage_type": kind, **VALID[kind]}
    if operation == "create":
        payload[field] = ""
        response = await async_client.post("/api/v1/storage", json=payload)
        assert 400 <= response.status_code < 500
        async with test_db() as session:
            assert (
                await session.execute(select(RemoteStorageModel))
            ).scalars().all() == []
        assert registry.backends == {}
    else:
        created = await async_client.post("/api/v1/storage", json=payload)
        assert created.status_code == 200
        sid = created.json()["id"]
        before = (await async_client.get(f"/api/v1/storage/{sid}")).json()
        backend = registry.get_backend(sid)
        # Credentials retain existing blank-on-edit semantics; explicit null is a clear.
        response = await async_client.put(
            f"/api/v1/storage/{sid}", json={field: None, "name": "must-not-persist"}
        )
        assert 400 <= response.status_code < 500
        assert (await async_client.get(f"/api/v1/storage/{sid}")).json() == before
        assert registry.get_backend(sid) is backend


@pytest.mark.asyncio
@pytest.mark.parametrize("port", [-3, 0, 65536])
async def test_port_bounds_rejected_without_adapter_mutation(
    async_client, registry, port
):
    response = await async_client.post(
        "/api/v1/storage",
        json={"name": "owned", "storage_type": "ftp", **VALID["ftp"], "port": port},
    )
    assert response.status_code == 422
    assert registry.backends == {}


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", list(VALID))
async def test_valid_providers_and_partial_update_preserve_optional_fields(
    async_client, registry, kind
):
    created = await async_client.post(
        "/api/v1/storage",
        json={"name": "owned", "storage_type": kind, **VALID[kind], "port": 65535},
    )
    assert created.status_code == 200
    sid = created.json()["id"]
    response = await async_client.put(
        f"/api/v1/storage/{sid}", json={"name": "renamed", "enabled": False}
    )
    assert response.status_code == 200
    assert response.json()["name"] == "renamed" and not response.json()["enabled"]
    assert response.json()["port"] == 65535
    assert response.json()["validation_errors"] == []
    assert registry.configs[sid].port == 65535


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "reference",
    [{"retention_policy_id": 999}, {"remote_storage_ids": [999]}, {"schedule_id": 999}],
)
@pytest.mark.parametrize("operation", ["create", "update"])
async def test_new_dangling_target_references_rejected_atomically(
    async_client, test_db, reference, operation, monkeypatch
):
    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    payload = {"name": "owned", "target_type": "volume", "volume_name": "owned"}
    if operation == "create":
        response = await async_client.post(
            "/api/v1/targets", json={**payload, **reference}
        )
        assert 400 <= response.status_code < 500
        async with test_db() as session:
            assert (await session.execute(select(BackupTarget))).scalars().all() == []
    else:
        created = await async_client.post("/api/v1/targets", json=payload)
        assert created.status_code == 200
        tid = created.json()["id"]
        before = created.json()
        response = await async_client.put(
            f"/api/v1/targets/{tid}", json={"name": "must-not-persist", **reference}
        )
        assert 400 <= response.status_code < 500
        assert (await async_client.get(f"/api/v1/targets/{tid}")).json() == before


@pytest.mark.asyncio
async def test_legacy_invalid_records_are_readable_with_repair_guidance(
    async_client, test_db, registry
):
    async with test_db() as session:
        storage = RemoteStorageModel(name="legacy-invalid", storage_type="s3", port=-3)
        target = BackupTarget(
            name="legacy-target",
            target_type="volume",
            volume_name="owned",
            remote_storage_ids=[999],
        )
        session.add_all([storage, target])
        await session.commit()
        sid, tid = storage.id, target.id
    listed = await async_client.get("/api/v1/storage")
    assert listed.status_code == 200
    assert listed.json()[0]["validation_errors"]
    assert (await async_client.get(f"/api/v1/storage/{sid}")).json()[
        "validation_errors"
    ]
    assert (await async_client.get(f"/api/v1/targets/{tid}")).json()[
        "validation_errors"
    ]
    assert (await async_client.get("/api/v1/targets")).json()[0]["validation_errors"]
    assert registry.backends == {}


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "kind,field", [("webdav", "webdav_url"), ("s3", "s3_endpoint_url")]
)
@pytest.mark.parametrize(
    "url",
    [
        "http://[",
        "https://[::1",
        "https://example.test:invalid",
        "https://example.test:65536",
    ],
)
@pytest.mark.parametrize("operation", ["create", "update"])
async def test_malformed_provider_urls_are_atomic_4xx(
    async_client, test_db, registry, monkeypatch, kind, field, url, operation
):
    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    payload = {"name": "owned-url", "storage_type": kind, **VALID[kind]}
    if operation == "create":
        response = await async_client.post(
            "/api/v1/storage", json={**payload, field: url}
        )
        assert 400 <= response.status_code < 500, response.text
        async with test_db() as session:
            assert (
                await session.execute(select(RemoteStorageModel))
            ).scalars().all() == []
        assert registry.backends == {}
    else:
        created = await async_client.post("/api/v1/storage", json=payload)
        assert created.status_code == 200
        sid = created.json()["id"]
        before = (await async_client.get(f"/api/v1/storage/{sid}")).json()
        backend = registry.get_backend(sid)
        response = await async_client.put(
            f"/api/v1/storage/{sid}", json={field: url, "name": "must-not-persist"}
        )
        assert 400 <= response.status_code < 500, response.text
        assert (await async_client.get(f"/api/v1/storage/{sid}")).json() == before
        assert registry.get_backend(sid) is backend


@pytest.mark.asyncio
async def test_unknown_backup_status_is_deliberate_4xx(async_client, monkeypatch):
    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    response = await async_client.get("/api/v1/backups", params={"status": "unknown"})
    assert response.status_code == 422
    for valid in ("completed", "running", "pending", "failed", "cancelled"):
        assert (
            await async_client.get("/api/v1/backups", params={"status": valid})
        ).status_code == 200
