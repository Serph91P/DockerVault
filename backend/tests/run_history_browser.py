"""Run #185 seven-AC acceptance in two owned isolated disposable containers."""

from pathlib import Path
import json
import io
import tarfile
import hashlib
import sys
import time
import uuid
import docker
from run_storage_jobs_browser import archive_files


def main():
    product, browser_image, axe, destination = sys.argv[1:5]
    out = Path(destination)
    out.mkdir(parents=True, exist_ok=True)
    client = docker.from_env()
    owned = []
    backend = None
    name = "dv185-" + uuid.uuid4().hex[:12]

    def record(container):
        container.reload()
        owned.append(
            {
                "id": container.id,
                "name": container.name,
                "volumes": [
                    m["Name"]
                    for m in container.attrs["Mounts"]
                    if m["Type"] == "volume"
                ],
            }
        )
        (out / "owned.json").write_text(json.dumps(owned, indent=2))

    try:
        (out / "inputs.json").write_text(
            json.dumps(
                {
                    "product": client.images.get(product).id,
                    "browser": client.images.get(browser_image).id,
                    "source_sha256": {
                        name: hashlib.sha256(
                            Path(__file__).with_name(name).read_bytes()
                        ).hexdigest()
                        for name in (
                            "run_history_browser.py",
                            "runtime_history_app.py",
                            "browser_history.mjs",
                        )
                    },
                    "axe_sha256": hashlib.sha256(Path(axe).read_bytes()).hexdigest(),
                },
                indent=2,
            )
        )
        backend = client.containers.create(
            product,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_history_app.py"],
            environment={
                "COOKIE_SECURE": "false",
                "KOMODO_ENABLED": "false",
                "PYTHONPATH": "/app",
            },
            labels={"dockervault.test": "history185"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_history_app.py": Path(__file__)
                    .with_name("runtime_history_app.py")
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
            raise RuntimeError("History fixture unhealthy")
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "history185"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_history.mjs": Path(__file__)
                    .with_name("browser_history.mjs")
                    .read_text(),
                    "axe.min.js": Path(axe).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_history.mjs"])
        (out / "browser.log").write_bytes(result.output)
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "History browser acceptance failed"
        result_file = runner.exec_run(["cat", "/runner/history185-result.json"])
        assert result_file.exit_code == 0
        (out / "result.json").write_bytes(result_file.output)
        for surface in ("list", "detail"):
            for width in (320, 1440):
                filename = f"history-{surface}-{width}.png"
                archive, _ = runner.get_archive("/runner/" + filename)
                with tarfile.open(fileobj=io.BytesIO(b"".join(archive))) as images:
                    (out / filename).write_bytes(images.extractfile(filename).read())
        effects = backend.exec_run(
            [
                "python",
                "-c",
                "import sqlite3,tarfile;from pathlib import Path;c=sqlite3.connect('/app/data/backup.db');assert c.execute('pragma foreign_key_check').fetchall()==[];assert c.execute('select count(*) from backups').fetchone()[0]==513;assert c.execute('select removed_at from backup_targets where name=?',('Retained history source',)).fetchone()[0];p=Path('/app/data/history185/remote/retained/remote.tar');a=tarfile.open(p);assert a.extractfile('root/file-2004.txt').read()==b'owned history payload 2004';assert Path('/app/data/history185/plain.tar').is_file();assert not Path('/app/data/history185/local.tar').exists();restored=list(Path('/backups/history-restored').rglob('file-2004.txt'));assert len(restored)==1 and restored[0].read_bytes()==b'owned history payload 2004';print('PASS native SQLite513/tombstone/TAR bytes/restore bytes/confirmed local removal/source preservation')",
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
            try:
                client.containers.get(item["id"])
                raise AssertionError("Owned container remains")
            except docker.errors.NotFound:
                pass
            for volume in item["volumes"]:
                try:
                    client.volumes.get(volume)
                    raise AssertionError("Owned volume remains")
                except docker.errors.NotFound:
                    pass
        (out / "cleanup.json").write_text(
            json.dumps({"all_absent": True, "owned": owned}, indent=2)
        )
        client.close()


if __name__ == "__main__":
    main()
