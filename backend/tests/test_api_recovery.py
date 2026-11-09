"""Real cookie-authenticated export -> NEW SQLite installation -> restore."""

import importlib

import pytest
from httpx import ASGITransport, AsyncClient
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import database
from tests.test_recovery_activation import file_database as file_database
from tests.test_recovery_service import populated as populated


def bind(monkeypatch, sessions):
    for name in [
        "app.database",
        "app.auth",
        "app.main",
        "app.api.auth",
        "app.api.settings",
        "app.api.targets",
        "app.api.schedules",
        "app.api.retention",
        "app.api.backups",
        "app.api.storage",
    ]:
        module = importlib.import_module(name)
        if hasattr(module, "async_session"):
            monkeypatch.setattr(module, "async_session", sessions)


@pytest.mark.asyncio
async def test_authenticated_export_to_new_installation_preview_import_and_session(
    populated, file_database, monkeypatch, tmp_path
):
    from app import main
    from app.recovery_service import export_state, insert_state
    from app.credential_encryption import decrypt_value
    from app.config import settings
    from app import credential_encryption
    from cryptography.fernet import Fernet

    snapshot = await export_state(populated)
    async with file_database() as session:
        await insert_state(session, snapshot)
        await session.commit()
    bind(monkeypatch, file_database)
    async with AsyncClient(
        transport=ASGITransport(app=main.app), base_url="https://test"
    ) as source:
        account = await source.post(
            "/api/v1/auth/setup",
            json={
                "username": "original-admin",
                "password": "synthetic-account-password",
                "confirm_password": "synthetic-account-password",
            },
        )
        assert account.status_code == 200
        exported = await source.post(
            "/api/v1/recovery/export",
            json={
                "password": "synthetic-bundle-password",
                "confirm_password": "synthetic-bundle-password",
            },
        )
        assert exported.status_code == 200
        assert exported.headers["Cache-Control"] == "no-store"
        assert ".dockervault" in exported.headers["Content-Disposition"]
        assert b"synthetic-portable-credential" not in exported.content
        assert b"synthetic-bundle-password" not in exported.content
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'new.db'}")
    async with engine.begin() as connection:
        await connection.run_sync(database.Base.metadata.create_all)
    destination = async_sessionmaker(engine, expire_on_commit=False)
    bind(monkeypatch, destination)
    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    try:
        async with AsyncClient(
            transport=ASGITransport(app=main.app), base_url="https://test"
        ) as client:
            invalid = await client.post(
                "/api/v1/recovery/preview",
                json={"bundle": exported.text, "password": "synthetic-wrong-password"},
            )
            assert invalid.status_code == 400
            async with destination() as session:
                assert await session.scalar(select(func.count(database.User.id))) == 0
                assert await session.scalar(select(func.count(database.Backup.id))) == 0
            preview = await client.post(
                "/api/v1/recovery/preview",
                json={"bundle": exported.text, "password": "synthetic-bundle-password"},
            )
            assert preview.status_code == 200
            assert preview.headers["Cache-Control"] == "no-store"
            assert preview.json()["counts"]["backups"] == 1
            assert "synthetic-portable-credential" not in preview.text
            assert not preview.cookies
            restored = await client.post(
                "/api/v1/recovery/restore",
                json={
                    "bundle": exported.text,
                    "password": "synthetic-bundle-password",
                    "username": "new-admin",
                    "admin_password": "synthetic-new-account",
                    "confirm_admin_password": "synthetic-new-account",
                },
            )
            assert restored.status_code == 200
            assert "session_token" in restored.cookies
            assert "token" not in restored.json()
            assert "synthetic-portable-credential" not in restored.text
            assert "synthetic-bundle-password" not in restored.text
            me = await client.get("/api/v1/auth/me")
            assert me.status_code == 200 and me.json()["username"] == "new-admin"
            assert (await client.get("/api/v1/auth/status")).json()[
                "setup_complete"
            ] is True
            assert (
                await client.get("/api/v1/recovery/status")
            ).json() == restored.json()["result"]
            denied = await client.post(
                "/api/v1/recovery/restore",
                json={
                    "bundle": exported.text,
                    "password": "synthetic-bundle-password",
                    "username": "third-admin",
                    "admin_password": "synthetic-new-account",
                    "confirm_admin_password": "synthetic-new-account",
                },
            )
            assert denied.status_code == 403
        async with destination() as session:
            assert await session.scalar(select(func.count(database.User.id))) == 1
            assert await session.scalar(select(func.count(database.Session.id))) == 1
            assert (await session.get(database.BackupTarget, 9)).schedule_id == 8
            assert (await session.get(database.Backup, 10)).file_size == 321
            assert (
                decrypt_value((await session.get(database.RemoteStorage, 6)).password)
                == "synthetic-portable-credentialftp"
            )
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_export_requires_real_admin_and_password_confirmation(
    file_database, monkeypatch
):
    from app import main
    from app.auth import create_session, hash_password

    bind(monkeypatch, file_database)
    async with file_database() as session:
        session.add(
            database.User(
                id=1,
                username="reader",
                password_hash=hash_password("synthetic-password"),
                is_admin=False,
            )
        )
        session.add(
            database.User(
                id=2,
                username="admin",
                password_hash=hash_password("synthetic-password"),
                is_admin=True,
            )
        )
        await session.commit()
        reader = await create_session(1, session)
        admin = await create_session(2, session)
    async with AsyncClient(
        transport=ASGITransport(app=main.app), base_url="https://test"
    ) as client:
        request = {
            "password": "synthetic-bundle-password",
            "confirm_password": "synthetic-bundle-password",
        }
        assert (
            await client.post("/api/v1/recovery/export", json=request)
        ).status_code == 401
        client.cookies.set("session_token", reader)
        assert (
            await client.post("/api/v1/recovery/export", json=request)
        ).status_code == 403
        client.cookies.set("session_token", admin)
        response = await client.post(
            "/api/v1/recovery/export",
            json={**request, "confirm_password": "synthetic-other-password"},
        )
        assert response.status_code == 400
        assert "synthetic-bundle-password" not in response.text
        assert (
            await client.post(
                "/api/v1/recovery/preview",
                json={"bundle": "{}", "password": "synthetic-password"},
            )
        ).status_code == 403
        assert (await client.get("/api/v1/recovery/nonexistent")).status_code == 404
