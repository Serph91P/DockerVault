"""Run TLS/cookie browser acceptance using only disposable, owned containers.

Usage: python tests/run_browser_acceptance.py CANDIDATE_IMAGE RUNNER_IMAGE EVIDENCE_JSON
Build RUNNER_IMAGE with tests/browser.Dockerfile. No host ports/socket mounts.
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
candidate, runner, evidence_path = sys.argv[1:4]
destination = Path(evidence_path)
destination.parent.mkdir(parents=True, exist_ok=True)
client = docker.from_env()
labels = {
    "dockervault.test": "websocket-session",
    "dockervault.task": os.environ.get("HERMES_KANBAN_TASK", "ci-browser"),
}
name = "dv-browser-" + uuid.uuid4().hex[:12]
root = "/app/data/browser-acceptance"


def archive(filename):
    content = io.BytesIO()
    with tarfile.open(fileobj=content, mode="w") as tar:
        tar.add(source / filename, arcname=filename)
    return content.getvalue()


backend = client.containers.create(
    candidate,
    name=name,
    auto_remove=True,
    entrypoint=["python", "/app/data/runtime_browser_app.py"],
    network_mode="none",
    labels=labels,
    environment={
        "DV_BROWSER_ROOT": root,
        "PYTHONPATH": "/app",
        "DATABASE_URL": f"sqlite+aiosqlite:///{root}/backup.db",
        "BACKUP_BASE_PATH": f"{root}/backups",
        "COOKIE_SECURE": "true",
        "CORS_ORIGINS": "https://127.0.0.1:8443",
        "KOMODO_ENABLED": "false",
    },
)
try:
    backend.put_archive("/app/data", archive("runtime_browser_app.py"))
    backend.start()
    result = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "-i",
            "--name",
            name + "-runner",
            "--label",
            "dockervault.test=websocket-session",
            "--label",
            "dockervault.task=" + labels["dockervault.task"],
            "--network",
            "container:" + backend.id,
            "--entrypoint",
            "sh",
            runner,
            "-ec",
            "tar -xf - -C /runner; node --version; npm --version; exec node browser_websocket.mjs",
        ],
        input=archive("browser_websocket.mjs"),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=120,
    )
    print(result.stdout.decode(errors="replace"), end="", flush=True)
    if result.returncode:
        raise RuntimeError(f"browser acceptance exited {result.returncode}")
    stream, _ = backend.get_archive(root + "/evidence.json")
    with tarfile.open(fileobj=io.BytesIO(b"".join(stream))) as tar:
        rows = json.load(tar.extractfile("evidence.json"))
    foreign = [row for row in rows if row["origin"] == "https://127.0.0.1:8444"]
    assert foreign and all(
        row["cookie_present"] and not row["accepted"] and row["denial_code"] == 4003
        for row in foreign
    )
    assert any(row["accepted"] and row["cookie_present"] for row in rows)
    assert any(not row["cookie_present"] and row["denial_code"] == 4001 for row in rows)
    destination.write_text(json.dumps(rows, indent=2) + "\n")
    print(
        "PASS independent ASGI evidence: foreign same-site browser DID send cookie and was denied; anonymous denied; same-origin accepted"
    )
finally:
    try:
        remaining_runner = client.containers.get(name + "-runner")
        assert remaining_runner.labels.get("dockervault.test") == "websocket-session"
        remaining_runner.remove(force=True)
        remaining_runner.wait(condition="removed", timeout=10)
    except docker.errors.NotFound:
        pass
    try:
        logs = backend.logs().decode(errors="replace")
        Path(evidence_path + ".server.log").write_text(logs)
        backend.reload()
        assert backend.labels.get("dockervault.test") == "websocket-session"
        backend.stop(timeout=10)
        backend.wait(condition="removed", timeout=10)
    except docker.errors.NotFound:
        pass
    try:
        client.containers.get(backend.id)
        raise AssertionError("owned browser backend remains")
    except docker.errors.NotFound:
        print("PASS owned browser backend/anonymous volumes removed; absence verified")
    client.close()
