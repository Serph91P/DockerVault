"""Ordered, once-only upgrades against a static historical schema and real SQLite."""

import sqlite3
from pathlib import Path
from unittest.mock import patch

import pytest
import pytest_asyncio
from sqlalchemy import text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import database


@pytest_asyncio.fixture
async def legacy_database(tmp_path):
    path = tmp_path / "legacy.sqlite"
    connection = sqlite3.connect(path)
    try:
        connection.executescript(
            (Path(__file__).parent / "fixtures/legacy_pre_schedule.sql").read_text()
        )
        connection.executescript("""
            ALTER TABLE backup_targets ADD COLUMN legacy_extension TEXT;
            INSERT INTO remote_storages (id,name,storage_type,enabled,password,base_path)
                VALUES (7,'legacy storage','local',0,'opaque:legacy-ciphertext','/synthetic/remote');
            INSERT INTO backup_targets (id,name,target_type,enabled,sync_to_remote,remote_storage_ids,legacy_extension)
                VALUES (41,'legacy target','path',0,0,'[7]','unknown future value');
            INSERT INTO backups (id,target_id,backup_type,status,file_path,backup_metadata)
                VALUES (51,41,'FULL','COMPLETED',NULL,'{"unknown":"preserve","parent_id":19}');
            INSERT INTO backup_storage_syncs (id,backup_id,storage_id,remote_path,sync_status)
                VALUES (61,51,7,'original/archive.tar','completed'), (62,999,7,'orphan/archive.tar','completed');
            INSERT INTO backup_logs (id,backup_id,step,message)
                VALUES (71,51,'complete','original'), (72,999,'complete','orphan');
        """)
    finally:
        connection.close()
    engine = create_async_engine(f"sqlite+aiosqlite:///{path}")
    try:
        with (
            patch.object(database, "engine", engine),
            patch.object(
                database,
                "async_session",
                async_sessionmaker(engine, expire_on_commit=False),
            ),
            patch.object(database.settings, "BACKUP_BASE_PATH", str(tmp_path)),
        ):
            yield engine, path
    finally:
        await engine.dispose()


async def schema_snapshot(engine):
    async with engine.connect() as connection:
        schema = (
            await connection.execute(
                text(
                    "SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name"
                )
            )
        ).all()
        target = (await connection.execute(text("SELECT * FROM backup_targets"))).all()
        return schema, target


@pytest.mark.asyncio
async def test_fresh_database_has_ordered_migration_ledger(tmp_path):
    engine = create_async_engine(f"sqlite+aiosqlite:///{tmp_path}/fresh.sqlite")
    try:
        with (
            patch.object(database, "engine", engine),
            patch.object(
                database,
                "async_session",
                async_sessionmaker(engine, expire_on_commit=False),
            ),
            patch.object(database.settings, "BACKUP_BASE_PATH", str(tmp_path)),
        ):
            await database.init_db()
            async with engine.connect() as connection:
                names = (
                    (
                        await connection.execute(
                            text("SELECT name FROM sqlite_master WHERE type='table'")
                        )
                    )
                    .scalars()
                    .all()
                )
                assert "schema_migrations" in names
                assert (
                    await connection.execute(
                        text("SELECT version FROM schema_migrations ORDER BY version")
                    )
                ).scalars().all() == [1, 2]
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_supported_legacy_upgrade_preserves_rows_and_does_not_replay(
    legacy_database,
):
    engine, _ = legacy_database
    await database.run_migrations()
    async with engine.begin() as connection:
        names = (
            (
                await connection.execute(
                    text("SELECT name FROM sqlite_master WHERE type='table'")
                )
            )
            .scalars()
            .all()
        )
        assert "schema_migrations" in names
        assert (
            await connection.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2]
        assert (
            await connection.execute(
                text(
                    "SELECT sync_to_remote,legacy_extension FROM backup_targets WHERE id=41"
                )
            )
        ).one() == (1, "unknown future value")
        assert (
            await connection.execute(
                text("SELECT password FROM remote_storages WHERE id=7")
            )
        ).scalar_one() == "opaque:legacy-ciphertext"
        assert (
            await connection.execute(
                text("SELECT backup_metadata FROM backups WHERE id=51")
            )
        ).scalar_one() == '{"unknown":"preserve","parent_id":19}'
        assert (
            await connection.execute(
                text("SELECT remote_path FROM backup_storage_syncs WHERE id=61")
            )
        ).scalar_one() == "original/archive.tar"
        assert (
            await connection.execute(text("SELECT count(*) FROM catalog_quarantine"))
        ).scalar_one() == 2
        await connection.execute(
            text("UPDATE backup_targets SET sync_to_remote=0 WHERE id=41")
        )
    before = await schema_snapshot(engine)
    await database.run_migrations()
    assert await schema_snapshot(engine) == before
    async with engine.connect() as connection:
        assert (
            await connection.execute(
                text("SELECT sync_to_remote FROM backup_targets WHERE id=41")
            )
        ).scalar_one() == 0
        assert (await connection.execute(text("PRAGMA foreign_key_check"))).all() == []
        target_columns = {
            row[1]
            for row in (
                await connection.execute(text("PRAGMA table_info(backup_targets)"))
            ).all()
        }
        assert set(database.BackupTarget.__table__.columns.keys()) <= target_columns


