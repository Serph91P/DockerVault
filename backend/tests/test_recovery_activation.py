"""Setup activation keeps all writes and the administrator session uncommitted."""

import importlib
import json
from pathlib import Path

import pytest
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import database
from app.auth import create_session
from tests.test_recovery_service import populated as populated


@pytest.mark.asyncio
async def test_failed_activation_does_not_leave_a_new_default_instance_unlock_key(
    file_database, db_session, tmp_path, monkeypatch
):
    from app import credential_encryption
    from app.config import settings
    from app import recovery_service, recovery_activation
    from cryptography.fernet import Fernet

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    db_session.add(
        database.RemoteStorage(
            id=1,
            name="portable provider",
            storage_type="ftp",
            host="synthetic.invalid",
            username="synthetic",
            password=credential_encryption.encrypt_value("synthetic-source-credential"),
            base_path="/",
        )
    )
    await db_session.commit()
    payload = await recovery_service.export_state(db_session)
    key = tmp_path / ".credential_key"
    monkeypatch.setattr(settings, "CREDENTIAL_ENCRYPTION_KEY", "")
    monkeypatch.setattr(credential_encryption, "_KEY_FILE_PATH", key)
    monkeypatch.setattr(credential_encryption, "_fernet", None)

    async def fail(*args):
        assert key.is_file()
        raise RuntimeError("synthetic internal persistence failure")

    monkeypatch.setattr(recovery_service, "inspect_import", fail)
    with pytest.raises(recovery_activation.RecoveryActivationError):
        await recovery_activation.activate_state(
            payload, "replacement", "synthetic-new-password", file_database
        )
    assert not key.exists(), (
        "uncommitted default instance key survived activation rollback"
    )
    assert credential_encryption._fernet is None
    assert not list(tmp_path.glob(".dv-recovery-key-*"))
    async with file_database() as session:
        assert await session.scalar(select(func.count(database.User.id))) == 0
        assert await session.scalar(select(func.count(database.RemoteStorage.id))) == 0


@pytest.mark.asyncio
async def test_session_can_be_staged_without_committing_account(db_session):
    user = database.User(username="synthetic-admin", password_hash="synthetic-hash")
    db_session.add(user)
    await db_session.flush()
    token = await create_session(user.id, db_session, commit=False)
    assert token
    await db_session.rollback()
    assert (await db_session.scalar(select(func.count(database.User.id)))) == 0
    assert (await db_session.scalar(select(func.count(database.Session.id)))) == 0


@pytest.fixture
async def file_database(tmp_path, monkeypatch):
    from app.config import settings

    monkeypatch.setattr(settings, "KOMODO_ENABLED", False)
    monkeypatch.setattr(settings, "KOMODO_API_URL", "")
    monkeypatch.setattr(settings, "KOMODO_API_KEY", "")
    monkeypatch.setattr(settings, "KOMODO_API_SECRET", "")
    monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(tmp_path / "archives"))
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'live.db'}")
    async with engine.begin() as conn:
        await conn.run_sync(database.Base.metadata.create_all)
    sessions = async_sessionmaker(engine, expire_on_commit=False)
    yield sessions
    await engine.dispose()


@pytest.mark.asyncio
async def test_shared_setup_claim_blocks_other_claims_and_recovers_abandoned_stage(
    file_database,
):
    module = importlib.import_module("app.recovery_activation")
    async with module.setup_claim(file_database):
        async with file_database() as session:
            generation = await module.begin_recovery(session)
            await session.commit()
        directory = module.generation_directory(file_database, generation)
        directory.mkdir(parents=True)
        (directory / "rclone.conf").write_bytes(b"encrypted-synthetic-stage")
        with pytest.raises(module.SetupClaimError):
            async with module.setup_claim(file_database):
                pytest.fail("second claimant entered")
        # A different startup cannot clean a living claimant's encrypted material.
        assert await module.recover_incomplete_setup(file_database) is False
        assert directory.is_dir()
    assert await module.recover_incomplete_setup(file_database) is True
    assert not directory.exists()
    async with file_database() as session:
        assert (
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
        ) is None
    async with module.setup_claim(file_database):
        pass


