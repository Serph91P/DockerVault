"""Real OS-process death and first-run concurrency; never production data."""

import asyncio
import multiprocessing
import os
import shutil
import signal
import time
from pathlib import Path

from cryptography.fernet import Fernet
import pytest
from tests.test_recovery_activation import file_database as file_database


def _bootstrap_worker(pipe, directory, key):
    from app.config import settings
    from app import credential_encryption
    from app import rclone_recovery as m

    settings.CREDENTIAL_ENCRYPTION_KEY = key
    credential_encryption._fernet = None
    original = m._rpc

    def observe(address, endpoint, data):
        result = original(address, endpoint, data)
        if endpoint == "rc/noop":
            native = original(address, "core/pid", {})
            pipe.send(
                {
                    "pid": native["pid"],
                    "socket_directory": str(Path(address).parent.resolve()),
                }
            )
            pipe.recv()  # Parent kills this process while real native bootstrap is alive.
        return result

    m._rpc = observe
    payload = {"schema_version": 1, "roots": [], "remotes": {}, "files": {}}

    async def prepare():
        from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
        from app import database
        from app import recovery_activation as activation

        engine = create_async_engine(f"sqlite+aiosqlite:///{directory}")
        async with engine.begin() as connection:
            await connection.run_sync(database.Base.metadata.create_all)
        sessions = async_sessionmaker(engine, expire_on_commit=False)
        async with sessions() as session:
            generation = await activation.begin_recovery(session)
            await session.commit()
        path = activation.generation_directory(sessions, generation)
        path.parent.mkdir(mode=0o700)
        await m.create_managed_config(payload, path)

    asyncio.run(prepare())


