"""Run #181 browser/SQLite/age journeys on owned, network-isolated containers.

Usage: .venv/bin/python tests/run_ux181_acceptance.py IMAGE RUNNER EVIDENCE [SCENARIO...]
No production data, published ports, Docker socket mounts or auth bypasses.
"""

import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import uuid

import docker

source = Path(__file__).resolve().parent
image, runner, destination, *scenarios = sys.argv[1:]
scenarios = scenarios or ["encrypted", "skip", "lost"]
evidence = Path(destination)
evidence.parent.mkdir(parents=True, exist_ok=True)
client = docker.from_env()
results = []


def archive(*filenames):
    content = io.BytesIO()
    with tarfile.open(fileobj=content, mode="w") as tar:
        for filename in filenames:
            tar.add(source / filename, arcname=filename)
    return content.getvalue()


try:
    for scenario in scenarios:
        assert scenario in ("encrypted", "skip", "lost")
        name = "dv-ux181-" + uuid.uuid4().hex[:12]
        label = {
            "dockervault.test": "ux181",
            "dockervault.task": os.environ.get("HERMES_KANBAN_TASK", "ux181"),
        }
        backend = client.containers.create(
            image,
            name=name,
            auto_remove=True,
            network_mode="none",
            labels=label,
            entrypoint=["python", "/app/data/runtime_ux181_app.py"],
            environment={
                "DATABASE_URL": "sqlite+aiosqlite:////app/data/ux181/backup.db",
                "BACKUP_BASE_PATH": "/backups",
                "COOKIE_SECURE": "true",
                "CORS_ORIGINS": "https://127.0.0.1:8443",
                "KOMODO_ENABLED": "false",
                # Harmless synthetic hook roundtrip; no production allowlist change.
                "ALLOWED_HOOK_COMMANDS": "true",
                "LOG_LEVEL": "WARNING",
                "PYTHONPATH": "/app",
            },
        )
        volumes = [
            row["Name"] for row in backend.attrs["Mounts"] if row["Type"] == "volume"
        ]
        (evidence.parent / f"resources-{scenario}.json").write_text(
            json.dumps({"container": backend.id, "volumes": volumes})
        )
        try:
            backend.put_archive(
                "/app/data", archive("runtime_ux181_app.py", "runtime_ux181_verify.py")
            )
            backend.start()
            run = subprocess.run(
                [
                    "docker",
                    "run",
                    "--rm",
                    "-i",
                    "--name",
                    name + "-runner",
                    "--label",
                    "dockervault.test=ux181",
                    "--network",
                    "container:" + backend.id,
                    "--volumes-from",
                    backend.id,
                    "--env",
                    "DV_SCENARIO=" + scenario,
                    "--entrypoint",
                    "sh",
                    runner,
                    "-ec",
                    "tar -xf - -C /runner; exec node browser_ux181.mjs",
                ],
                input=archive("browser_ux181.mjs"),
                capture_output=True,
                timeout=240,
            )
            output = run.stdout.decode(errors="replace") + run.stderr.decode(
                errors="replace"
            )
            (evidence.parent / f"browser-{scenario}.log").write_text(output)
            print(output, end="", flush=True)
            assert run.returncode == 0, f"Browser scenario {scenario} failed"
            if scenario != "skip":
                verify = backend.exec_run(
                    ["python", "/app/data/runtime_ux181_verify.py"]
                )
                print(verify.output.decode(errors="replace"), end="", flush=True)
                assert verify.exit_code == 0, (
                    "Real exported identity verification failed"
                )
            results.append(
                {
                    "scenario": scenario,
                    "browser": "passed",
                    "offline_age_recovery": scenario != "skip",
                    "image_id": backend.image.id,
                }
            )
        finally:
            try:
                leftover = client.containers.get(name + "-runner")
                assert leftover.labels.get("dockervault.test") == "ux181"
                leftover.stop(timeout=5)
            except docker.errors.NotFound:
                pass
            try:
                (evidence.parent / f"backend-{scenario}.log").write_bytes(
                    backend.logs()
                )
                backend.stop(timeout=10)
                backend.wait(condition="removed", timeout=15)
            except docker.errors.NotFound:
                pass
            for volume in volumes:
                try:
                    owned = client.volumes.get(volume)
                except docker.errors.NotFound:
                    continue
                assert not client.containers.list(
                    all=True, filters={"volume": volume}
                ), "Owned test volume is still in use"
                try:
                    owned.remove()
                except docker.errors.NotFound:
                    # AutoRemove can finish deleting an anonymous volume after get().
                    pass
                try:
                    client.volumes.get(volume)
                except docker.errors.NotFound:
                    continue
                raise AssertionError("Owned test volume cleanup failed")
        evidence.write_text(json.dumps(results, indent=2) + "\n")
finally:
    client.close()