@pytest.mark.asyncio
async def test_create_new_honors_the_same_recovery_claim(file_database, monkeypatch):
    from httpx import ASGITransport, AsyncClient
    from app import main
    from app.api import auth as auth_api

    module = importlib.import_module("app.recovery_activation")
    monkeypatch.setattr(auth_api, "async_session", file_database)
    async with module.setup_claim(file_database):
        async with AsyncClient(
            transport=ASGITransport(app=main.app), base_url="https://test"
        ) as client:
            result = await client.post(
                "/api/v1/auth/setup",
                json={
                    "username": "synthetic-admin",
                    "password": "synthetic-password",
                    "confirm_password": "synthetic-password",
                },
            )
        assert result.status_code == 400
        assert "session_token" not in result.cookies
    async with file_database() as session:
        assert await session.scalar(select(func.count(database.User.id))) == 0


@pytest.mark.asyncio
async def test_activation_commits_catalog_new_admin_and_session_together(
    file_database, db_session, monkeypatch, tmp_path
):
    from app.recovery_service import export_state
    from app.auth import get_session_user

    module = importlib.import_module("app.recovery_activation")
    source = database.RetentionPolicy(id=7, name="imported", keep_last=5)
    db_session.add(source)
    await db_session.commit()
    payload = await export_state(db_session)
    result = await module.activate_state(
        payload, "new-admin", "synthetic-new-password", file_database
    )
    async with file_database() as session:
        user = await get_session_user(result["token"], session)
        assert user.username == "new-admin" and user.is_admin
        assert await session.scalar(select(func.count(database.User.id))) == 1
        assert await session.scalar(select(func.count(database.Session.id))) == 1
        assert (await session.get(database.RetentionPolicy, 7)).keep_last == 5
        marker = json.loads(
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
        )
        assert marker["state"] == "active"
    with pytest.raises(module.SetupClaimError):
        await module.activate_state(
            payload, "another-admin", "synthetic-new-password", file_database
        )


@pytest.mark.asyncio
async def test_internal_activation_failure_rolls_back_original_state(
    file_database, db_session, monkeypatch
):
    from app.recovery_service import export_state

    module = importlib.import_module("app.recovery_activation")
    db_session.add(database.RetentionPolicy(id=7, name="imported", keep_last=5))
    await db_session.commit()
    payload = await export_state(db_session)
    async with file_database() as session:
        session.add(database.RetentionPolicy(id=1, name="original", keep_last=11))
        await session.commit()
    # A real SQLite INSERT failure after deleting/replacing live logical rows.
    async with file_database.kw["bind"].begin() as connection:
        await connection.execute(
            database.text(
                "CREATE TRIGGER reject_new_admin BEFORE INSERT ON users BEGIN SELECT RAISE(ABORT, 'synthetic persistence failure'); END"
            )
        )
    with pytest.raises(module.RecoveryActivationError):
        await module.activate_state(
            payload, "new-admin", "synthetic-new-password", file_database
        )
    async with file_database() as session:
        assert await session.scalar(select(func.count(database.User.id))) == 0
        assert await session.scalar(select(func.count(database.Session.id))) == 0
        assert (await session.get(database.RetentionPolicy, 1)).keep_last == 11
        assert await session.get(database.RetentionPolicy, 7) is None
        assert (
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
            is None
        )


