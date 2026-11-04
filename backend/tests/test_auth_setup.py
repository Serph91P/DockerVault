"""Atomic initial setup across real independent processes and SQLite connections."""

import json
import os
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

import pytest
import pytest_asyncio
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from app import database

PROBE = Path(__file__).with_name("runtime_auth_probe.py")
BACKEND = Path(__file__).resolve().parents[1]


@pytest_asyncio.fixture
async def auth_database(tmp_path):
    path = tmp_path / "auth.sqlite"
    engine = create_async_engine(f"sqlite+aiosqlite:///{path}")
    with patch.object(database, "engine", engine):
        await database.run_migrations()
    environment = dict(
        os.environ,
        DATABASE_URL=f"sqlite+aiosqlite:///{path}",
        BACKUP_BASE_PATH=str(tmp_path),
        PYTHONPATH=str(BACKEND),
    )
    try:
        yield engine, environment
    finally:
        await engine.dispose()


def result_from_output(output):
    records = [
        json.loads(line.removeprefix("RESULT="))
        for line in output.splitlines()
        if line.startswith("RESULT=")
    ]
    assert len(records) == 1, output
    return records[0]


@pytest.mark.asyncio
@pytest.mark.parametrize("repetition", range(3))
async def test_only_one_setup_wins_across_processes(auth_database, repetition):
    engine, environment = auth_database
    processes = []
    try:
        for index in range(2):
            processes.append(
                subprocess.Popen(
                    [
                        sys.executable,
                        str(PROBE),
                        "race",
                        f"synthetic-admin-{repetition}-{index}",
                    ],
                    cwd=BACKEND,
                    env=environment,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
            )
        for process in processes:
            assert process.stdout.readline().strip() == "READY"
        for process in processes:
            process.stdin.write("!")
            process.stdin.flush()
        results = []
        for process in processes:
            stdout, stderr = process.communicate(timeout=20)
            assert process.returncode == 0, stderr
            results.append(result_from_output(stdout))
        assert sorted(result["setup"] for result in results) == [200, 400]
        assert sum(result["session_cookie"] for result in results) == 1
        async with engine.connect() as connection:
            assert (
                await connection.execute(text("SELECT count(*) FROM users"))
            ).scalar_one() == 1
            assert (
                await connection.execute(text("SELECT count(*) FROM sessions"))
            ).scalar_one() == 1
    finally:
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=20)


@pytest.mark.asyncio
async def test_real_sequential_auth_contract_still_works(auth_database):
    engine, environment = auth_database
    process = subprocess.run(
        [sys.executable, str(PROBE), "sequential", "synthetic-admin"],
        cwd=BACKEND,
        env=environment,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert process.returncode == 0, process.stderr
    result = result_from_output(process.stdout)
    assert result == {
        "setup": 200,
        "session_cookie": True,
        "cookie_httponly": True,
        "cookie_secure": True,
        "cookie_samesite": True,
        "repeat_setup": 400,
        "me_after_setup": 200,
        "logout": 200,
        "me_after_logout": 401,
        "login": 200,
        "change_password": 200,
        "me_after_change": 200,
        "old_password": 401,
        "new_password": 200,
    }
    async with engine.connect() as connection:
        assert (
            await connection.execute(text("SELECT count(*) FROM users"))
        ).scalar_one() == 1
        assert (
            await connection.execute(text("SELECT count(*) FROM sessions"))
        ).scalar_one() == 1


@pytest.mark.asyncio
async def test_setup_session_failure_does_not_leave_an_admin(auth_database):
    engine, environment = auth_database
    process = subprocess.run(
        [sys.executable, str(PROBE), "session-failure", "synthetic-admin"],
        cwd=BACKEND,
        env=environment,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert process.returncode == 0, process.stderr
    assert result_from_output(process.stdout)["setup"] == 500
    async with engine.connect() as connection:
        assert (
            await connection.execute(text("SELECT count(*) FROM users"))
        ).scalar_one() == 0
        assert (
            await connection.execute(text("SELECT count(*) FROM sessions"))
        ).scalar_one() == 0