@pytest.mark.asyncio
async def test_failed_upgrade_rolls_back_ddl_data_and_ledger_then_retries(
    legacy_database,
):
    engine, _ = legacy_database
    before = await schema_snapshot(engine)
    original = database._repair_catalog_orphans

    async def fail_after_repair(connection):
        await original(connection)
        raise RuntimeError("injected migration interruption")

    with patch.object(database, "_repair_catalog_orphans", fail_after_repair):
        with pytest.raises(RuntimeError, match="injected migration interruption"):
            await database.run_migrations()
    assert await schema_snapshot(engine) == before
    await database.run_migrations()
    async with engine.connect() as connection:
        assert (
            await connection.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2]
        assert (
            await connection.execute(text("SELECT count(*) FROM catalog_quarantine"))
        ).scalar_one() == 2


@pytest.mark.asyncio
async def test_failed_required_migration_prevents_lifespan_readiness(legacy_database):
    from app import main

    entered = False

    async def fail(_connection):
        raise RuntimeError("required migration failed")

    with (
        patch.object(database, "_repair_catalog_orphans", fail),
        patch.object(main, "_enforce_key_file_permissions"),
    ):
        with pytest.raises(RuntimeError, match="required migration failed"):
            async with main.lifespan(main.app):
                entered = True
    assert entered is False


@pytest.mark.asyncio
@pytest.mark.parametrize("versions", [(2,), (1, 2, 3)])
async def test_unknown_or_gapped_ledger_fails_without_mutation(test_engine, versions):
    async with test_engine.begin() as connection:
        await connection.execute(
            text(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT)"
            )
        )
        for version in versions:
            await connection.execute(
                text("INSERT INTO schema_migrations (version) VALUES (:version)"),
                {"version": version},
            )
    before = await schema_snapshot(test_engine)
    with patch.object(database, "engine", test_engine):
        with pytest.raises(
            RuntimeError, match="Unsupported or incomplete migration ledger"
        ):
            await database.run_migrations()
    assert await schema_snapshot(test_engine) == before


@pytest.mark.asyncio
async def test_real_process_exit_during_upgrade_rolls_back_and_retries(legacy_database):
    import os
    import subprocess
    import sys

    engine, path = legacy_database
    before = await schema_snapshot(engine)
    environment = dict(
        os.environ,
        DATABASE_URL=f"sqlite+aiosqlite:///{path}",
        BACKUP_BASE_PATH=str(path.parent),
    )
    code = """
import asyncio
import os
from app import database
original = database._repair_catalog_orphans
async def interrupted(connection):
    await original(connection)
    os._exit(23)
database._repair_catalog_orphans = interrupted
asyncio.run(database.run_migrations())
"""
    result = subprocess.run(
        [sys.executable, "-c", code],
        cwd=Path(__file__).resolve().parents[1],
        env=environment,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert result.returncode == 23, result.stderr
    assert await schema_snapshot(engine) == before
    await database.run_migrations()
    async with engine.connect() as connection:
        assert (
            await connection.execute(
                text("SELECT version FROM schema_migrations ORDER BY version")
            )
        ).scalars().all() == [1, 2]
        assert (
            await connection.execute(text("SELECT count(*) FROM catalog_quarantine"))
        ).scalar_one() == 2
