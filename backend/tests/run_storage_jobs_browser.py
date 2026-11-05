"""Real production-image storage operations: browser, SQLite, disk and process loss.

Usage: python tests/run_storage_jobs_browser.py PRODUCT_IMAGE PLAYWRIGHT_RUNNER_IMAGE
Synthetic fixtures only, no Docker socket or production mounts in either container.
"""

import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid

import docker


# Test-only sitecustomize in the disposable container, never in product source.
HOLD = """
import time
from pathlib import Path
from app import storage_operations
original = storage_operations.delete_local_target
def held(root, path, **kwargs):
    key = ('held' if path == 'held-delete' else 'cancel' if path == 'held-cancel-one' else 'restart' if path == 'restart-delete' else None)
    if key is None:
        return original(root, path, **kwargs)
    # Restart scenario interrupts AFTER the actual effect but BEFORE the DB outcome.
    result = original(root, path, **kwargs) if key == 'restart' else None
    marker = Path('/app/data/fixture-' + key)
    marker.with_suffix('.entered').touch()
    deadline = time.monotonic() + 120
    while not marker.with_suffix('.release').exists():
        if time.monotonic() > deadline:
            raise RuntimeError('Owned fixture hold timed out')
        time.sleep(0.02)
    return result if result is not None else original(root, path, **kwargs)
storage_operations.delete_local_target = held
"""
FIXTURE = """
from pathlib import Path
root = Path('/backups')
for i in range(30):
    (root / ('extra-' + str(i).zfill(2))).write_bytes(b'preserved scrolling fixture')
nested = root / 'nested-delete' / 'nested'
nested.mkdir(parents=True)
(nested / 'data').write_bytes(b'x' * 1024)
(nested.parent / 'data').write_bytes(b'x' * 512)
sentinel = Path('/app/data/sentinel')
sentinel.write_bytes(b'unchanged synthetic sentinel')
(nested / 'external').symlink_to(sentinel)
(root / 'leaf-link').symlink_to(sentinel)
for name in ['ordinary-file','held-delete','held-future','held-cancel-one','held-cancel-two','restart-delete','restart-future']:
    (root / name).write_bytes(b'synthetic owned data')
locked = root / 'locked'
locked.mkdir()
(locked / 'data').write_bytes(b'locked owned data')
locked.chmod(0)
folder = root / 'context-folder'
folder.mkdir()
(folder / 'context-file').write_bytes(b'context data')
configured = Path('/app/data/configured/configured-folder')
configured.mkdir(parents=True)
(configured / 'child').write_bytes(b'configured child')
(configured.parent / 'configured-file').write_bytes(b'configured file')
"""
VERIFY = """
from pathlib import Path
import sqlite3, json
root = Path('/backups')
for name in ['nested-delete','leaf-link','locked','held-delete','held-future','held-cancel-one','restart-delete','context-folder/context-file']:
    assert not (root / name).exists(), name
assert (root / 'ordinary-file').read_bytes() == b'new file after original success'
for name in ['held-cancel-two','restart-future']:
    assert (root / name).read_bytes() == b'synthetic owned data'
assert Path('/app/data/sentinel').read_bytes() == b'unchanged synthetic sentinel'
assert not Path('/app/data/configured/configured-folder').exists()
assert not Path('/app/data/configured/configured-file').exists()
connection = sqlite3.connect('/app/data/backup.db')
rows = connection.execute('SELECT state,total,completed,succeeded,failed,skipped,unknown FROM storage_operations').fetchall()
assert len(rows) == 7, rows
assert all(total == completed == succeeded + failed + skipped + unknown for _,total,completed,succeeded,failed,skipped,unknown in rows)
assert connection.execute('PRAGMA foreign_key_check').fetchall() == []
assert connection.execute('SELECT MAX(version) FROM schema_migrations').fetchone() == (7,)
print(json.dumps({'physical_disk': 'passed', 'sentinel': 'unchanged', 'no_success_replay': True, 'no_pending_replay': True, 'sqlite_operation_count': len(rows), 'counters': rows}))
"""


def archive_files(files):
    content = io.BytesIO()
    with tarfile.open(fileobj=content, mode="w") as archive:
        for name, body in files.items():
            info = tarfile.TarInfo(name)
            encoded = body.encode()
            info.size = len(encoded)
            info.mode = 0o644
            archive.addfile(info, io.BytesIO(encoded))
    return content.getvalue()


