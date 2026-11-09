"""Run #143 against standard product bytes, real browser/API/SQLite/age/rclone.

IMAGE BROWSER_IMAGE AXE_JS OUT. No host binds, production socket or source overlay.
"""

import hashlib
import io
import json
from pathlib import Path
import secrets
import sys
import tarfile
import time
import uuid

import docker
from run_storage_jobs_browser import archive_files


def main():
    image, browser_image, axe, destination = sys.argv[1:5]
    out = Path(destination)
    out.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).parent
    client = docker.from_env()
    owned = []
    backend = runner = None
    seed = secrets.token_hex(32)
    name = "dv143-" + uuid.uuid4().hex[:12]

    def record(container):
        container.reload()
        assert not container.attrs["HostConfig"]["Binds"]
        owned.append(
            {
                "id": container.id,
                "volumes": [
                    mount["Name"]
                    for mount in container.attrs["Mounts"]
                    if mount["Type"] == "volume"
                ],
            }
        )
        (out / "owned.json").write_text(json.dumps(owned, indent=2))

    def healthy():
        for _ in range(90):
            if (
                backend.exec_run(["curl", "-fsS", "http://127.0.0.1/health"]).exit_code
                == 0
            ):
                return
            time.sleep(0.5)
        raise AssertionError("isolated product did not become healthy")

    def state(mode):
        result = backend.exec_run(
            ["python", "/app/data/runtime_system_recovery_app.py", "--inspect", mode]
        )
        assert result.exit_code == 0, "Out-of-band native state inspection failed"
        return json.loads(result.output.decode().strip().splitlines()[-1])

    def phase(which):
        result = runner.exec_run(
            ["node", "/runner/browser_system_recovery.mjs"],
            environment={"DV143_PHASE": which, "DV143_SEED": seed},
        )
        (out / (which + "-browser.log")).write_bytes(result.output)
        print(result.output.decode(), flush=True)
        assert result.exit_code == 0, "Native #143 browser phase failed: " + which

    def gather():
        runner.exec_run(
            [
                "sh",
                "-c",
                "cd /runner && tar cf /runner/system143-evidence.tar system143-*.png system143-*-axe.json system143-*-result.json 2>/dev/null; true",
            ]
        )
        stream, _ = runner.get_archive("/runner/system143-evidence.tar")
        with tarfile.open(fileobj=io.BytesIO(b"".join(stream))) as archive:
            body = archive.extractfile("system143-evidence.tar").read()
        with tarfile.open(fileobj=io.BytesIO(body)) as archive:
            for member in archive.getmembers():
                if member.isfile():
                    (out / Path(member.name).name).write_bytes(
                        archive.extractfile(member).read()
                    )

    try:
        (out / "binding.json").write_text(
            json.dumps(
                {
                    "product": client.images.get(image).id,
                    "browser": client.images.get(browser_image).id,
                    "source_overlay_diagnostic": False,
                    "harness_sha256": {
                        name: hashlib.sha256((source / name).read_bytes()).hexdigest()
                        for name in [
                            "run_system_recovery_browser.py",
                            "runtime_system_recovery_app.py",
                            "browser_system_recovery.mjs",
                        ]
                    },
                },
                indent=2,
            )
        )
        backend = client.containers.create(
            image,
            name=name,
            auto_remove=True,
            network_mode="none",
            entrypoint=["python", "/app/data/runtime_system_recovery_app.py"],
            environment={"PYTHONPATH": "/app", "DV143_SEED": seed},
            labels={"dockervault.test": "system-recovery143"},
        )
        record(backend)
        backend.put_archive(
            "/app/data",
            archive_files(
                {
                    "runtime_system_recovery_app.py": (
                        source / "runtime_system_recovery_app.py"
                    ).read_text()
                }
            ),
        )
        backend.start()
        healthy()
        runner = client.containers.create(
            browser_image,
            name=name + "-browser",
            auto_remove=True,
            network_mode="container:" + backend.id,
            volumes_from=[backend.id],
            entrypoint="sh",
            command=["-c", "sleep 1800"],
            labels={"dockervault.test": "system-recovery143"},
        )
        record(runner)
        runner.put_archive(
            "/runner",
            archive_files(
                {
                    "browser_system_recovery.mjs": (
                        source / "browser_system_recovery.mjs"
                    ).read_text(),
                    "axe.min.js": Path(axe).read_text(),
                }
            ),
        )
        runner.start()
        phase("source")
        # A process replacement starts a different database, instance key and data root.
        for _ in range(90):
            probe = backend.exec_run(
                ["curl", "-fsS", "http://127.0.0.1/api/v1/auth/status"]
            )
            if (
                probe.exit_code == 0
                and json.loads(probe.output).get("setup_complete") is False
            ):
                break
            time.sleep(0.5)
        else:
            raise AssertionError("NEW installation did not require setup")
        before = state("destination")
        assert before["users"] == before["sessions"] == 0 and not before["key_exists"]
        phase("failure")
        after = state("destination")
        assert after == before, (
            "Wrong password / actual SQL activation failure changed original rows, archives, key or startup state"
        )
        (out / "failure-no-live-effect.json").write_text(
            json.dumps({"before": before, "after": after, "unchanged": True}, indent=2)
        )
        phase("restored")
        after = state("destination")
        assert after["users"] == after["sessions"] == 1
        assert after["archives"] == before["archives"] and not after["tripwire"]
        assert (
            after["age_public_preserved"]
            and after["credentials_match"]
            and after["fresh_local_key"]
            and after["private_ssh_not_restored"]
            and after["rclone_reexported"]
        )
        assert after["staging_keys"] == 0
        proof = backend.exec_run(
            [
                "python",
                "-c",
                "from pathlib import Path; expected=b'DockerVault143 retained archive bytes\\x00\\xff\\n'; files=list(Path('/backups/system143-restored').rglob('payload.txt')); assert len(files)==1 and files[0].read_bytes()==expected; print('PASS retained remote-only encrypted archive restored with exact original bytes')",
            ]
        )
        (out / "archive-byte-proof.log").write_bytes(proof.output)
        assert proof.exit_code == 0
        (out / "canonical-result.json").write_text(json.dumps(after, indent=2))
        gather()
    finally:
        if runner:
            try:
                gather()
            except (docker.errors.NotFound, docker.errors.APIError):
                pass
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
        for name in {name for item in owned for name in item["volumes"]}:
            try:
                client.volumes.get(name).remove()
            except docker.errors.NotFound:
                pass
        for kind, identifiers in [
            ("containers", [item["id"] for item in owned]),
            ("volumes", {name for item in owned for name in item["volumes"]}),
        ]:
            for identifier in identifiers:
                try:
                    getattr(client, kind).get(identifier)
                except docker.errors.NotFound:
                    continue
                raise AssertionError("Owned resource remains: " + identifier)
        (out / "cleanup.json").write_text(
            json.dumps({"all_absent": True, "owned": owned}, indent=2)
        )
        client.close()


if __name__ == "__main__":
    main()
