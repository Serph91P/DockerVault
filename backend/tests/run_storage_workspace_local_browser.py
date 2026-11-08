"""Run exact source/build bytes on the accepted dependency runtime, not a new image.

Canonical Dockerfile build remains a separate mandatory, currently blocked check.
No Dockerfile/CI edits, production sockets, host mounts or auth bypass.
"""

from pathlib import Path
import io
import json
import sys
import tarfile
import time
import uuid

import docker
from run_storage_jobs_browser import archive_files

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
product = (
    sys.argv[2]
    if len(sys.argv) > 2
    else "sha256:d6e883efbf161bed7a54850fbd75832541c4f81f46b76a592dc0121bc2d7f396"
)
client = docker.from_env()
owned = []
backend = runner = None
name = "dv189-source-" + uuid.uuid4().hex[:10]


def record(container):
    container.reload()
    assert not container.attrs["HostConfig"]["Binds"]
    owned.append(
        {
            "id": container.id,
            "volumes": [
                mount["Name"]
                for mount in container.attrs["Mounts"]
                if mount["Type"] == "volume"
            ],
        }
    )
    (out / "owned.json").write_text(json.dumps(owned, indent=2))


def archive_paths(root):
    content = io.BytesIO()
    with tarfile.open(fileobj=content, mode="w") as archive:
        for path in root.rglob("*"):
            if path.is_file() and "__pycache__" not in path.parts:
                archive.add(path, arcname=path.relative_to(root))
    return content.getvalue()


try:
    backend = client.containers.create(
        product,
        name=name,
        auto_remove=True,
        network_mode="none",
        user="dockervault",
        entrypoint=["python", "/app/data/runtime_storage_workspace_local.py"],
        command=["/app/data/source189", "/usr/share/nginx/html"],
        environment={"PYTHONPATH": "/app"},
        labels={"dockervault.test": "storage189-source"},
    )
    record(backend)
    backend.put_archive(
        "/app/data",
        archive_files(
            {
                "runtime_storage_workspace_local.py": Path(__file__)
                .with_name("runtime_storage_workspace_local.py")
                .read_text()
            }
        ),
    )
    if len(sys.argv) <= 2:
        backend.put_archive(
            "/app/app", archive_paths(Path(__file__).parents[1] / "app")
        )
        backend.put_archive(
            "/usr/share/nginx/html",
            archive_paths(Path(__file__).parents[2] / "frontend/dist"),
        )
    (out / "images.json").write_text(
        json.dumps(
            {
                "product": client.images.get(product).id,
                "source_overlay_diagnostic": len(sys.argv) <= 2,
            },
            indent=2,
        )
    )
    backend.start()
    for _ in range(90):
        if (
            backend.exec_run(["curl", "-fsS", "http://127.0.0.1:8000/health"]).exit_code
            == 0
        ):
            break
        time.sleep(0.5)
    else:
        raise AssertionError("Source fixture unhealthy: " + backend.logs().decode())
    runner = client.containers.create(
        "sha256:87ac02f52c1e0c24a35cf64449f51d17da5b8d571ffb8b10474434093655e473",
        name=name + "-browser",
        auto_remove=True,
        network_mode="container:" + backend.id,
        entrypoint="sh",
        command=["-c", "sleep 900"],
        environment={"FIXTURE_ROOT": "/app/data/source189"},
        labels={"dockervault.test": "storage189-source"},
    )
    record(runner)
    runner.put_archive(
        "/runner",
        archive_files(
            {
                "browser_storage_workspace.mjs": Path(__file__)
                .with_name("browser_storage_workspace.mjs")
                .read_text(),
                "axe.min.js": Path(
                    "/opt/data/agent/artifacts/dockervault-187-local/d89c723/tools/axe.min.js"
                ).read_text(),
            }
        ),
    )
    runner.start()
    result = runner.exec_run(["node", "/runner/browser_storage_workspace.mjs"])
    (out / "browser.log").write_bytes(result.output)
    print(result.output.decode(), flush=True)
    runner.exec_run(
        [
            "sh",
            "-c",
            "cd /runner && tar cf /runner/evidence.tar storage*.png axe-storage*.json storage189-source-result.json 2>/dev/null; true",
        ]
    )
    archive, _ = runner.get_archive("/runner/evidence.tar")
    with tarfile.open(fileobj=io.BytesIO(b"".join(archive))) as outer:
        payload = outer.extractfile("evidence.tar").read()
    with tarfile.open(fileobj=io.BytesIO(payload)) as evidence:
        for member in evidence.getmembers():
            if member.isfile():
                (out / Path(member.name).name).write_bytes(
                    evidence.extractfile(member).read()
                )
    effects = backend.exec_run(
        [
            "python",
            "-c",
            "import sqlite3,json; from pathlib import Path; root=Path('/app/data/source189'); c=sqlite3.connect(root/'backup.db'); print(json.dumps({'storages':c.execute('select id,name,storage_type,enabled,base_path from remote_storages').fetchall(),'operations':c.execute('select id,state,total,succeeded,failed,unknown from storage_operations').fetchall(),'foreign_key_errors':c.execute('pragma foreign_key_check').fetchall(),'legacy_probe_preserved':(root/'backups/.backup_test').read_bytes()==b'legacy name must survive','files':[str(p.relative_to(root/'backups')) for p in (root/'backups').rglob('*')]}))",
        ]
    )
    (out / "effects.log").write_bytes(effects.output)
    assert result.exit_code == 0, "Source-runtime storage browser acceptance failed"
    assert effects.exit_code == 0, effects.output.decode()
finally:
    if backend:
        (out / "backend.log").write_bytes(backend.logs())
    for container in [runner, backend]:
        if container:
            try:
                container.stop(timeout=3)
            except docker.errors.NotFound:
                pass
    absent = False
    for _ in range(100):
        ids = {container["Id"] for container in client.api.containers(all=True)}
        volumes = {volume["Name"] for volume in client.api.volumes().get("Volumes", [])}
        absent = all(
            row["id"] not in ids
            and all(volume not in volumes for volume in row["volumes"])
            for row in owned
        )
        if absent:
            break
        time.sleep(0.05)
    (out / "cleanup.json").write_text(
        json.dumps({"owned": owned, "all_absent": absent}, indent=2)
    )
    assert absent