@pytest.mark.asyncio
async def test_populated_activation_is_read_only_for_existing_archives(
    populated, file_database, tmp_path
):
    from app.recovery_service import export_state
    from app.credential_encryption import decrypt_value

    module = importlib.import_module("app.recovery_activation")
    remote = tmp_path / "remote"
    remote.mkdir()
    archive = tmp_path / "archives" / "plan" / "backup.tar.gz.age"
    archive.parent.mkdir(parents=True)
    archive.write_bytes(b"synthetic-existing-archive-byte-sentinel")
    key = Path(str(archive) + ".key")
    key.write_bytes(b"synthetic-existing-encrypted-sidecar-byte-sentinel")
    before = {
        str(p): p.read_bytes()
        for root in [remote, archive.parent]
        for p in root.rglob("*")
        if p.is_file()
    }
    payload = await export_state(populated)
    result = await module.activate_state(
        payload, "new-admin", "synthetic-password", file_database
    )
    states = {row["id"]: row["state"] for row in result["result"]["storages"]}
    assert states[1] == "connected" and states[2] == states[3] == "credentials_required"
    assert result["result"]["catalog"][0]["local"] == "available"
    assert result["result"]["catalog"][0]["copies"][0]["state"] == "unavailable"
    assert result["result"]["archives_modified"] is False
    assert {
        str(p): p.read_bytes()
        for root in [remote, archive.parent]
        for p in root.rglob("*")
        if p.is_file()
    } == before
    async with file_database() as session:
        assert (await session.get(database.Schedule, 8)).cron_expression == "0 2 * * *"
        assert (await session.get(database.BackupTarget, 9)).remote_storage_ids == [
            1,
            2,
            3,
            4,
            5,
            6,
        ]
        assert (
            decrypt_value((await session.get(database.RemoteStorage, 6)).password)
            == "synthetic-portable-credentialftp"
        )
        assert await session.scalar(select(func.count(database.Backup.id))) == 1
        assert (
            await session.scalar(select(func.count(database.BackupStorageSync.id))) == 1
        )
        assert await session.scalar(select(func.count(database.BackupLog.id))) == 0
        assert (
            await session.scalar(select(func.count(database.WorkloadOperation.id))) == 0
        )


@pytest.mark.asyncio
async def test_missing_rclone_dependency_is_credentials_required_not_offline(
    file_database, db_session, tmp_path, monkeypatch
):
    from app.config import settings
    from app.recovery_service import export_state, validate_state

    module = importlib.import_module("app.recovery_activation")
    monkeypatch.setattr(settings, "RCLONE_CONFIG", str(tmp_path / "absent.conf"))
    db_session.add(
        database.RemoteStorage(
            id=1,
            name="fixture-rclone",
            storage_type="rclone",
            rclone_remote="missing",
            base_path="/backups",
        )
    )
    await db_session.commit()
    payload = await export_state(db_session)
    assert payload["rclone"]["roots"][0]["state"] == "credentials_required"
    preview = await validate_state(payload)
    assert preview["storages"][0]["state"] == "credentials_required"
    result = await module.activate_state(
        payload, "new-admin", "synthetic-password", file_database
    )
    assert result["result"]["storages"][0]["state"] == "credentials_required"