def main():
    product, browser_image = sys.argv[1:3]
    client = docker.from_env()
    name = "dv-storage-142-" + uuid.uuid4().hex[:12]
    backend = client.containers.create(
        product,
        name=name,
        auto_remove=True,
        network_mode="none",
        labels={"dockervault.test": "storage-142"},
        environment={
            "COOKIE_SECURE": "false",
            "KOMODO_ENABLED": "false",
            "PYTHONPATH": "/app/test-fixture:/app",
        },
    )
    runner = None

    def execute(code):
        result = backend.exec_run(["python", "-c", code], user="dockervault")
        if result.exit_code:
            raise RuntimeError(result.output.decode())
        return result.output.decode()

    def health():
        for _ in range(90):
            result = backend.exec_run(["curl", "-fsS", "http://127.0.0.1/health"])
            if result.exit_code == 0:
                return
            time.sleep(0.5)
        raise RuntimeError("Production health unavailable")

    results = []

    def phase(stage, intervention=None):
        with tempfile.TemporaryDirectory(
            prefix="dv142-browser-", dir=os.environ.get("TMPDIR")
        ) as folder:
            output = Path(folder) / "output.log"
            with output.open("w") as log:
                process = subprocess.Popen(
                    [
                        "docker",
                        "exec",
                        "-e",
                        "PHASE=" + stage,
                        runner.id,
                        "node",
                        "/runner/browser_storage_jobs.mjs",
                    ],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
                try:
                    if intervention:
                        for _ in range(200):
                            if "CHECKPOINT polling-ready" in output.read_text():
                                intervention()
                                break
                            if process.poll() is not None:
                                raise RuntimeError(output.read_text())
                            time.sleep(0.1)
                        else:
                            raise RuntimeError(
                                "Polling browser did not reach controlled checkpoint"
                            )
                    process.wait(timeout=120)
                    text = output.read_text()
                    print(text, flush=True)
                    if process.returncode:
                        raise RuntimeError("Browser phase failed: " + stage)
                    parsed = [
                        json.loads(line)
                        for line in text.splitlines()
                        if line.startswith('{"status":')
                    ]
                    assert len(parsed) == 1
                    results.append(parsed[0])
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=10)

    try:
        backend.put_archive(
            "/app", archive_files({"test-fixture/sitecustomize.py": HOLD})
        )
        backend.start()
        health()
        execute(FIXTURE)
        runner = client.containers.run(
            browser_image,
            name=name + "-browser",
            detach=True,
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "storage-142"},
        )
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_storage_jobs.mjs": Path(__file__)
                    .with_name("browser_storage_jobs.mjs")
                    .read_text()
                }
            ),
        )
        phase("ordinary")
        execute(
            "from pathlib import Path; Path('/backups/locked').chmod(0o700); Path('/backups/ordinary-file').write_bytes(b'new file after original success')"
        )
        phase("retry")
        for prefix in ("held", "cancel"):
            phase(prefix + "-start")
            execute(
                f"from pathlib import Path; assert Path('/app/data/fixture-{prefix}.entered').exists(); assert Path('/backups/{'held-delete' if prefix == 'held' else 'held-cancel-one'}').exists()"
            )
            phase(
                prefix + "-settle",
                lambda prefix=prefix: execute(
                    f"from pathlib import Path; Path('/app/data/fixture-{prefix}.release').touch()"
                ),
            )
        phase("restart-start")
        execute(
            "from pathlib import Path; assert Path('/app/data/fixture-restart.entered').exists(); assert not Path('/backups/restart-delete').exists(); assert Path('/backups/restart-future').exists()"
        )
        result = backend.exec_run(["supervisorctl", "signal", "KILL", "backend"])
        if result.exit_code:
            raise RuntimeError(result.output.decode())
        # Wait for the NEW process to have durably resolved the uncertain window.
        for _ in range(100):
            status = execute(
                "import sqlite3; c=sqlite3.connect('/app/data/backup.db'); print(c.execute(\"SELECT count(*) FROM storage_operations WHERE stage='interrupted'\").fetchone()[0])"
            ).strip()
            if status == "1":
                break
            time.sleep(0.2)
        else:
            raise RuntimeError("Restart recovery did not resolve operation")
        health()
        phase("restart-check")
        print(execute(VERIFY), flush=True)
        print(
            json.dumps(
                {
                    "status": "passed",
                    "browser_checks": sum(len(r["checks"]) for r in results),
                    "phases": results,
                }
            ),
            flush=True,
        )
    finally:
        if runner is not None:
            try:
                runner.stop(timeout=3)
            except docker.errors.NotFound:
                pass
        try:
            backend.stop(timeout=10)
        except docker.errors.NotFound:
            pass
        client.close()


if __name__ == "__main__":
    main()