def _running(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().split(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def test_native_bootstrap_cannot_outlive_killed_recovery_owner(tmp_path):
    context = multiprocessing.get_context("spawn")
    parent, child = context.Pipe()
    database_path = tmp_path / "live.sqlite"
    worker = context.Process(
        target=_bootstrap_worker,
        args=(child, str(database_path), Fernet.generate_key().decode()),
    )
    native = None
    worker.start()
    child.close()
    try:
        assert parent.poll(20), "real native bootstrap did not become ready"
        native = parent.recv()
        assert _running(native["pid"])
        worker.kill()
        worker.join(timeout=10)
        assert worker.exitcode == -signal.SIGKILL
        deadline = time.monotonic() + 5
        while _running(native["pid"]) and time.monotonic() < deadline:
            time.sleep(0.02)
        assert not _running(native["pid"]), (
            "orphan native daemon retained config unlock after owner death"
        )

        async def restart():
            from sqlalchemy import select
            from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
            from app.database import AppSettings
            from app.recovery_activation import recover_incomplete_setup

            engine = create_async_engine(f"sqlite+aiosqlite:///{database_path}")
            sessions = async_sessionmaker(engine, expire_on_commit=False)
            assert await recover_incomplete_setup(sessions) is True
            async with sessions() as session:
                assert (
                    await session.scalar(
                        select(AppSettings.value).where(
                            AppSettings.key == "recovery_activation"
                        )
                    )
                    is None
                )
            await engine.dispose()

        asyncio.run(restart())
        assert not Path(native["socket_directory"]).exists(), (
            "orphan confidential RC socket directory survived owner death"
        )
    finally:
        if worker.is_alive():
            worker.kill()
            worker.join(timeout=10)
        if native:
            if _running(native["pid"]):
                os.kill(native["pid"], signal.SIGKILL)
            shutil.rmtree(native["socket_directory"], ignore_errors=True)
        parent.close()


def _request_worker(pipe, database_path, backup_path, key, request, stage):
    async def execute():
        import json
        from httpx import ASGITransport, AsyncClient
        from sqlalchemy import event, select
        from sqlalchemy.ext.asyncio import (
            async_sessionmaker,
            create_async_engine,
            AsyncSession,
        )
        from app.config import settings
        from app import credential_encryption, database, rclone_recovery
        from app import main
        from app.api import auth as auth_api

        settings.CREDENTIAL_ENCRYPTION_KEY = key
        settings.BACKUP_BASE_PATH = backup_path
        settings.KOMODO_ENABLED = False
        credential_encryption._KEY_FILE_PATH = (
            Path(database_path).parent / ".credential_key"
        )
        credential_encryption._fernet = None
        engine = create_async_engine(f"sqlite+aiosqlite:///{database_path}")
        sessions = async_sessionmaker(engine, expire_on_commit=False)
        database.async_session = sessions
        auth_api.async_session = sessions

        def park(name):
            pipe.send({"stage": name})
            pipe.recv()

        original = rclone_recovery.create_managed_config

        async def prepare(payload, directory):
            result = await original(payload, directory)
            if stage == "prepared":
                park("prepared")
            return result

        rclone_recovery.create_managed_config = prepare
        native_run = rclone_recovery.subprocess.run

        def probe(command, *args, **kwargs):
            if stage == "probe_running" and "lsd" in command:
                pipe.send({"stage": stage, "cache_path": kwargs["env"]["TMPDIR"]})
                pipe.recv()
            return native_run(command, *args, **kwargs)

        rclone_recovery.subprocess.run = probe

        @event.listens_for(AsyncSession.sync_session_class, "before_commit")
        def before_commit(session):
            value = session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
            active = value and json.loads(value)["state"] == "active"
            session.info["recovery_active"] = bool(active)
            if active and stage == "before_commit":
                park("before_commit")

        @event.listens_for(AsyncSession.sync_session_class, "after_commit")
        def after_commit(session):
            if session.info.get("recovery_active") and stage == "after_commit":
                park("after_commit")

        async with AsyncClient(
            transport=ASGITransport(app=main.app), base_url="https://test"
        ) as client:
            response = await client.post(request["url"], json=request["body"])
            pipe.send(
                {
                    "code": response.status_code,
                    "cookie": bool(response.cookies.get("session_token")),
                    "body": response.json(),
                }
            )
        await engine.dispose()

    asyncio.run(execute())


@pytest.mark.asyncio
@pytest.mark.parametrize("credential_mode", ["environment", "default_file"])
@pytest.mark.parametrize(
    "stage", ["prepared", "probe_running", "before_commit", "after_commit"]
)
async def test_real_restore_crash_and_competing_setup_have_one_durable_activation(
    stage, credential_mode, file_database, db_session, tmp_path, monkeypatch
):
    import json
    from sqlalchemy import select, func
    from app import database, credential_encryption
    from app.config import settings
    from app.recovery_service import export_state, TABLES
    from app.recovery_bundle import encode_bundle
    from app.recovery_activation import recover_incomplete_setup, recovery_pending
    from app import rclone_recovery as rclone

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    monkeypatch.setattr(
        settings, "RCLONE_CONFIG", "/nonexistent/synthetic-source-config"
    )
    db_session.add(database.RetentionPolicy(id=7, name="imported", keep_last=5))
    db_session.add(
        database.RemoteStorage(
            id=2,
            name="synthetic memory",
            storage_type="rclone",
            rclone_remote="vault",
            password=credential_encryption.encrypt_value(
                "synthetic-restored-provider-secret"
            ),
        )
    )
    await db_session.commit()
    source_storage = {"id": 2, "storage_type": "rclone", "rclone_remote": "vault"}
    source_payload = rclone.package_rclone(
        {"vault": {"type": "memory"}}, [source_storage], rclone.provider_options()
    )
    source_state = await rclone.create_managed_config(
        source_payload, tmp_path / "source-generation"
    )
    db_session.add(
        database.AppSettings(key="recovery_rclone", value=json.dumps(source_state))
    )
    await db_session.commit()
    payload = await export_state(db_session)
    storages = payload["tables"]["remote_storages"]
    bundle = encode_bundle(payload, "synthetic-bundle-password").decode()
    key = Fernet.generate_key().decode() if credential_mode == "environment" else ""
    key_path = tmp_path / ".credential_key"
    monkeypatch.setattr(credential_encryption, "_KEY_FILE_PATH", key_path)
    monkeypatch.setattr(settings, "CREDENTIAL_ENCRYPTION_KEY", key)
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    async with file_database() as session:
        session.add(database.RetentionPolicy(id=1, name="original", keep_last=11))
        await session.commit()
        original = {
            name: [
                dict(row) for row in (await session.execute(select(table))).mappings()
            ]
            for name, table in TABLES.items()
        }
    database_path = str(file_database.kw["bind"].url.database)
    request = {
        "url": "/api/v1/recovery/restore",
        "body": {
            "bundle": bundle,
            "password": "synthetic-bundle-password",
            "username": "restored-admin",
            "admin_password": "synthetic-admin-password",
            "confirm_admin_password": "synthetic-admin-password",
        },
    }
    context = multiprocessing.get_context("spawn")
    parent, child = context.Pipe()
    worker = context.Process(
        target=_request_worker,
        args=(child, database_path, settings.BACKUP_BASE_PATH, key, request, stage),
    )
    worker.start()
    child.close()
    try:
        assert await asyncio.to_thread(parent.poll, 30), (
            "restore never reached requested real persistence boundary"
        )
        boundary = parent.recv()
        assert boundary["stage"] == stage
        if credential_mode == "default_file":
            assert key_path.is_file()
            assert key_path.stat().st_mode & 0o777 == 0o600
            assert not list(tmp_path.glob(".dv-recovery-key-*"))
        async with file_database() as session:
            assert await recovery_pending(session) is (stage != "after_commit")
            assert await session.scalar(select(func.count(database.User.id))) == (
                1 if stage == "after_commit" else 0
            )
        assert (
            await recover_incomplete_setup(file_database) is False
        )  # Living claim is never stolen.
        marker = None
        async with file_database() as session:
            marker = json.loads(
                await session.scalar(
                    select(database.AppSettings.value).where(
                        database.AppSettings.key == "recovery_activation"
                    )
                )
            )
        directory = Path(database_path).parent / ".recovery" / marker["generation"]
        assert (
            (directory / "rclone.conf")
            .read_bytes()
            .startswith(b"# Encrypted rclone configuration File")
        )
        create = {
            "url": "/api/v1/auth/setup",
            "body": {
                "username": "competing-admin",
                "password": "synthetic-new-password",
                "confirm_password": "synthetic-new-password",
            },
        }
        for contender in (request, create):
            receive, send = context.Pipe()
            competitor = context.Process(
                target=_request_worker,
                args=(
                    send,
                    database_path,
                    settings.BACKUP_BASE_PATH,
                    key,
                    contender,
                    None,
                ),
            )
            competitor.start()
            send.close()
            try:
                assert await asyncio.to_thread(receive.poll, 30)
                rejected = receive.recv()
                assert rejected["code"] in {400, 403, 409} and not rejected["cookie"]
                competitor.join(timeout=10)
                assert competitor.exitcode == 0
            finally:
                if competitor.is_alive():
                    competitor.kill()
                    competitor.join(timeout=10)
                receive.close()
        worker.kill()
        worker.join(timeout=10)
        assert worker.exitcode == -signal.SIGKILL
        assert await recover_incomplete_setup(file_database) is (
            stage != "after_commit"
        )
        if credential_mode == "default_file":
            assert key_path.exists() is (stage == "after_commit")
            assert not list(tmp_path.glob(".dv-recovery-key-*"))
        async with file_database() as session:
            assert await recovery_pending(session) is False
            if stage == "after_commit":
                assert (await session.get(database.RetentionPolicy, 7)).keep_last == 5
                assert await session.get(database.RetentionPolicy, 1) is None
                assert await session.scalar(select(func.count(database.User.id))) == 1
                assert (
                    await session.scalar(select(func.count(database.Session.id))) == 1
                )
                storage = await session.get(database.RemoteStorage, 2)
                assert (
                    credential_encryption.decrypt_value(storage.password)
                    == "synthetic-restored-provider-secret"
                )
                assert (await rclone.export_rclone(session, storages)) == payload[
                    "rclone"
                ]
            else:
                assert await session.scalar(select(func.count(database.User.id))) == 0
                assert (
                    await session.scalar(select(func.count(database.Session.id))) == 0
                )
                current = {
                    name: [
                        dict(row)
                        for row in (await session.execute(select(table))).mappings()
                    ]
                    for name, table in TABLES.items()
                }
                assert current == original
                assert not directory.exists()
                if stage == "probe_running":
                    assert not Path(boundary["cache_path"]).exists(), (
                        "uncommitted probe cache survived recovery restart"
                    )
    finally:
        if worker.is_alive():
            worker.kill()
            worker.join(timeout=10)
        parent.close()
