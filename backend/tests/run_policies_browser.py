"""Real #187 cookie/browser/API/SQLite/file gates, unique isolated containers.

Usage: python tests/run_policies_browser.py PRODUCT_IMAGE BROWSER_IMAGE AXE_JS OUT
No production Docker socket, host data, Komodo or external provider is mounted.
Fixture-only controls and injected faults are explicitly identified in the report.
"""

from pathlib import Path
import hashlib
import json
import sys
import time
import uuid

import docker
from run_storage_jobs_browser import archive_files


def main():
    product, browser_image, axe_path, out_path = sys.argv[1:5]
    out = Path(out_path)
    out.mkdir(parents=True, exist_ok=True)
    client = docker.from_env()
    owned = []
    backend = runner = None
    name = "dv187-" + uuid.uuid4().hex[:12]

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
        (out / "owned-resources.json").write_text(json.dumps(owned, indent=2))

    try:
        sources = {
            filename: Path(__file__).with_name(filename).read_text()
            for filename in (
                "runtime_policy_app.py",
                "runtime_protection_app.py",
                "browser_policies.mjs",
            )
        }
        (out / "source-manifest.json").write_text(
            json.dumps(
                {
                    name: hashlib.sha256(text.encode()).hexdigest()
                    for name, text in sources.items()
                },
                indent=2,
            )
        )
        backend = client.containers.create(
            product,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_policy_app.py"],
            environment={
                "COOKIE_SECURE": "false",
                "KOMODO_ENABLED": "false",
                "PYTHONPATH": "/app",
                "TZ": "America/New_York",
            },
            labels={"dockervault.test": "policies187"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {key: value for key, value in sources.items() if key.endswith(".py")}
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
            raise RuntimeError("Owned policy fixture not healthy")
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "policies187"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_policies.mjs": sources["browser_policies.mjs"],
                    "axe.min.js": Path(axe_path).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_policies.mjs"])
        (out / "browser.log").write_bytes(result.output)
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "Six-AC policy browser failed"
        raw = runner.exec_run(["cat", "/runner/policy187-result.json"])
        assert raw.exit_code == 0
        summary = json.loads(raw.output)
        (out / "result.json").write_text(json.dumps(summary, indent=2))
        # Independent final native SQLite and gzip bytes, not just HTTP 2xx.
        verify = backend.exec_run(
            [
                "python",
                "-c",
                """
import hashlib,json,sqlite3,tarfile
from pathlib import Path
c=sqlite3.connect('/app/data/backup.db')
assert c.execute('pragma foreign_key_check').fetchall()==[]
row=c.execute('select status,file_path,file_size from backups where id=?',("""
                + str(summary["runId"])
                + """,)).fetchone()
assert row and row[0]=='COMPLETED',row
p=Path(row[1]);assert p.stat().st_size==row[2]
with tarfile.open(p,'r:gz') as archive:
    payloads=[archive.extractfile(member).read() for member in archive.getmembers() if member.isfile()]
assert b'owned nested protection fixture' in payloads
root=Path('/backups/policy187-orphans')
assert (root/'catalog.tar.gz').read_bytes()==b'cataloged sentinel'
assert (root/'active.tar.gz.tmp').read_bytes()==b'active sentinel'
assert not (root/'orphan0.tar.gz').exists() and not (root/'orphan1.tar.gz').exists()
print(json.dumps({'foreign_key_violations':0,'real_gzip_archive_bytes':p.stat().st_size,'real_gzip_sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'sentinels_preserved':True,'final_orphans_absent':True}))
""",
            ]
        )
        (out / "independent-effects.log").write_bytes(verify.output)
        print(verify.output.decode(), flush=True)
        assert verify.exit_code == 0, "Independent policy disk/SQLite effects failed"
    except BaseException:
        if backend:
            (out / "backend.log").write_bytes(backend.logs())
        raise
    finally:
        for item in owned:
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
                raise AssertionError("Owned policy container remains")
            except docker.errors.NotFound:
                pass
            for volume in item["volumes"]:
                try:
                    client.volumes.get(volume)
                    raise AssertionError("Owned policy volume remains")
                except docker.errors.NotFound:
                    pass
        (out / "cleanup.json").write_text(
            json.dumps({"owned": owned, "all_absent": True}, indent=2)
        )
        client.close()


if __name__ == "__main__":
    main()
