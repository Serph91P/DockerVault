"""Real product restore acceptance in an offline, disposable native Docker daemon.

Usage: python tests/run_restore_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS OUT
Only the outer test daemon is privileged; no host bind/socket/network is passed.
"""

from pathlib import Path
import io
import json
import subprocess
import sys
import tarfile
import time
import uuid
import docker
from run_storage_jobs_browser import archive_files

DIND = "sha256:ee47d1044cff7319b4e03e4bcdc7d1599202951de3b5e85549c1b63365b8e2b6"
BUSYBOX = "sha256:c6348fa86ba0fb2108c9334f5fe913ddc6d853313e655891f133a0127c30099f"


def main():
    product, browser_image, axe, destination = sys.argv[1:5]
    out = Path(destination)
    out.mkdir(parents=True, exist_ok=True)
    client = docker.from_env()
    owned = []
    daemon = None
    backend = None
    runner = None
    prefix = "dv186-" + uuid.uuid4().hex[:10]
    socket = client.volumes.create(
        name=prefix + "-socket", labels={"dockervault.test": "restore186"}
    )

    def record(container):
        container.reload()
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
        assert not container.attrs["HostConfig"]["Binds"], "No host binds allowed"
        (out / "owned.json").write_text(json.dumps(owned, indent=2))

    def execute(container, cmd):
        result = container.exec_run(cmd)
        if result.exit_code:
            raise AssertionError(result.output.decode(errors="replace"))
        return result.output

    try:
        (out / "images.json").write_text(
            json.dumps(
                {
                    name: client.images.get(image).id
                    for name, image in [
                        ("product", product),
                        ("browser", browser_image),
                        ("dind", DIND),
                        ("busybox", BUSYBOX),
                    ]
                },
                indent=2,
            )
        )
        daemon = client.containers.create(
            DIND,
            name=prefix + "-daemon",
            privileged=True,
            auto_remove=True,
            network_mode="none",
            entrypoint="dockerd",
            command=[
                "--host=unix:///testsocket/docker.sock",
                "--iptables=false",
                "--ip6tables=false",
                "--bridge=none",
                "--storage-driver=vfs",
            ],
            environment={"DOCKER_TLS_CERTDIR": ""},
            mounts=[docker.types.Mount("/testsocket", socket.name, type="volume")],
            labels={"dockervault.test": "restore186"},
        )
        record(daemon)
        daemon.start()
        for _ in range(90):
            if (
                daemon.exec_run(
                    ["docker", "-H", "unix:///testsocket/docker.sock", "info"]
                ).exit_code
                == 0
            ):
                break
            time.sleep(1)
        else:
            raise AssertionError("Isolated native daemon unavailable")
        saved = subprocess.run(
            ["docker", "save", BUSYBOX], check=True, stdout=subprocess.PIPE
        ).stdout
        loaded = subprocess.run(
            [
                "docker",
                "exec",
                "-i",
                daemon.id,
                "docker",
                "-H",
                "unix:///testsocket/docker.sock",
                "load",
            ],
            input=saved,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=True,
        )
        (out / "offline-load.log").write_bytes(loaded.stdout)
        backend = client.containers.create(
            product,
            name=prefix + "-app",
            auto_remove=True,
            network_mode="none",
            volumes_from=[daemon.id],
            entrypoint=["python", "/app/data/runtime_restore_app.py"],
            environment={
                "COOKIE_SECURE": "false",
                "KOMODO_ENABLED": "false",
                "PYTHONPATH": "/app",
                "DOCKER_SOCKET": "/testsocket/docker.sock",
            },
            labels={"dockervault.test": "restore186"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_restore_app.py": Path(__file__)
                    .with_name("runtime_restore_app.py")
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
            raise AssertionError("Native restore app unhealthy")
        runner = client.containers.create(
            browser_image,
            name=prefix + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "restore186"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_restore.mjs": Path(__file__)
                    .with_name("browser_restore.mjs")
                    .read_text(),
                    "axe.min.js": Path(axe).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_restore.mjs"])
        (out / "browser.log").write_bytes(result.output)
        print(result.output.decode(), flush=True)
        for path in [
            "restore186-result.json",
            "restore-failure.png",
            *[f"restore-{w}.png" for w in (320, 390, 768, 1024, 1440)],
        ]:
            try:
                stream, _ = runner.get_archive("/runner/" + path)
                with tarfile.open(fileobj=io.BytesIO(b"".join(stream))) as archive:
                    (out / path).write_bytes(archive.extractfile(path).read())
            except docker.errors.NotFound:
                pass
        assert result.exit_code == 0, "Native restore browser acceptance failed"
        effects = execute(
            backend,
            [
                "python",
                "-c",
                "import sqlite3;from pathlib import Path;c=sqlite3.connect('/app/data/backup.db');assert not c.execute('pragma foreign_key_check').fetchall();assert c.execute(\"select count(*) from backups where status='completed'\").fetchone()[0]==3;assert Path('/backups/restore186/source/unrelated').read_bytes()==b'keep unrelated bytes';assert Path('/backups/restore186/unknown/nested/file.txt').read_bytes()==b'owned nested restore bytes\\n';print('PASS SQLite3 real completed engine runs / nested restored bytes / noncolliding bytes preserved')",
            ],
        )
        (out / "effects.log").write_bytes(effects)
    finally:
        for name, container in [("app", backend), ("daemon", daemon)]:
            if container:
                try:
                    (out / (name + ".log")).write_bytes(container.logs())
                except docker.errors.NotFound:
                    pass
        for item in reversed(owned):
            try:
                client.containers.get(item["id"]).remove(force=True, v=True)
            except docker.errors.NotFound:
                pass
        volumes = {socket.name, *[v for c in owned for v in c["volumes"]]}
        for name in volumes:
            try:
                client.volumes.get(name).remove()
            except docker.errors.NotFound:
                pass
        for item in owned:
            try:
                client.containers.get(item["id"])
                raise AssertionError("Owned container remains")
            except docker.errors.NotFound:
                pass
        for name in volumes:
            try:
                client.volumes.get(name)
                raise AssertionError("Owned volume remains")
            except docker.errors.NotFound:
                pass
        (out / "cleanup.json").write_text(
            json.dumps(
                {"all_absent": True, "containers": owned, "volumes": sorted(volumes)},
                indent=2,
            )
        )


if __name__ == "__main__":
    main()
