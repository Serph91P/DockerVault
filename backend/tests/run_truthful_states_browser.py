"""Exercise #183 against an immutable product image with real cookie/SQLite/files.

Usage: python tests/run_truthful_states_browser.py PRODUCT_SHA BROWSER_SHA AXE_JS
Only failure/partial transport states are injected. Happy paths use the real API.
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
    name = "dv183-" + uuid.uuid4().hex[:12]
    try:
        backend = client.containers.run(
            product,
            name=name,
            auto_remove=True,
            detach=True,
            network_mode="none",
            environment={"COOKIE_SECURE": "false", "KOMODO_ENABLED": "false"},
            labels={"dockervault.test": "ux183"},
        )
        for _ in range(90):
            if (
                backend.exec_run(["curl", "-fsS", "http://127.0.0.1/health"]).exit_code
                == 0
            ):
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("Isolated product did not become healthy")
        fixture = backend.exec_run(
            [
                "python",
                "-c",
                "from pathlib import Path; p=Path('/app/data/ux183-source');p.mkdir();(p/'data.txt').write_bytes(b'owned truthful states fixture');Path('/backups/notification-delete').write_bytes(b'owned disposable file');Path('/app/data/ux183-remote').mkdir()",
            ],
            user="dockervault",
        )
        assert fixture.exit_code == 0, fixture.output.decode()
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "ux183"},
        )
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_truthful_states.mjs": Path(__file__)
                    .with_name("browser_truthful_states.mjs")
                    .read_text(),
                    "axe.min.js": Path(axe_path).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_truthful_states.mjs"])
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "Product browser assertions failed"
        verification = backend.exec_run(
            [
                "python",
                "-c",
                "import sqlite3,json;from pathlib import Path;c=sqlite3.connect('/app/data/backup.db');assert c.execute('PRAGMA foreign_key_check').fetchall()==[];assert c.execute('select count(*) from backups where status=?',('COMPLETED',)).fetchone()[0]==61;assert Path('/app/data/ssh_keys/truthful_key').is_file();assert Path('/app/data/ssh_keys/truthful_key.pub').is_file();assert c.execute('select count(*) from storage_operations where state=?',('completed',)).fetchone()[0]>=1;assert not Path('/backups/notification-delete').exists();assert Path('/app/data/ux183-source/data.txt').read_bytes()==b'owned truthful states fixture';print(json.dumps({'sqlite':'passed','physical_deletion':'passed','source_bytes':'unchanged'}))",
            ]
        )
        assert verification.exit_code == 0, verification.output.decode()
        print(verification.output.decode(), flush=True)
    finally:
        for container in (runner, backend):
            if container is not None:
                try:
                    container.stop(timeout=5)
                except docker.errors.NotFound:
                    pass
        client.close()


if __name__ == "__main__":
    main()
