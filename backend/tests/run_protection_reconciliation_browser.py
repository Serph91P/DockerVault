"""Real #184 reconciliation regression; disposable fixtures, no production sockets.

Usage: python tests/run_protection_reconciliation_browser.py PRODUCT_IMAGE BROWSER_IMAGE
"""

from pathlib import Path
import sys
import time
import uuid

import docker
from run_storage_jobs_browser import archive_files


def main():
    product, browser_image = sys.argv[1:3]
    client = docker.from_env()
    backend = runner = None
    name = "dv184-reconcile-" + uuid.uuid4().hex[:10]
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
            },
            labels={"dockervault.test": "ux184-reconciliation"},
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
            raise RuntimeError("Owned fixture unavailable")
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 600"],
            labels={"dockervault.test": "ux184-reconciliation"},
        )
        script = "browser_protection_reconciliation.mjs"
        runner.put_archive(
            "/runner",
            archive_files({script: Path(__file__).with_name(script).read_text()}),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/" + script])
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "Reconciliation browser assertions failed"
        verify = backend.exec_run(
            [
                "python",
                "-c",
                """
import json, sqlite3
c=sqlite3.connect('/app/data/backup.db')
assert c.execute('PRAGMA foreign_key_check').fetchall()==[]
assert c.execute('select count(*) from backups').fetchone()[0]==0
assert c.execute('select count(*) from schedules').fetchone()[0]==2
assert c.execute('select count(*) from retention_policies').fetchone()[0]==2
rows=c.execute('select id,name,schedule_id,retention_policy_id from backup_targets order by id').fetchall()
assert len(rows)==6, rows
assert rows[2]==(3,'Reconciled retention',1,2), rows
assert rows[3][:3]==(4,'Reconciled target',2), rows
assert [r[1] for r in rows[4:]]==['Discarded ambiguity']*2
# The browser verifies private values without printing them. SQLite independently checks preservation.
pre,post=c.execute('select pre_backup_command,post_backup_command from backup_targets where id=3').fetchone()
assert pre==post and pre.startswith('  pg_dump ') and pre.endswith('  ')
print(json.dumps({'sqlite':'passed','targets':len(rows),'correct_policy_ids':[1,2], 'schedules':2,'retention_policies':2,'backups':0,'private_hook_preservation':'passed'}))
""",
            ]
        )
        assert verify.exit_code == 0, verify.output.decode()
        print(verify.output.decode(), flush=True)
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
                    raise AssertionError("Owned container remains")
                except docker.errors.NotFound:
                    pass
        client.close()


if __name__ == "__main__":
    main()
