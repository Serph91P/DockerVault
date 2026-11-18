"""#190 real standard-image acceptance. IMAGE BROWSER AXE OUT; no source overlay."""

from pathlib import Path
import hashlib
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
    source = Path(__file__).parent
    client = docker.from_env()
    owned = []
    backend = runner = None
    name = "dv190-" + uuid.uuid4().hex[:12]

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
        (out / "binding.json").write_text(
            json.dumps(
                {
                    "product": client.images.get(product).id,
                    "browser": client.images.get(browser).id,
                    "source_overlay_diagnostic": False,
                    "harness_sha256": {
                        p: hashlib.sha256((source / p).read_bytes()).hexdigest()
                        for p in [
                            "run_settings_browser.py",
                            "runtime_settings_app.py",
                            "browser_settings.mjs",
                        ]
                    },
                },
                indent=2,
            )
        )
        backend = client.containers.create(
            product,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_settings_app.py"],
            environment={"PYTHONPATH": "/app"},
            labels={"dockervault.test": "settings190"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_settings_app.py": (
                        source / "runtime_settings_app.py"
                    ).read_text()
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
            raise AssertionError("Settings fixture unhealthy")
        runner = client.containers.create(
            browser,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            volumes_from=[backend.id],
            entrypoint="sh",
            command=["-c", "sleep 900"],
            labels={"dockervault.test": "settings190"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_settings.mjs": (
                        source / "browser_settings.mjs"
                    ).read_text(),
                    "axe.min.js": Path(axe).read_text(),
                }
            ),
        )
        runner.start()
        result = runner.exec_run(["node", "/runner/browser_settings.mjs"])
        (out / "browser.log").write_bytes(result.output)
        (out / "browser-exit.json").write_text(
            json.dumps({"exit_code": result.exit_code})
        )
        print(result.output.decode(), flush=True)
        runner.exec_run(
            [
                "sh",
                "-c",
                "cd /runner && tar cf evidence.tar settings-*.png axe-settings-*.json settings190-result.json 2>/dev/null; true",
            ]
        )
        data, _ = runner.get_archive("/runner/evidence.tar")
        with tarfile.open(fileobj=io.BytesIO(b"".join(data))) as outer:
            payload = outer.extractfile("evidence.tar").read()
        with tarfile.open(fileobj=io.BytesIO(payload)) as evidence:
            for member in evidence.getmembers():
                if member.isfile():
                    (out / Path(member.name).name).write_bytes(
                        evidence.extractfile(member).read()
                    )
        assert result.exit_code == 0, "Settings browser acceptance failed"
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
        # Anonymous volumes may be shared by the two owned containers.
        for volume in {v for item in owned for v in item["volumes"]}:
            try:
                client.volumes.get(volume).remove()
            except docker.errors.NotFound:
                pass
        for kind, identities in [
            ("containers", [item["id"] for item in owned]),
            ("volumes", {v for item in owned for v in item["volumes"]}),
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
