"""Owner R7: legacy and already-ledgered schemas need real policy identity FKs."""

from contextlib import closing
from pathlib import Path
import sqlite3
from unittest.mock import patch

import pytest
from sqlalchemy import text
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import create_async_engine

from app import database


async def old_ledger(engine):
    """Reproduce the previously published v1/v2 state, not the new upgrader."""
    async with engine.begin() as connection:
        await connection.execute(text("BEGIN IMMEDIATE"))
        await connection.run_sync(database.Base.metadata.create_all)
        await database._upgrade_legacy_schema(connection)
        await database._repair_catalog_orphans(connection)
        await connection.execute(
            text(
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT)"
            )
        )
        await connection.execute(
            text("INSERT INTO schema_migrations VALUES (1,'legacy'),(2,'legacy')")
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("already_migrated", [False, True])
async def test_legacy_policy_constraints_prevent_id_reuse_and_preserve_unknown_data(
    tmp_path, already_migrated
):
    path = tmp_path / "legacy.sqlite"
    with closing(sqlite3.connect(path)) as raw, raw:
        raw.executescript(
            (Path(__file__).parent / "fixtures/legacy_pre_schedule.sql").read_text()
        )
        raw.executescript("""
          ALTER TABLE backup_targets ADD COLUMN legacy_extension TEXT;
          CREATE INDEX legacy_lookup ON backup_targets(legacy_extension);
          CREATE TABLE legacy_events(value TEXT);
          CREATE TRIGGER legacy_event AFTER UPDATE ON backup_targets BEGIN
            INSERT INTO legacy_events VALUES(NEW.legacy_extension);
          END;
          INSERT INTO backup_targets(id,name,target_type,legacy_extension) VALUES(41,'owned','path','preserve unknown');
          INSERT INTO backups(id,target_id,status) VALUES(51,41,'COMPLETED');
        """)
    engine = create_async_engine(f"sqlite+aiosqlite:///{path}")
    try:
        if already_migrated:
            await old_ledger(engine)
        with patch.object(database, "engine", engine):
            await database.run_migrations()
            await database.run_migrations()
        async with engine.begin() as connection:
            foreign_keys = {
                row[3]: row[2]
                for row in (
                    await connection.execute(
                        text("PRAGMA foreign_key_list(backup_targets)")
                    )
                ).all()
            }
            assert foreign_keys.get("schedule_id") == "schedules"
            assert foreign_keys.get("retention_policy_id") == "retention_policies"
            assert (
                await connection.execute(
                    text("SELECT legacy_extension FROM backup_targets WHERE id=41")
                )
            ).scalar_one() == "preserve unknown"
            assert (
                await connection.execute(
                    text("SELECT target_id FROM backups WHERE id=51")
                )
            ).scalar_one() == 41
            assert (
                await connection.execute(text("PRAGMA foreign_keys"))
            ).scalar_one() == 1
            assert (
                await connection.execute(text("PRAGMA foreign_key_check"))
            ).all() == []
            for column in ("schedule_id", "retention_policy_id"):
                with pytest.raises(IntegrityError):
                    await connection.execute(
                        text(f"UPDATE backup_targets SET {column}=999 WHERE id=41")
                    )
            await connection.execute(
                text(
                    "INSERT INTO retention_policies(id,name,keep_last) VALUES(1,'original',3)"
                )
            )
            await connection.execute(
                text("UPDATE backup_targets SET retention_policy_id=1 WHERE id=41")
            )
            with pytest.raises(IntegrityError):
                await connection.execute(
                    text("DELETE FROM retention_policies WHERE id=1")
                )
            await connection.execute(
                text("UPDATE backup_targets SET retention_policy_id=NULL WHERE id=41")
            )
            await connection.execute(text("DELETE FROM retention_policies WHERE id=1"))
            await connection.execute(
                text(
                    "INSERT INTO retention_policies(name,keep_last) VALUES('unrelated',1)"
                )
            )
            assert (
                await connection.execute(
                    text("SELECT retention_policy_id FROM backup_targets WHERE id=41")
                )
            ).scalar_one() is None
            assert (
                await connection.execute(text("SELECT count(*) FROM legacy_events"))
            ).scalar_one() >= 2
            assert (
                await connection.execute(
                    text(
                        "SELECT count(*) FROM sqlite_master WHERE name='legacy_lookup'"
                    )
                )
            ).scalar_one() == 1
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_invalid_legacy_reference_fails_closed_and_keeps_ledger_and_rows(
    tmp_path,
):
    path = tmp_path / "invalid.sqlite"
    with closing(sqlite3.connect(path)) as raw, raw:
        raw.executescript(
            (Path(__file__).parent / "fixtures/legacy_pre_schedule.sql").read_text()
        )
        raw.execute(
            "INSERT INTO backup_targets(id,name,target_type) VALUES(41,'owned','path')"
        )
    engine = create_async_engine(f"sqlite+aiosqlite:///{path}")
    try:
        await old_ledger(engine)
        async with engine.begin() as connection:
            await connection.execute(
                text("UPDATE backup_targets SET retention_policy_id=999 WHERE id=41")
            )
        with patch.object(database, "engine", engine):
            with pytest.raises(RuntimeError, match="reference"):
                await database.run_migrations()
        async with engine.connect() as connection:
            assert (
                await connection.execute(
                    text("SELECT version FROM schema_migrations ORDER BY version")
                )
            ).scalars().all() == [1, 2]
            assert (
                await connection.execute(
                    text("SELECT retention_policy_id FROM backup_targets WHERE id=41")
                )
            ).scalar_one() == 999
            assert (
                await connection.execute(text("PRAGMA foreign_keys"))
            ).scalar_one() == 1
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_real_process_loss_after_target_rebuild_rolls_back_then_retries(tmp_path):
    import os
    import subprocess
    import sys

    path = tmp_path / "interrupted.sqlite"
    with closing(sqlite3.connect(path)) as raw, raw:
        raw.executescript(
            (Path(__file__).parent / "fixtures/legacy_pre_schedule.sql").read_text()
        )
        raw.execute(
            "INSERT INTO backup_targets(id,name,target_type) VALUES(41,'owned','path')"
        )
    engine = create_async_engine(f"sqlite+aiosqlite:///{path}")
    try:
        await old_ledger(engine)
        async with engine.connect() as connection:
            before = (
                await connection.execute(
                    text("SELECT name,sql FROM sqlite_master ORDER BY name")
                )
            ).all()
        code = """
import asyncio, os
from app import database
original = database._ensure_target_foreign_keys
async def interrupted(connection):
    await original(connection)
    os._exit(27)
database._ensure_target_foreign_keys = interrupted
asyncio.run(database.run_migrations())
"""
        backend = Path(__file__).resolve().parents[1]
        result = subprocess.run(
            [sys.executable, "-c", code],
            cwd=tmp_path,
            env=os.environ
            | {
                "DATABASE_URL": f"sqlite+aiosqlite:///{path}",
                "PYTHONPATH": str(backend),
            },
            capture_output=True,
            text=True,
            timeout=20,
        )
        assert result.returncode == 27, result.stderr
        async with engine.connect() as connection:
            assert (
                await connection.execute(
                    text("SELECT name,sql FROM sqlite_master ORDER BY name")
                )
            ).all() == before
            assert (
                await connection.execute(
                    text("SELECT version FROM schema_migrations ORDER BY version")
                )
            ).scalars().all() == [1, 2]
        with patch.object(database, "engine", engine):
            await database.run_migrations()
        async with engine.connect() as connection:
            assert (
                await connection.execute(
                    text("SELECT version FROM schema_migrations ORDER BY version")
                )
            ).scalars().all() == [1, 2, 3]
            assert (
                len(
                    (
                        await connection.execute(
                            text("PRAGMA foreign_key_list(backup_targets)")
                        )
                    ).all()
                )
                == 2
            )
    finally:
        await engine.dispose()