@pytest.mark.asyncio
async def test_reconciliation_uses_staged_rclone_not_old_default(
    file_database, db_session, tmp_path, monkeypatch
):
    from app.recovery_service import export_state
    from app.rclone_recovery import (
        create_managed_config,
        package_rclone,
        provider_options,
    )

    module = importlib.import_module("app.recovery_activation")
    storages = [{"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}]
    portable = package_rclone(
        {"vault": {"type": "memory"}}, storages, provider_options()
    )
    source = await create_managed_config(portable, tmp_path / "source-generation")
    db_session.add(
        database.AppSettings(key="recovery_rclone", value=json.dumps(source))
    )
    db_session.add(
        database.RemoteStorage(
            id=1,
            name="fixture-rclone",
            storage_type="rclone",
            rclone_remote="vault",
            base_path="/",
        )
    )
    db_session.add(
        database.BackupTarget(
            id=1,
            name="fixture-plan",
            target_type="path",
            host_path="/synthetic/source",
            remote_storage_ids=[1],
        )
    )
    await db_session.flush()
    db_session.add(
        database.Backup(
            id=1, target_id=1, status=database.BackupStatus.COMPLETED, file_size=5
        )
    )
    await db_session.flush()
    db_session.add(
        database.BackupStorageSync(
            backup_id=1,
            storage_id=1,
            remote_path="existing.tar",
            sync_status="completed",
        )
    )
    await db_session.commit()
    payload = await export_state(db_session)
    foreign = tmp_path / "foreign-data"
    foreign.mkdir()
    (foreign / "existing.tar").write_bytes(b"foreign sentinel")
    config = tmp_path / "foreign.conf"
    config.write_text("[vault]\ntype=alias\nremote=" + str(foreign) + "\n")
    monkeypatch.setenv("RCLONE_CONFIG", str(config))
    monkeypatch.setattr(database, "async_session", file_database)
    result = await module.activate_state(
        payload, "new-admin", "synthetic-password", file_database
    )
    assert result["result"]["storages"][0]["state"] == "connected"
    assert result["result"]["catalog"][0]["copies"][0]["state"] == "unavailable"


@pytest.mark.asyncio
@pytest.mark.parametrize("consumer", ["schedule", "backup", "retention", "orphans"])
async def test_preparing_generation_fences_scheduling_admission_and_deletion(
    file_database, tmp_path, monkeypatch, consumer
):
    from datetime import datetime
    from app import backup_engine as engine_module
    from app import retention, scheduler
    from app.config import settings

    module = importlib.import_module("app.recovery_activation")
    root = Path(settings.BACKUP_BASE_PATH)
    root.mkdir()
    paths = [root / "old.tar", root / "new.tar", root / "orphan.tar"]
    for path in paths:
        path.write_bytes(b"synthetic-protected-by-pending-recovery")
    async with file_database() as session:
        session.add(
            database.RetentionPolicy(
                id=1,
                name="default",
                keep_last=1,
                keep_daily=0,
                keep_weekly=0,
                keep_monthly=0,
                keep_yearly=0,
                max_age_days=1,
            )
        )
        target = database.BackupTarget(
            id=1,
            name="fixture-plan",
            target_type="path",
            host_path="/synthetic/source",
            schedule_cron="* * * * *",
            retention_policy_id=1,
        )
        session.add(target)
        await session.flush()
        for i, path in enumerate(paths[:2], start=1):
            session.add(
                database.Backup(
                    id=i,
                    target_id=1,
                    status=database.BackupStatus.COMPLETED,
                    file_path=str(path),
                    created_at=datetime(2020, 1, i),
                )
            )
        await session.commit()
    async with file_database() as session:
        await module.begin_recovery(session)
        await session.commit()
    monkeypatch.setattr(engine_module, "async_session", file_database)
    monkeypatch.setattr(scheduler, "async_session", file_database)
    monkeypatch.setattr(retention, "async_session", file_database)
    if consumer == "schedule":
        owned = scheduler.BackupScheduler()
        owned.scheduler.start(paused=True)
        try:
            await owned.reconcile()
            assert not [
                job
                for job in owned.scheduler.get_jobs()
                if job.id.startswith("backup_target_")
            ]
        finally:
            owned.scheduler.shutdown(wait=False)
    elif consumer == "backup":
        with pytest.raises(ValueError):
            await engine_module.backup_engine.create_backup(target)
    elif consumer == "retention":
        with pytest.raises(ValueError):
            await retention.retention_manager.apply_retention(1)
    else:
        with pytest.raises(ValueError):
            await retention.retention_manager.cleanup_orphaned_files()
    assert all(
        path.read_bytes() == b"synthetic-protected-by-pending-recovery"
        for path in paths
    )
    async with file_database() as session:
        assert await session.scalar(select(func.count(database.Backup.id))) == 2


@pytest.mark.asyncio
async def test_startup_does_not_recover_jobs_inside_a_living_restore_claim(
    file_database, monkeypatch
):
    module = importlib.import_module("app.recovery_activation")
    monkeypatch.setattr(database, "engine", file_database.kw["bind"])
    monkeypatch.setattr(database, "async_session", file_database)
    async with file_database() as session:
        session.add(
            database.BackupTarget(
                id=1, name="fixture", target_type="path", host_path="/synthetic"
            )
        )
        await session.flush()
        session.add(
            database.Backup(id=1, target_id=1, status=database.BackupStatus.RUNNING)
        )
        await session.commit()
    async with module.setup_claim(file_database):
        async with file_database() as session:
            await module.begin_recovery(session)
            await session.commit()
        await database.init_db()
        async with file_database() as session:
            assert (
                await session.get(database.Backup, 1)
            ).status == database.BackupStatus.RUNNING
            assert (
                await session.scalar(select(func.count(database.RetentionPolicy.id)))
                == 0
            )
    await database.init_db()
    async with file_database() as session:
        assert (
            await session.get(database.Backup, 1)
        ).status == database.BackupStatus.FAILED
        assert (
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_activation"
                )
            )
            is None
        )


