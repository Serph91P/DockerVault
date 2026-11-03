"""Run both real removal pages against an owned exact-source runtime and restart it.

Usage: python tests/run_removal_browser.py CANDIDATE_IMAGE RUNNER_IMAGE LEGACY_ASSETS
The legacy assets are built from frontend/tests/removal.html using Vite. No host
ports, production volumes or Docker socket are exposed to either container.
"""

import io
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tarfile
import uuid

import docker


def bundle(files):
    content = io.BytesIO()
    with tarfile.open(fileobj=content, mode="w") as tar:
        for path, name in files:
            tar.add(path, arcname=name)
    return content.getvalue()


def main():
    candidate, runner, legacy_assets = sys.argv[1:4]
    source = Path(__file__).resolve().parent
    assets = Path(legacy_assets).resolve()
    client = docker.from_env()
    name = "dv-ux180-" + uuid.uuid4().hex[:12]
    password = secrets.token_urlsafe(32)
    backend = client.containers.create(
        candidate,
        name=name,
        auto_remove=True,
        entrypoint=["python", "/app/data/runtime_removal_fixture.py"],
        network_mode="none",
        labels={"dockervault.test": "ux180-removal"},
        environment={
            "DV_REMOVAL_ROOT": "/app/data/removal-fixture",
            "PYTHONPATH": "/app",
            "DATABASE_URL": "sqlite+aiosqlite:////app/data/removal-fixture/db.sqlite",
            "BACKUP_BASE_PATH": "/app/data/removal-fixture",
            "COOKIE_SECURE": "false",
            "KOMODO_ENABLED": "false",
        },
    )
    try:
        backend.put_archive(
            "/app/data",
            bundle(
                [(source / "runtime_removal_fixture.py", "runtime_removal_fixture.py")]
            ),
        )
        backend.put_archive(
            "/usr/share/nginx/html",
            bundle(
                [
                    (path, str(path.relative_to(assets)))
                    for path in assets.rglob("*")
                    if path.is_file()
                ]
            ),
        )
        backend.start()
        for phase in ("plans", "copies"):
            if phase == "copies":
                backend.restart(timeout=10)
                print(
                    "PASS owned application process restarted with same isolated SQLite/disk",
                    flush=True,
                )
            result = subprocess.run(
                [
                    "docker",
                    "run",
                    "--rm",
                    "-i",
                    "--name",
                    name + "-runner",
                    "--label",
                    "dockervault.test=ux180-removal",
                    "--network",
                    "container:" + backend.id,
                    "--env",
                    "DV_FIXTURE_PASSWORD",
                    "--entrypoint",
                    "sh",
                    runner,
                    "-ec",
                    f"tar -xf - -C /runner; exec node browser_removal.mjs {phase}",
                ],
                input=bundle([(source / "browser_removal.mjs", "browser_removal.mjs")]),
                env=os.environ | {"DV_FIXTURE_PASSWORD": password},
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=240,
            )
            print(result.stdout.decode(errors="replace"), end="", flush=True)
            if result.returncode:
                raise RuntimeError(
                    f"Rendered removal acceptance ({phase}) exited {result.returncode}"
                )
    finally:
        try:
            instance = client.containers.get(name + "-runner")
            assert instance.labels.get("dockervault.test") == "ux180-removal"
            instance.remove(force=True)
        except docker.errors.NotFound:
            pass
        try:
            backend.reload()
            assert backend.labels.get("dockervault.test") == "ux180-removal"
            # Fixture never logs request bodies or the synthetic private key.
            print(backend.logs().decode(errors="replace")[-6000:])
            backend.stop(timeout=10)
            backend.wait(condition="removed", timeout=10)
        except docker.errors.NotFound:
            pass


if __name__ == "__main__":
    main()
