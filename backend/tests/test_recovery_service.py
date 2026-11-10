"""Portable state uses real SQLite and a different destination credential key."""

import copy
import importlib
import json
from datetime import datetime

import pytest
from cryptography.fernet import Fernet
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import credential_encryption as credentials
from app import database as db
from app.config import settings

SECRET = "synthetic-portable-credential"
NOW = datetime(2026, 7, 11, 12, 30, 45)


@pytest.mark.asyncio
async def test_import_rejects_removed_rclone_configuration_even_with_consistent_manifest(
    db_session, monkeypatch
):
    monkeypatch.setattr(
        settings, "RCLONE_CONFIG", "/nonexistent/synthetic-source-config"
    )
    db_session.add(
        db.RemoteStorage(
            id=1, name="synthetic rclone", storage_type="rclone", rclone_remote="vault"
        )
    )
    await db_session.commit()
    payload = await service().export_state(db_session)
    payload["rclone"] = {"schema_version": 1, "roots": [], "remotes": {}, "files": {}}
    payload["manifest"] = service().state_manifest(payload)
    with pytest.raises(service().RecoveryStateError):
        await service().validate_state(payload)


@pytest.mark.asyncio
@pytest.mark.parametrize("field", ["password", "s3_access_key", "s3_secret_key"])
async def test_accepted_long_provider_credential_roundtrips_under_new_key(
    db_session, monkeypatch, field
):
    secret = "synthetic-long-provider-credential-" + "x" * 1024
    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credentials, "_fernet", None)
    db_session.add(
        db.RemoteStorage(
            id=1,
            name="synthetic credential",
            storage_type="s3",
            **{field: credentials.encrypt_value(secret)},
        )
    )
    await db_session.commit()
    payload = await service().export_state(db_session)
    assert payload["tables"]["remote_storages"][0][field] == secret
    await service().validate_state(payload)
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    try:
        async with engine.begin() as connection:
            await connection.run_sync(db.Base.metadata.create_all)
        monkeypatch.setattr(
            settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
        )
        monkeypatch.setattr(credentials, "_fernet", None)
        async with async_sessionmaker(engine, expire_on_commit=False)() as destination:
            await service().insert_state(destination, payload)
            await destination.commit()
            restored = await destination.get(db.RemoteStorage, 1)
            assert credentials.decrypt_value(getattr(restored, field)) == secret
    finally:
        await engine.dispose()


def service():
    return importlib.import_module("app.recovery_service")