@pytest.mark.asyncio
async def test_pending_restore_blocks_storage_admission_without_touching_files(
    file_database, monkeypatch, tmp_path
):
    import uuid
    from fastapi import HTTPException
    from app.storage_jobs import StorageJobs
    from app.api.operations import CreateOperation
    from app.config import settings

    root = Path(settings.BACKUP_BASE_PATH)
    root.mkdir()
    file = root / "must-stay.txt"
    file.write_bytes(b"synthetic-storage-fence")
    async with file_database() as session:
        session.add(database.User(id=1, username="fixture", password_hash="unused"))
        session.add(
            database.AppSettings(
                key="recovery_activation",
                value=json.dumps({"state": "preparing", "generation": "a" * 32}),
            )
        )
        await session.commit()
    jobs = StorageJobs(file_database)
    with pytest.raises(HTTPException) as error:
        await jobs.submit(
            1,
            CreateOperation(
                action="storage_delete",
                storage_id="local",
                path="",
                paths=["must-stay.txt"],
                request_id=str(uuid.uuid4()),
            ),
        )
    assert error.value.status_code == 409
    assert file.read_bytes() == b"synthetic-storage-fence"
    async with file_database() as session:
        assert (
            await session.scalar(select(func.count(database.StorageOperation.id))) == 0
        )


@pytest.mark.asyncio
async def test_preparing_restore_prevents_workload_recovery_and_late_backup_execution(
    file_database, monkeypatch
):
    from unittest.mock import AsyncMock
    from app import backup_engine as engine_module

    module = importlib.import_module("app.recovery_activation")
    async with file_database() as session:
        session.add(
            database.BackupTarget(
                id=1, name="fixture", target_type="path", host_path="/synthetic"
            )
        )
        await session.flush()
        session.add(
            database.Backup(id=1, target_id=1, status=database.BackupStatus.RUNNING)
        )
        await session.flush()
        session.add(
            database.WorkloadOperation(
                id=1,
                backup_id=1,
                kind="backup",
                state="active",
                resources={
                    "synthetic-container": {
                        "original": "running",
                        "requested": True,
                        "stopped": True,
                        "restored": False,
                    }
                },
            )
        )
        await session.commit()
    async with file_database() as session:
        await module.begin_recovery(session)
        await session.commit()
    monkeypatch.setattr(engine_module, "async_session", file_database)
    docker = AsyncMock()
    docker.get_container_state.return_value = "running"
    monkeypatch.setattr(engine_module, "docker_client", docker)
    engine = engine_module.BackupEngine()
    monkeypatch.setattr(
        engine, "_restart_containers_cancellation_safe", AsyncMock(return_value=[])
    )
    await engine.recover_operations()
    async with file_database() as session:
        operation = await session.get(database.WorkloadOperation, 1)
        assert operation.state == "active"
        assert operation.resources["synthetic-container"]["restored"] is False
    assert await engine.run_backup(1, skip_validation=True) is False
    async with file_database() as session:
        assert (
            await session.get(database.Backup, 1)
        ).status == database.BackupStatus.RUNNING


@pytest.mark.asyncio
async def test_completed_installation_is_never_claimed_or_cleaned(file_database):
    module = importlib.import_module("app.recovery_activation")
    async with file_database() as session:
        session.add(database.User(username="existing-admin", password_hash="synthetic"))
        session.add(
            database.AppSettings(
                key="recovery_activation",
                value=json.dumps({"state": "active", "generation": "a" * 32}),
            )
        )
        await session.commit()
    with pytest.raises(module.SetupClaimError):
        async with module.setup_claim(file_database):
            pytest.fail("existing installation claimed")
    assert await module.recover_incomplete_setup(file_database) is False
