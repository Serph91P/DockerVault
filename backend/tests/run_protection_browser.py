"""#184 real cookie/API/SQLite/file browser acceptance on disposable fixtures only.

Usage: python tests/run_protection_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS
The product image is unmodified. Synthetic discovery/lifecycle is injected only
in the owned fixture entrypoint; no Docker socket or production data is mounted.
"""

from pathlib import Path
import sys
import time
import uuid

import docker
from run_storage_jobs_browser import archive_files


def main():
    product, browser_image, axe_path = sys.argv[1:4]
    client = docker.from_env()
    backend = runner = None
    name = "dv184-" + uuid.uuid4().hex[:12]
    try:
        backend = client.containers.create(
            product,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_protection_app.py"],
            environment={
                "COOKIE_SECURE": "false",
                "KOMODO_ENABLED": "false",
                "PYTHONPATH": "/app",
                "ALLOWED_HOOK_COMMANDS": "pg_dump,/usr/bin/touch",
            },
            labels={"dockervault.test": "ux184"},
        )
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_protection_app.py": Path(__file__)
                    .with_name("runtime_protection_app.py")
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
            raise RuntimeError("Owned fixture not healthy")
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "ux184"},
        )
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_protection.mjs": Path(__file__)
                    .with_name("browser_protection.mjs")
                    .read_text(),
                    "axe.min.js": Path(axe_path).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_protection.mjs"])
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "Protection browser assertions failed"
        # Read stored rows/files independently from the browser/API transport.
        verify = backend.exec_run(
            [
                "python",
                "-c",
                """
import json, sqlite3, tarfile
from pathlib import Path
c=sqlite3.connect('/app/data/backup.db')
assert c.execute('PRAGMA foreign_key_check').fetchall()==[]
assert c.execute("select count(*) from remote_storages where storage_type='sftp' and enabled=0").fetchone()[0]==1
rows=c.execute("select file_path,status from backups order by id").fetchall()
# UI runs the four original sources with gzip, then explicitly changes path to TAR.
assert len(rows)==5 and all(status=='COMPLETED' for _,status in rows), rows
for index,(filename,_) in enumerate(rows):
    compressed=index<4
    assert Path(filename).read_bytes().startswith(bytes((31, 139))) is compressed
    assert filename.endswith('.tar.gz' if compressed else '.tar')
    with tarfile.open(filename, mode='r:gz' if compressed else 'r:') as archive:
        files={m.name:archive.extractfile(m).read() for m in archive.getmembers() if m.isfile()}
    assert files and all(content==b'owned nested protection fixture' for content in files.values()), files
    assert all(name.endswith('config/keep.txt') for name in files), files
    for destination in ('remote-one','remote-two'):
        copy=Path('/app/data/184-fixtures')/destination/Path(filename).parent.name/Path(filename).name
        assert copy.read_bytes()==Path(filename).read_bytes()
assert Path('/app/data/184-fixtures/path/config/keep.txt').read_bytes()==b'owned nested protection fixture'
assert Path('/backups/184-path-restored/config/keep.txt').read_bytes()==b'owned nested protection fixture'
assert Path('/app/data/184-fixtures/pre-hook-effect').is_file()
assert Path('/app/data/184-fixtures/post-hook-effect').is_file()
print(json.dumps({'sqlite':'passed','archives':len(rows),'actual_members_and_bytes':'passed','formats':'4 explicit r:gz + gzip magic; 1 explicit r: + non-gzip magic','restored_bytes':'passed','discovery_lifecycle':'explicit synthetic doubles; not Docker/Komodo/provider evidence'}))
""",
            ]
        )
        assert verify.exit_code == 0, verify.output.decode()
        print(verify.output.decode(), flush=True)
    except BaseException:
        if backend:
            print(
                "Owned product logs withheld: hook commands must not enter acceptance artifacts.",
                flush=True,
            )
        raise
    finally:
        for container in (runner, backend):
            if container:
                try:
                    container.stop(timeout=5)
                    container.wait(condition="removed", timeout=10)
                except docker.errors.NotFound:
                    pass
        for container in (runner, backend):
            if container:
                try:
                    client.containers.get(container.id)
                    raise AssertionError("Owned acceptance resource remains")
                except docker.errors.NotFound:
                    pass
        client.close()


if __name__ == "__main__":
    main()
