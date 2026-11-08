"""Run #188 native product acceptance with owned --rm containers and no host binds.

Usage: python tests/run_overview_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS OUT
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


def main():
    product, browser, axe, destination = sys.argv[1:5]
    out = Path(destination)
    out.mkdir(parents=True, exist_ok=True)
    client = docker.from_env()
    owned = []
    backend = runner = None
    name = "dv188-" + uuid.uuid4().hex[:12]

    def record(container):
        container.reload()
        assert not container.attrs["HostConfig"]["Binds"]
        owned.append(
            {
                "id": container.id,
                "volumes": [
                    m["Name"]
                    for m in container.attrs["Mounts"]
                    if m["Type"] == "volume"
                ],
            }
        )
        (out / "owned.json").write_text(json.dumps(owned, indent=2))

    try:
        (out / "images.json").write_text(
            json.dumps(
                {
                    key: client.images.get(value).id
                    for key, value in [("product", product), ("browser", browser)]
                },
                indent=2,
            )
        )
        backend = client.containers.create(
            product,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_overview_app.py"],
            environment={
                "COOKIE_SECURE": "false",
                "KOMODO_ENABLED": "false",
                "PYTHONPATH": "/app",
            },
            labels={"dockervault.test": "overview188"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_overview_app.py": Path(__file__)
                    .with_name("runtime_overview_app.py")
                    .read_text()
                }
            ),
        )
        backend.start()
        for _ in range(90):
            if (
                backend.exec_run(["curl", "-fsS", "http://127.0.0.1/health"]).exit_code
                == 0
            ):
                break
            time.sleep(0.5)
        else:
            raise AssertionError("Overview fixture unhealthy")
        runner = client.containers.create(
            browser,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "overview188"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_overview.mjs": Path(__file__)
                    .with_name("browser_overview.mjs")
                    .read_text(),
                    "axe.min.js": Path(axe).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_overview.mjs"])
        (out / "browser.log").write_bytes(result.output)
        print(result.output.decode(), flush=True)
        # Preserve failure screenshots and successful checkpoints before evaluating exit status.
        result_files = runner.exec_run(
            [
                "sh",
                "-c",
                "cd /runner && tar cf /runner/evidence.tar overview-*.png axe-*.json overview188-result.json 2>/dev/null; true",
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
        assert result.exit_code == 0, "Overview browser acceptance failed"
        effects = backend.exec_run(
            [
                "python",
                "-c",
                "import sqlite3; c=sqlite3.connect('/app/data/backup.db'); assert c.execute('pragma foreign_key_check').fetchall()==[]; rows=c.execute('select id,status,file_path from backups').fetchall(); assert any(r[1]=='COMPLETED' and r[2] for r in rows); print('PASS native SQLite/FKs/completed archive records',rows)",
            ]
        )
        (out / "effects.log").write_bytes(effects.output)
        assert effects.exit_code == 0, effects.output.decode()
    finally:
        if backend:
            try:
                (out / "backend.log").write_bytes(backend.logs())
            except docker.errors.NotFound:
                pass
        for item in reversed(owned):
            try:
                client.containers.get(item["id"]).remove(force=True, v=True)
            except docker.errors.NotFound:
                pass
            for volume in item["volumes"]:
                try:
                    client.volumes.get(volume).remove()
                except docker.errors.NotFound:
                    pass
        for item in owned:
            for kind, identities in [
                ("containers", [item["id"]]),
                ("volumes", item["volumes"]),
            ]:
                for identity in identities:
                    try:
                        getattr(client, kind).get(identity)
                        raise AssertionError("Owned resource remains: " + identity)
                    except docker.errors.NotFound:
                        pass
        (out / "cleanup.json").write_text(
            json.dumps({"all_absent": True, "owned": owned}, indent=2)
        )
        client.close()


if __name__ == "__main__":
    main()
