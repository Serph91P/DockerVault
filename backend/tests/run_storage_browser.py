"""Run production-image storage deletion/size acceptance on disposable data only.

Usage: python tests/run_storage_browser.py PRODUCT_IMAGE PLAYWRIGHT_RUNNER_IMAGE
The runner needs Playwright installed in /runner; no production mounts or sockets.
"""

import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import time
import uuid

import docker


FIXTURE = """
from pathlib import Path
root = Path('/backups')
nested = root / 'synthetic-backup' / 'nested'
nested.mkdir(parents=True)
(nested / 'data').write_bytes(b'x' * 1024)
(root / 'synthetic-backup' / 'data').write_bytes(b'x' * 512)
sentinel = Path('/app/data/sentinel')
sentinel.write_bytes(b'unchanged synthetic sentinel')
(nested / 'external').symlink_to(sentinel)
(root / 'parent').symlink_to(sentinel.parent, target_is_directory=True)
(root / 'dangling').symlink_to('/app/data/missing')
for name in ('first', 'last'):
    (root / name).write_bytes(b'delete')
locked = root / 'locked'
locked.mkdir()
(locked / 'data').write_bytes(b'protected synthetic data')
locked.chmod(0)
mounted = Path('/app/data/mounted/mounted-backup/nested')
mounted.mkdir(parents=True)
(mounted / 'data').write_bytes(b'0123456789')
"""
VERIFY = """
from pathlib import Path
import json, os
root = Path('/backups')
assert not (root / 'synthetic-backup').exists()
assert not (root / 'first').exists() and not (root / 'last').exists()
assert not Path('/app/data/mounted/mounted-backup').exists()
assert Path('/app/data/sentinel').read_bytes() == b'unchanged synthetic sentinel'
assert (root / 'parent').is_symlink() and os.path.lexists(root / 'dangling')
(root / 'locked').chmod(0o700)
assert (root / 'locked/data').read_bytes() == b'protected synthetic data'
print(json.dumps({'disk_effects': 'passed', 'sentinel': 'unchanged', 'protected_file': 'unchanged', 'directories': 'removed'}))
"""


def main():
    product, runner = sys.argv[1:3]
    client = docker.from_env()
    name = "dv-storage-141-" + uuid.uuid4().hex[:12]
    backend = client.containers.run(
        product,
        name=name,
        detach=True,
        auto_remove=True,
        network_mode="none",
        labels={"dockervault.test": "storage-141"},
        environment={"COOKIE_SECURE": "false", "KOMODO_ENABLED": "false"},
    )
    try:
        for _ in range(60):
            result = backend.exec_run(["curl", "-fsS", "http://127.0.0.1/health"])
            if result.exit_code == 0:
                break
            time.sleep(1)
        else:
            raise RuntimeError("Production-image health did not become ready")
        print("PASS real production nginx/backend health", flush=True)
        result = backend.exec_run(["python", "-c", FIXTURE], user="dockervault")
        if result.exit_code:
            raise RuntimeError(result.output.decode())
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode="w") as archive:
            archive.add(
                Path(__file__).with_name("browser_storage.mjs"),
                arcname="browser_storage.mjs",
            )
        process = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "-i",
                "--name",
                name + "-browser",
                "--network",
                "container:" + backend.id,
                "--entrypoint",
                "sh",
                runner,
                "-ec",
                "tar -xf - -C /runner; exec node /runner/browser_storage.mjs",
            ],
            input=content.getvalue(),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=180,
        )
        print(process.stdout.decode(errors="replace"), flush=True)
        if process.returncode:
            raise RuntimeError("Real browser acceptance failed")
        result = backend.exec_run(["python", "-c", VERIFY], user="dockervault")
        if result.exit_code:
            raise RuntimeError(result.output.decode())
        print(json.dumps(json.loads(result.output)), flush=True)
    finally:
        try:
            leftover = client.containers.get(name + "-browser")
            leftover.stop(timeout=5)
        except docker.errors.NotFound:
            pass
        try:
            backend.stop(timeout=10)
        except docker.errors.NotFound:
            pass
        client.close()


if __name__ == "__main__":
    main()