@pytest.fixture
async def populated(db_session, monkeypatch, tmp_path):
    monkeypatch.setattr(settings, "BACKUP_BASE_PATH", str(tmp_path / "archives"))
    monkeypatch.setattr(settings, "KOMODO_ENABLED", True)
    monkeypatch.setattr(settings, "KOMODO_API_URL", "https://komodo.example.invalid")
    monkeypatch.setattr(settings, "KOMODO_API_KEY", "synthetic-env-key")
    monkeypatch.setattr(settings, "KOMODO_API_SECRET", "synthetic-env-secret")
    db_session.add(
        db.RetentionPolicy(
            id=7,
            name="default",
            description="GFS",
            keep_last=5,
            keep_daily=6,
            keep_weekly=7,
            keep_monthly=8,
            keep_yearly=9,
            max_age_days=400,
            created_at=NOW,
            updated_at=NOW,
        )
    )
    db_session.add(
        db.Schedule(
            id=8,
            name="nightly",
            cron_expression="0 2 * * *",
            description="schedule",
            enabled=True,
            created_at=NOW,
            updated_at=NOW,
        )
    )
    db_session.add(
        db.EncryptionConfig(
            id=1,
            public_key="age1synthetic-public",
            key_created_at=NOW,
            encryption_enabled=True,
            setup_completed=True,
            created_at=NOW,
            updated_at=NOW,
        )
    )
    for i, kind in enumerate(["local", "ssh", "sftp", "webdav", "s3", "ftp"], start=1):
        db_session.add(
            db.RemoteStorage(
                id=i,
                name=f"fixture-{kind}",
                storage_type=kind,
                enabled=True,
                host="storage.example.invalid",
                port=2222,
                username="fixture-user",
                password=credentials.encrypt_value(SECRET + kind),
                base_path=str(tmp_path / "remote") if kind == "local" else "/archives",
                ssh_key_path="/app/data/ssh_keys/external"
                if kind in ["ssh", "sftp"]
                else None,
                s3_bucket="fixture-bucket",
                s3_region="test-region",
                s3_access_key=credentials.encrypt_value("synthetic-access"),
                s3_secret_key=credentials.encrypt_value("synthetic-secret"),
                s3_endpoint_url="https://s3.example.invalid",
                webdav_url="https://webdav.example.invalid",
                created_at=NOW,
                updated_at=NOW,
            )
        )
    await db_session.flush()
    db_session.add(
        db.BackupTarget(
            id=9,
            name="portable plan",
            target_type="path",
            container_id="container-id",
            container_name="container-name",
            volume_name="volume",
            host_path="/data/fixture",
            stack_name="stack",
            schedule_id=8,
            schedule_cron="0 3 * * *",
            enabled=True,
            removed_at=None,
            retention_policy_id=7,
            dependencies=["other"],
            selected_volumes=["volume"],
            include_paths=["include"],
            exclude_paths=["exclude"],
            per_volume_rules={
                "volume": {"include_paths": [], "exclude_paths": ["cache"]}
            },
            pre_backup_command="pg_dump synthetic",
            post_backup_command="pg_dump synthetic-post",
            stop_container=False,
            compression_enabled=True,
            sync_to_remote=True,
            remote_storage_ids=[1, 2, 3, 4, 5, 6],
            delete_local_after_sync=True,
            created_at=NOW,
            updated_at=NOW,
        )
    )
    await db_session.flush()
    archive = str(tmp_path / "archives" / "plan" / "backup.tar.gz.age")
    db_session.add(
        db.Backup(
            id=10,
            target_id=9,
            backup_type=db.BackupType.FULL,
            status=db.BackupStatus.COMPLETED,
            file_path=archive,
            file_size=321,
            checksum="a" * 64,
            encrypted=True,
            encryption_key_path=archive + ".key",
            started_at=NOW,
            completed_at=NOW,
            duration_seconds=19,
            error_message=None,
            retry_count=2,
            backup_metadata={"target_name": "portable plan", "compression": "gzip"},
            created_at=NOW,
        )
    )
    db_session.add(
        db.BackupSchedule(
            id=11,
            target_id=9,
            cron_expression="0 4 * * *",
            next_run=NOW,
            last_run=NOW,
            enabled=False,
            created_at=NOW,
            updated_at=NOW,
        )
    )
    db_session.add(
        db.AppSettings(
            id=12,
            key="komodo_api_key",
            value=credentials.encrypt_value(SECRET),
            updated_at=NOW,
        )
    )
    # DB presence, including explicit empty values, wins over environment.
    db_session.add(
        db.AppSettings(id=13, key="komodo_api_secret", value="", updated_at=NOW)
    )
    db_session.add(
        db.AppSettings(
            id=14, key="storage_check_quota_before_upload", value="true", updated_at=NOW
        )
    )
    db_session.add(
        db.AppSettings(
            id=15, key="encryption_onboarding_deferred", value="false", updated_at=NOW
        )
    )
    db_session.add(
        db.AppSettings(
            id=16, key="unknown_runtime_secret", value="never-portable", updated_at=NOW
        )
    )
    db_session.add(
        db.User(id=1, username="old-user", password_hash="not-exported", is_admin=True)
    )
    await db_session.flush()
    db_session.add(
        db.BackupStorageSync(
            id=17,
            backup_id=10,
            storage_id=1,
            remote_path="plan/backup.tar.gz.age",
            synced_at=NOW,
            sync_status="completed",
            error_message=None,
        )
    )
    db_session.add(
        db.BackupLog(backup_id=10, step="complete", message="never-export-log")
    )
    db_session.add(db.Session(user_id=1, token="never-export-session", expires_at=NOW))
    db_session.add(
        db.WorkloadOperation(
            backup_id=10, kind="backup", resources={"secret": "never-export-operation"}
        )
    )
    await db_session.commit()
    return db_session


