"""Production HTTP/disk regressions for late discovery, rejection and lost 202.

Usage: python tests/run_storage_jobs_recovery_browser.py PRODUCT_IMAGE BROWSER_IMAGE
"""

from pathlib import Path
import sys
import time
import uuid

import docker

from run_storage_jobs_browser import archive_files


FIXTURE = """
from pathlib import Path
Path('/app/data/sentinel').write_bytes(b'preserved sentinel')
if {rejection}:
    root = Path('/backups/folder')
    root.mkdir()
    for index in range(1001):
        (root / ('item-' + str(index))).write_bytes(b'preserve')
else:
    Path('/backups/recovery-file').write_bytes(b'physical recovery fixture')
"""
VERIFY = """
from pathlib import Path
import sqlite3, json
assert Path('/app/data/sentinel').read_bytes() == b'preserved sentinel'
if {rejection}:
    files = list(Path('/backups/folder').iterdir())
    assert len(files) == 1 and files[0].name == 'item-0'
    assert files[0].read_bytes() == b'preserve'
else:
    assert not Path('/backups/recovery-file').exists()
c = sqlite3.connect('/app/data/backup.db')
rows = c.execute('SELECT state,total,completed,succeeded,failed,skipped,unknown FROM storage_operations').fetchall()
assert rows == [('completed', {total}, {total}, {total}, 0, 0, 0)], rows
assert c.execute('SELECT count(*) FROM storage_operation_items WHERE state="succeeded"').fetchone()[0] == {total}
assert c.execute('PRAGMA foreign_key_check').fetchall() == []
print(json.dumps(dict(physical_disk='passed', sentinel='unchanged', sqlite=rows)))
"""


def main():
    product, browser_image = sys.argv[1:3]
    client = docker.from_env()
    try:
        for scenario in ("discovery", "rejection", "ambiguous"):
            backend = runner = None
            name = "dv142-recovery-" + uuid.uuid4().hex[:12]
            try:
                backend = client.containers.run(
                    product,
                    name=name,
                    auto_remove=True,
                    detach=True,
                    network_mode="none",
                    environment={"COOKIE_SECURE": "false", "KOMODO_ENABLED": "false"},
                    labels={"dockervault.test": "storage-142-recovery"},
                )
                for _ in range(90):
                    result = backend.exec_run(
                        ["curl", "-fsS", "http://127.0.0.1/health"]
                    )
                    if result.exit_code == 0:
                        break
                    time.sleep(0.5)
                else:
                    raise RuntimeError("Owned product not healthy")
                result = backend.exec_run(
                    ["python", "-c", FIXTURE.format(rejection=scenario == "rejection")],
                    user="dockervault",
                )
                assert result.exit_code == 0, result.output
                runner = client.containers.run(
                    browser_image,
                    name=name + "-browser",
                    auto_remove=True,
                    detach=True,
                    network_mode="container:" + backend.id,
                    entrypoint="sh",
                    command=["-c", "sleep 300"],
                    labels={"dockervault.test": "storage-142-recovery"},
                )
                script = "browser_storage_jobs_recovery.mjs"
                runner.put_archive(
                    "/runner",
                    archive_files(
                        {script: Path(__file__).with_name(script).read_text()}
                    ),
                )
                result = runner.exec_run(
                    ["node", "/runner/" + script], environment={"SCENARIO": scenario}
                )
                print(result.output.decode(), flush=True)
                assert result.exit_code == 0, result.exit_code
                result = backend.exec_run(
                    [
                        "python",
                        "-c",
                        VERIFY.format(
                            rejection=scenario == "rejection",
                            total=1000 if scenario == "rejection" else 1,
                        ),
                    ]
                )
                assert result.exit_code == 0, result.output
                print(result.output.decode(), flush=True)
            finally:
                for container in (runner, backend):
                    if container:
                        try:
                            container.stop(timeout=10)
                        except docker.errors.NotFound:
                            pass
    finally:
        client.close()


if __name__ == "__main__":
    main()
