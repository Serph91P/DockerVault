"""Komodo settings, status and real protocol calls must use one durable config."""

from contextlib import ExitStack
import asyncio
import json
import os
import sys
from unittest.mock import patch

from aiohttp import web
import pytest
import pytest_asyncio
from sqlalchemy import select, text

from app.config import settings
from app.database import AppSettings
from app.komodo import KomodoClient
from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest_asyncio.fixture
async def komodo_service(async_client, test_db, monkeypatch, tmp_path):
    from app import credential_encryption
    from app.api import settings as api_settings, komodo as api_komodo
    from cryptography.fernet import Fernet

    # Keep credential encryption hermetic: an explicit synthetic key instead of
    # the production /app/data key file, which is absent on the CI runner.
    synthetic_key = Fernet.generate_key().decode()
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    monkeypatch.setattr(
        credential_encryption, "_KEY_FILE_PATH", tmp_path / ".credential_key"
    )
    monkeypatch.setattr(settings, "CREDENTIAL_ENCRYPTION_KEY", synthetic_key)

    calls = []

    async def respond(request):
        body = await request.json()
        calls.append(
            (
                request.path,
                body["type"],
                request.headers.get("X-Api-Key"),
                request.headers.get("X-Api-Secret"),
            )
        )
        return web.json_response({"version": "synthetic-1"})

    application = web.Application()
    application.router.add_post("/{route}", respond)
    runner = web.AppRunner(application)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    url = f"http://127.0.0.1:{site._server.sockets[0].getsockname()[1]}"
    monkeypatch.setattr(settings, "KOMODO_API_URL", url)
    monkeypatch.setattr(settings, "KOMODO_API_KEY", "synthetic-env-key")
    monkeypatch.setattr(settings, "KOMODO_API_SECRET", "synthetic-env-secret")
    monkeypatch.setattr(settings, "KOMODO_ENABLED", False)
    client = KomodoClient()
    monkeypatch.setattr(api_komodo, "komodo_client", client)
    monkeypatch.setattr("app.komodo.komodo_client", client)
    with ExitStack() as stack:
        stack.enter_context(patch.object(api_settings, "async_session", test_db))
        # The new shared reader uses the same real file-backed SQLite factory.
        stack.enter_context(patch("app.database.async_session", test_db))
        yield url, calls, client
    await client.close()
    await runner.cleanup()


@pytest.mark.asyncio
async def test_save_test_status_and_lifecycle_share_effective_config(
    async_client, test_db, komodo_service
):
    url, calls, client = komodo_service
    saved = await async_client.put(
        "/api/v1/settings/komodo", json={"enabled": True, "api_key": "synthetic-db-key"}
    )
    assert saved.status_code == 200
    assert saved.json()["enabled"] and saved.json()["has_api_key"]
    status = await async_client.get("/api/v1/komodo/status")
    assert status.json() == {"enabled": True, "api_url": url, "connected": True}
    assert (await async_client.post("/api/v1/settings/komodo/test")).json()["success"]
    assert (await async_client.post("/api/v1/komodo/test")).status_code == 200
    assert (
        await async_client.post(
            "/api/v1/komodo/container-action",
            json={"container_name": "owned-synthetic", "action": "stop"},
        )
    ).status_code == 200
    assert (
        await async_client.post(
            "/api/v1/komodo/container-action",
            json={"container_name": "owned-synthetic", "action": "start"},
        )
    ).status_code == 200
    assert {"GetVersion", "StopContainer", "StartContainer"} <= {c[1] for c in calls}
    assert all(c[2:] == ("synthetic-db-key", "synthetic-env-secret") for c in calls)
    assert client.enabled
    async with test_db() as session:
        stored = (await session.execute(select(AppSettings))).scalars().all()
        assert any(
            row.key == "komodo_api_key" and row.value.startswith("fernet:")
            for row in stored
        )
    disabled = await async_client.put(
        "/api/v1/settings/komodo", json={"enabled": False}
    )
    assert disabled.status_code == 200 and not disabled.json()["enabled"]
    before = len(calls)
    assert not (await async_client.get("/api/v1/komodo/status")).json()["enabled"]
    assert (
        await async_client.post(
            "/api/v1/komodo/container-action",
            json={"container_name": "owned-synthetic", "action": "stop"},
        )
    ).status_code == 400
    assert len(calls) == before
    assert "synthetic-db-key" not in json.dumps(saved.json())
    assert "synthetic-env-secret" not in json.dumps(status.json())


@pytest.mark.asyncio
async def test_invalid_update_is_atomic_and_client_unchanged(
    async_client, test_db, komodo_service
):
    url, calls, client = komodo_service
    assert (
        await async_client.put("/api/v1/settings/komodo", json={"enabled": True})
    ).status_code == 200
    async with test_db() as session:
        before = {
            r.key: r.value
            for r in (await session.execute(select(AppSettings))).scalars()
        }
    response = await async_client.put(
        "/api/v1/settings/komodo",
        json={
            "enabled": False,
            "api_url": "http://169.254.169.254/metadata",
            "api_key": "synthetic-invalid-key",
        },
    )
    assert response.status_code == 400
    async with test_db() as session:
        assert {
            r.key: r.value
            for r in (await session.execute(select(AppSettings))).scalars()
        } == before
    assert (await async_client.get("/api/v1/komodo/status")).json()["enabled"]
    assert client.enabled and client.api_url == url