@pytest.mark.asyncio
async def test_every_portable_table_roundtrips_under_new_local_key(
    populated, monkeypatch
):
    module = service()
    payload = await module.export_state(populated)
    assert set(payload["tables"]) == {
        "retention_policies",
        "schedules",
        "backup_targets",
        "backups",
        "backup_schedules",
        "remote_storages",
        "backup_storage_syncs",
        "encryption_config",
        "app_settings",
    }
    rendered = json.dumps(payload)
    for excluded in [
        "never-portable",
        "never-export-log",
        "never-export-session",
        "never-export-operation",
        "not-exported",
    ]:
        assert excluded not in rendered
    assert (
        SECRET in rendered
    )  # Only inside the envelope plaintext, never an HTTP response.
    assert "fernet:" not in rendered
    preview = await module.validate_state(payload)
    assert preview["counts"]["backups"] == 1
    assert SECRET not in json.dumps(preview)
    assert "pg_dump synthetic" not in json.dumps(preview)
    assert {
        x["id"] for x in preview["storages"] if x["state"] == "credentials_required"
    } == {2, 3}
    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credentials, "_fernet", None)
    destination = create_async_engine("sqlite+aiosqlite:///:memory:")
    try:
        async with destination.begin() as conn:
            await conn.run_sync(db.Base.metadata.create_all)
        sessions = async_sessionmaker(destination, expire_on_commit=False)
        async with sessions() as session:
            await module.insert_state(session, payload)
            await session.commit()
            restored = await module.export_state(session)
            assert restored["tables"] == payload["tables"]
            storages = (await session.execute(select(db.RemoteStorage))).scalars().all()
            for row in storages:
                assert row.password.startswith("fernet:")
                assert (
                    credentials.decrypt_value(row.password) == SECRET + row.storage_type
                )
                assert (
                    credentials.decrypt_value(row.s3_access_key) == "synthetic-access"
                )
                assert (
                    credentials.decrypt_value(row.s3_secret_key) == "synthetic-secret"
                )
            values = {
                row.key: row.value
                for row in (await session.execute(select(db.AppSettings))).scalars()
            }
            assert credentials.decrypt_value(values["komodo_api_key"]) == SECRET
            assert values["komodo_api_secret"] == ""
            assert values["komodo_api_url"] == "https://komodo.example.invalid"
            assert values["komodo_enabled"] == "true"
    finally:
        await destination.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "mutation",
    [
        "manifest",
        "missing-table",
        "unknown-table",
        "unknown-column",
        "foreign-key",
        "remote-id",
        "duplicate-id",
        "version",
        "application",
        "traversal",
        "outside-root",
        "sidecar",
        "absolute-remote",
        "bad-cron",
        "bad-type",
        "unknown-setting",
    ],
)
async def test_invalid_state_rejected_before_live_effect(populated, mutation):
    module = service()
    payload = await module.export_state(populated)
    before = await module.export_state(populated)
    if mutation == "manifest":
        payload["manifest"]["tables"]["backups"]["count"] = 123
    elif mutation == "missing-table":
        del payload["tables"]["schedules"]
    elif mutation == "unknown-table":
        payload["tables"]["users"] = []
    elif mutation == "unknown-column":
        payload["tables"]["backups"][0]["unknown"] = True
    elif mutation == "foreign-key":
        payload["tables"]["backups"][0]["target_id"] = 987
    elif mutation == "remote-id":
        payload["tables"]["backup_targets"][0]["remote_storage_ids"] = [987]
    elif mutation == "duplicate-id":
        payload["tables"]["schedules"].append(
            copy.deepcopy(payload["tables"]["schedules"][0])
        )
    elif mutation == "version":
        payload["schema_version"] = 999
    elif mutation == "application":
        payload["application"] = "other"
    elif mutation == "traversal":
        payload["tables"]["backups"][0]["file_path"] += "/../../escape"
    elif mutation == "outside-root":
        payload["tables"]["backups"][0]["file_path"] = "/etc/shadow"
    elif mutation == "sidecar":
        payload["tables"]["backups"][0]["encryption_key_path"] = "/etc/shadow"
    elif mutation == "absolute-remote":
        payload["tables"]["backup_storage_syncs"][0]["remote_path"] = "/etc/shadow"
    elif mutation == "bad-cron":
        payload["tables"]["schedules"][0]["cron_expression"] = "not a cron"
    elif mutation == "bad-type":
        payload["tables"]["retention_policies"][0]["keep_last"] = True
    elif mutation == "unknown-setting":
        payload["tables"]["app_settings"][0]["key"] = "raw_environment"
    if mutation != "manifest":
        payload["manifest"] = module.state_manifest(payload)
    with pytest.raises(module.RecoveryStateError) as error:
        await module.validate_state(payload)
    assert SECRET not in str(error.value)
    after = await module.export_state(populated)
    assert before["tables"] == after["tables"]


@pytest.mark.asyncio
async def test_native_encrypted_archive_sidecar_paths_are_preserved(populated):
    module = service()
    payload = await module.export_state(populated)
    archive = payload["tables"]["backups"][0]
    archive["file_path"] = payload["backup_root"] + "/plan/native.tar.gz.enc"
    archive["encryption_key_path"] = payload["backup_root"] + "/plan/native.tar.gz.key"
    payload["manifest"] = module.state_manifest(payload)
    assert (await module.validate_state(payload))["counts"]["backups"] == 1


@pytest.mark.asyncio
async def test_symlink_catalog_path_rejected(populated, tmp_path):
    module = service()
    root = tmp_path / "archives"
    root.mkdir()
    (root / "plan").symlink_to(tmp_path, target_is_directory=True)
    with pytest.raises(module.RecoveryStateError):
        await module.validate_state(await module.export_state(populated))
