"""Fresh processes reconcile external lifecycle intent and accepted jobs."""

import json
import os
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.mark.parametrize(
    "phase", ["intent", "stop", "archive", "restart", "upload", "terminal"]
)
def test_process_loss_recovers_owned_workloads_and_resolves_accepted_jobs(
    tmp_path, phase
):
    backend = Path(__file__).resolve().parents[1]
    probe = backend / "tests/runtime_recovery_probe.py"
    env = {**os.environ, "PYTHONPATH": str(backend)}

    def run(mode):
        return subprocess.run(
            [sys.executable, str(probe), str(tmp_path), mode, phase],
            cwd=backend,
            env=env,
            capture_output=True,
            text=True,
            timeout=40,
        )

    crash = run("crash")
    assert crash.returncode == 81, crash.stdout + crash.stderr
    recovered = run("recover")
    assert recovered.returncode == 0, recovered.stdout + recovered.stderr
    first = json.loads(recovered.stdout.strip().splitlines()[-1])
    assert first["workloads"]["running"]["state"] == "running"
    assert first["workloads"]["running"]["starts"] == 1
    assert first["workloads"]["pre-stopped"] == {
        "state": "exited",
        "starts": 0,
        "stops": 0,
    }
    assert first["statuses"] == ["failed", "failed"]
    assert all(first["errors"])
    assert first["queue"] == 0
    if phase == "upload":
        assert len(first["remote_files"]) == 1
    repeated = run("recover")
    assert repeated.returncode == 0, repeated.stdout + repeated.stderr
    assert json.loads(repeated.stdout.strip().splitlines()[-1]) == first