@pytest.mark.asyncio
async def test_environment_db_and_mixed_precedence_survive_fresh_client(
    async_client, test_db, komodo_service, monkeypatch
):
    url, calls, _ = komodo_service
    monkeypatch.setattr(settings, "KOMODO_ENABLED", True)
    env_only = (await async_client.get("/api/v1/settings/komodo")).json()
    assert env_only["enabled"] and env_only["api_url"] == url and env_only["connected"]
    monkeypatch.setattr(settings, "KOMODO_ENABLED", False)
    assert (
        await async_client.put(
            "/api/v1/settings/komodo",
            json={
                "enabled": True,
                "api_url": url,
                "api_key": "synthetic-db-key",
                "api_secret": "synthetic-db-secret",
            },
        )
    ).status_code == 200
    # Environment fallback changes after persistence must not override DB fields.
    monkeypatch.setattr(settings, "KOMODO_API_KEY", "")
    monkeypatch.setattr(settings, "KOMODO_API_SECRET", "")
    monkeypatch.setattr(settings, "KOMODO_API_URL", "")
    monkeypatch.setattr(settings, "KOMODO_ALLOWED_HOSTS", "127.0.0.1")
    restarted = KomodoClient()
    try:
        assert await restarted.is_available()
        assert calls[-1][2:] == ("synthetic-db-key", "synthetic-db-secret")
    finally:
        await restarted.close()
    # Clearing a credential is explicit: no fallback to an old environment secret.
    rejected = await async_client.put(
        "/api/v1/settings/komodo", json={"enabled": True, "api_key": ""}
    )
    assert rejected.status_code == 400


@pytest.mark.asyncio
async def test_credential_rotation_reuses_session_with_fresh_request_headers(
    async_client, komodo_service
):
    url, calls, client = komodo_service
    assert (
        await async_client.put("/api/v1/settings/komodo", json={"enabled": True})
    ).status_code == 200
    old_session = client.session
    response = await async_client.put(
        "/api/v1/settings/komodo",
        json={"enabled": True, "api_key": "synthetic-rotated"},
    )
    assert response.status_code == 200
    assert not old_session.closed
    assert await client.request_container_stop("owned-synthetic")
    assert calls[-1][2] == "synthetic-rotated"


@pytest.mark.asyncio
async def test_database_failure_rolls_back_all_settings_and_client(
    async_client, test_db, komodo_service, monkeypatch
):
    _, _, client = komodo_service
    assert (
        await async_client.put("/api/v1/settings/komodo", json={"enabled": True})
    ).status_code == 200
    async with test_db() as session:
        before = {
            r.key: r.value
            for r in (await session.execute(select(AppSettings))).scalars()
        }
        await session.execute(
            text(
                "CREATE TRIGGER owned_reject_key BEFORE INSERT ON app_settings WHEN NEW.key='komodo_api_key' BEGIN SELECT RAISE(ABORT, 'owned atomic failure'); END"
            )
        )
        await session.commit()
    monkeypatch.setattr(async_client._transport, "raise_app_exceptions", False)
    response = await async_client.put(
        "/api/v1/settings/komodo", json={"enabled": False, "api_key": "synthetic-new"}
    )
    assert response.status_code == 500
    async with test_db() as session:
        assert {
            r.key: r.value
            for r in (await session.execute(select(AppSettings))).scalars()
        } == before
    assert client.enabled


@pytest.mark.asyncio
async def test_fresh_process_reads_same_encrypted_database_and_calls_real_endpoint(
    async_client, test_engine, komodo_service
):
    url, calls, _ = komodo_service
    assert (
        await async_client.put(
            "/api/v1/settings/komodo",
            json={
                "enabled": True,
                "api_url": url,
                "api_key": "synthetic-process-key",
                "api_secret": "synthetic-process-secret",
            },
        )
    ).status_code == 200
    source = """import asyncio
from app.komodo import KomodoClient
from app.database import engine
async def main():
    client = KomodoClient()
    try:
        assert await client.is_available()
        assert client.enabled
        assert await client.request_container_stop('owned-process')
        print('FRESH_PROCESS_OK')
    finally:
        await client.close()
        await engine.dispose()
asyncio.run(main())
"""
    environment = dict(
        os.environ,
        DATABASE_URL=str(test_engine.url),
        KOMODO_ENABLED="false",
        KOMODO_API_URL="",
        KOMODO_API_KEY="",
        KOMODO_API_SECRET="",
        KOMODO_ALLOWED_HOSTS="127.0.0.1",
        CREDENTIAL_ENCRYPTION_KEY=settings.CREDENTIAL_ENCRYPTION_KEY,
    )
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        source,
        env=environment,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await asyncio.wait_for(process.communicate(), 30)
    assert process.returncode == 0, stderr.decode()
    assert b"FRESH_PROCESS_OK" in stdout
    assert calls[-1][1:] == (
        "StopContainer",
        "synthetic-process-key",
        "synthetic-process-secret",
    )


@pytest.mark.parametrize(
    "url",
    [
        "http://127.0.0.1:9999",
        "http://10.0.0.1",
        "file:///owned",
        "http://169.254.169.254",
        "http://user:pass@example.com",
    ],
)
def test_unapproved_internal_and_credential_urls_stay_blocked(monkeypatch, url):
    from app.komodo_config import validate_komodo_url

    monkeypatch.setattr(settings, "KOMODO_API_URL", "")
    monkeypatch.setattr(settings, "KOMODO_ALLOWED_HOSTS", "")
    with pytest.raises(ValueError):
        validate_komodo_url(url)
