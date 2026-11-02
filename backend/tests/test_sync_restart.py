"""Persistent sync acceptance without mocks or shared in-process state."""

import os
from pathlib import Path
import subprocess
import sys
import pytest


@pytest.mark.parametrize("operation", ["sync", "retry"])
def test_sync_survives_real_process_restart(tmp_path, operation):
    backend = Path(__file__).resolve().parents[1]
    env = os.environ | {
        "DATABASE_URL": f"sqlite+aiosqlite:///{tmp_path}/catalog.db",
        "BACKUP_BASE_PATH": str(tmp_path),
        "PYTHONPATH": str(backend),
    }
    for mode in (operation, "recover"):
        result = subprocess.run(
            [
                sys.executable,
                str(backend / "tests/runtime_sync_probe.py"),
                mode,
                str(tmp_path),
            ],
            env=env,
            cwd=tmp_path,
            text=True,
            capture_output=True,
            timeout=30,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert f"{mode}: real auth/API" in result.stdout
