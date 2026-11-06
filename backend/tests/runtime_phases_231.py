"""Owned product-Uvicorn crash/cancellation probes; not a production entry point.

performance_231.py --phase-suite --workloads owns the containers and cleanup.
Only test instrumentation/diagnostics are installed; actual API, SQLite, claims,
archive, crypto, Docker lifecycle and startup recovery remain product code.
"""

import asyncio
import builtins
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import threading
import time


ROOT = Path("/app/data/dv231-phases")
MARKER = ROOT / "marker.json"
RELEASE = ROOT / "release"


def marker(kind, **details):
    partial = MARKER.with_suffix(".partial")
    partial.write_text(
        json.dumps(
            {
                "kind": kind,
                "pid": os.getpid(),
                "thread": threading.current_thread().name,
                **details,
            }
        )
    )
    os.replace(partial, MARKER)


def server(phase):
    assert os.environ.get("DV231_OWNED") == "1"
    from app import backup_engine as module
    from app.main import app
    from app.database import async_session, WorkloadOperation
    from sqlalchemy import select
    import uvicorn

    engine = module.backup_engine
    module.settings.ALLOWED_HOOK_COMMANDS = sys.executable
    archive_calls = 0
    download_active = 0
    gated = False
    from app.api.backups import _ArchiveFileResponse

    send_archive = _ArchiveFileResponse.__call__

    async def observed_response(self, *args, **kwargs):
        nonlocal download_active
        download_active += 1
        try:
            return await send_archive(self, *args, **kwargs)
        finally:
            download_active -= 1

    _ArchiveFileResponse.__call__ = observed_response

    def blocking_gate(kind):
        nonlocal gated
        if not gated:
            gated = True
            marker(kind)
            while not RELEASE.exists():
                time.sleep(0.005)

    class Reader:
        def __init__(self, stream, kind):
            self.stream, self.kind = stream, kind

        def __getattr__(self, name):
            return getattr(self.stream, name)

        def __enter__(self):
            self.stream.__enter__()
            return self

        def __exit__(self, *args):
            return self.stream.__exit__(*args)

        def read(self, *args):
            blocking_gate(self.kind)
            return self.stream.read(*args)

        def readinto(self, *args):
            blocking_gate(self.kind)
            return self.stream.readinto(*args)

    if phase == "cancel-archive":
        # Exercise the real supported in-process fallback so the live source
        # reader can be held deterministically, not a fake completed archive.
        engine._sudo_available = lambda: False
        addfile = tarfile.TarFile.addfile

        def observed_addfile(self, info, fileobj=None):
            return addfile(
                self,
                info,
                Reader(fileobj, "source-reader") if fileobj is not None else None,
            )

        tarfile.TarFile.addfile = observed_addfile
    if phase == "cancel-hash":
        opened = builtins.open

        def observed_open(path, mode="r", *args, **kwargs):
            stream = opened(path, mode, *args, **kwargs)
            if mode == "rb" and str(path).startswith(module.settings.BACKUP_BASE_PATH):
                return Reader(stream, "checksum-reader")
            return stream

        builtins.open = observed_open
    if phase == "cancel-encrypt":
        create = asyncio.create_subprocess_exec
        communicate = asyncio.subprocess.Process.communicate
        codec_pids = set()

        async def observed_create(*args, **kwargs):
            child = await create(*args, **kwargs)
            if any(str(arg).endswith("/recovery.py") for arg in args):
                codec_pids.add(child.pid)
            return child

        async def observed_communicate(self, *args, **kwargs):
            nonlocal gated
            if self.pid in codec_pids and not gated:
                gated = True
                marker(
                    "codec-child",
                    child_pid=self.pid,
                    child_running=self.returncode is None,
                )
                while not RELEASE.exists():
                    await asyncio.sleep(0.005)
            return await communicate(self, *args, **kwargs)

        asyncio.create_subprocess_exec = observed_create
        asyncio.subprocess.Process.communicate = observed_communicate

    create_archive = engine._create_backup_archive

    async def observed_archive(*args, **kwargs):
        nonlocal archive_calls
        archive_calls += 1
        value = await create_archive(*args, **kwargs)
        if phase == "crash-archive":
            marker("settled-archive-before-restart")
            os._exit(81)
        return value

    engine._create_backup_archive = observed_archive
    checksum = engine._calculate_checksum

    async def observed_checksum(*args, **kwargs):
        value = await checksum(*args, **kwargs)
        if phase == "crash-hash":
            marker("settled-hash-after-snapshot-restart")
            os._exit(81)
        return value

    engine._calculate_checksum = observed_checksum
    decrypt = module.decrypt_backup

    async def observed_decrypt(*args, **kwargs):
        value = await decrypt(*args, **kwargs)
        if phase == "crash-restore":
            marker("authenticated-restore-before-stop", plaintext=str(value))
            os._exit(81)
        return value

    module.decrypt_backup = observed_decrypt

    @app.post("/api/v1/dv231/cancel/{backup_id}")
    async def cancel(backup_id: int):
        task = engine.active_backups.get(backup_id)
        if task is not None:
            task.cancel()
        return {"found": task is not None}

    @app.get("/api/v1/dv231/state")
    async def state():
        async with async_session() as session:
            operations = list(await session.scalars(select(WorkloadOperation)))
        return {
            "active": {
                str(identifier): task.done()
                for identifier, task in engine.active_backups.items()
            },
            "archive_calls": archive_calls,
            "download_active": download_active,
            "locked_claims": sum(
                lock.locked() for lock in engine._workload_locks.values()
            ),
            "permits": engine._backup_semaphore._value,
            "operations": [
                {"state": item.state, "resources": item.resources}
                for item in operations
            ],
        }

    uvicorn.run(app, host="127.0.0.1", port=8000, log_level="warning")


def controller(output):
    import docker
    import requests

    assert os.environ.get("DV231_OWNED") == "1"
    ids = json.loads(os.environ["DV231_WORKLOADS"])
    client = docker.from_env()
    ROOT.mkdir(exist_ok=True)
    base = "http://127.0.0.1"
    http = requests.Session()
    process = None
    logs = []

    def owned():
        for identifier in ids.values():
            item = client.containers.get(identifier)
            assert (
                item.id == identifier
                and item.labels.get("dockervault.fixture") == "dv231"
            )

    def states():
        owned()
        return {
            name: {
                "id": identifier,
                "running": client.containers.get(identifier).attrs["State"]["Running"],
                "started": client.containers.get(identifier).attrs["State"][
                    "StartedAt"
                ],
            }
            for name, identifier in ids.items()
        }

    def ready():
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            try:
                if all(
                    requests.get(f"http://127.0.0.1:{port}", timeout=0.2).content
                    == b"dv231-ready"
                    for port in (9123, 9124)
                ):
                    assert not states()["initially-stopped"]["running"]
                    return
            except requests.RequestException:
                pass
            time.sleep(0.025)
        raise AssertionError("Owned dependency/app not HTTP-ready after recovery")

    def call(method, path, **kwargs):
        response = http.request(method, base + path, timeout=60, **kwargs)
        assert response.status_code == 200, (
            path,
            response.status_code,
            response.text[:400],
        )
        return response.json()

    def launch(phase):
        nonlocal process
        MARKER.unlink(missing_ok=True)
        RELEASE.unlink(missing_ok=True)
        log = (ROOT / f"server-{phase or 'recover'}-{len(logs)}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(
            [sys.executable, __file__, "--server", phase],
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            assert process.poll() is None, (
                "Owned Uvicorn failed; inspect phase server logs"
            )
            try:
                if requests.get(base + "/health", timeout=0.2).status_code == 200:
                    return
            except requests.RequestException:
                pass
            time.sleep(0.025)
        raise AssertionError("Fresh product Uvicorn failed startup")

    def stop_server():
        if process is not None and process.poll() is None:
            process.terminate()
            process.wait(timeout=30)

    def backup(target):
        owned()
        return call("POST", "/api/v1/backups", json={"target_id": target["id"]})

    def target(name, hook=None, post=None):
        return call(
            "POST",
            "/api/v1/targets",
            json={
                "name": "dv231-" + name,
                "target_type": "path",
                "host_path": str(source),
                "container_name": client.containers.get(ids["app"]).name,
                "dependencies": [
                    client.containers.get(ids[kind]).name
                    for kind in ("dependency", "initially-stopped")
                ],
                "stop_container": True,
                "compression_enabled": False,
                "pre_backup_command": hook,
                "post_backup_command": post,
            },
        )

    def wait_marker():
        deadline = time.monotonic() + 40
        while not MARKER.exists():
            assert time.monotonic() < deadline and process.poll() is None
            time.sleep(0.005)
        return json.loads(MARKER.read_text())

    def terminal(identifier, expected):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            row = call("GET", f"/api/v1/backups/{identifier}")
            if row["status"] in expected:
                return row
            assert row["status"] not in ("failed", "cancelled", "completed"), row
            time.sleep(0.025)
        raise AssertionError("Owned backup did not settle")

    source = ROOT / "source"
    source.mkdir(exist_ok=True)
    with (source / "payload").open("wb") as stream:
        block = os.urandom(1024 * 1024)
        for _ in range(64):
            stream.write(block)
    # Existing tar data-filter semantics remove group/other write bits. Use the
    # same explicit safe mode and exact timestamp as the performance matrix.
    os.chmod(source / "payload", 0o640)
    os.utime(source / "payload", (1700000000, 1700000000))
    rows = []
    try:
        launch("")
        password = "dv231-owned-phase-password"
        call(
            "POST",
            "/api/v1/auth/setup",
            json={
                "username": "dv231",
                "password": password,
                "confirm_password": password,
            },
        )
        call(
            "POST",
            "/api/v1/auth/login",
            json={"username": "dv231", "password": password},
        )
        http.headers["Authorization"] = "Bearer " + http.cookies.get("session_token")
        keys = call("POST", "/api/v1/encryption/setup")
        call("POST", "/api/v1/encryption/confirm-setup", json={"confirmed": True})
        seed = backup(target("seed"))
        terminal(seed["id"], {"completed"})
        ready()
        stop_server()
        for phase in ("cancel-archive", "cancel-hash", "cancel-encrypt", "cancel-hook"):
            launch(phase)
            hook = (
                shlex.join([sys.executable, __file__, "--hook", "wait"])
                if phase == "cancel-hook"
                else None
            )
            first = backup(target(phase, hook=hook))
            evidence = wait_marker()
            second = None
            if phase == "cancel-archive":
                second = backup(target("overlapping-source"))
                time.sleep(0.05)
                assert call("GET", "/api/v1/dv231/state")["archive_calls"] == 1
            call("POST", f"/api/v1/dv231/cancel/{first['id']}")
            time.sleep(0.05)
            during = call("GET", "/api/v1/dv231/state")
            if phase in ("cancel-archive", "cancel-hash"):
                assert (
                    str(first["id"]) in during["active"]
                    and not during["active"][str(first["id"])]
                ), during
                assert during["locked_claims"] and during["permits"] < 2
                if phase == "cancel-archive":
                    assert during["archive_calls"] == 1
                    assert not states()["app"]["running"]
                call("POST", f"/api/v1/dv231/cancel/{first['id']}")
                RELEASE.write_text("release owned reader")
            terminal(first["id"], {"failed", "cancelled"})
            if second:
                terminal(second["id"], {"completed"})
            ready()
            final = call("GET", "/api/v1/dv231/state")
            assert final["locked_claims"] == 0 and final["permits"] == 2
            if phase == "cancel-hook":
                assert not Path(f"/proc/{evidence['pid']}").exists()
                assert not (ROOT / "hook-completed").exists()
            if phase == "cancel-encrypt":
                assert (
                    evidence["child_running"]
                    and not Path(f"/proc/{evidence['child_pid']}").exists()
                )
            rows.append(
                {
                    "phase": phase,
                    "marker": evidence,
                    "during": during,
                    "after": final,
                    "workloads": states(),
                    "ready": True,
                }
            )
            Path(output).write_text(json.dumps(rows, indent=2))
            stop_server()
        for phase in ("crash-archive", "crash-hash", "crash-restore"):
            launch(phase)
            before = states()
            if phase == "crash-restore":
                cipher = Path(call("GET", f"/api/v1/backups/{seed['id']}")["file_path"])
                legitimate_plain = cipher.with_suffix("")
                assert not legitimate_plain.exists()
                legitimate_plain.write_bytes(b"owned legitimate archive, not staging")
                staging_before = set(
                    Path("/backups").rglob(".dockervault-restore-*.tmp")
                )
                owned()
                try:
                    call(
                        "POST",
                        f"/api/v1/backups/{seed['id']}/restore",
                        json={"private_key": keys["private_key"]},
                    )
                except (requests.RequestException, AssertionError):
                    pass
                identifier = seed["id"]
            else:
                identifier = backup(target(phase))["id"]
            evidence = wait_marker()
            assert process.wait(timeout=20) == 81
            at_crash = states()
            if phase == "crash-restore":
                staged_plaintext = Path(evidence["plaintext"])
                assert staged_plaintext.exists()
            launch("")
            ready()
            if phase == "crash-restore":
                assert not staged_plaintext.exists()
                assert (
                    set(Path("/backups").rglob(".dockervault-restore-*.tmp"))
                    == staging_before
                )
                assert (
                    legitimate_plain.read_bytes()
                    == b"owned legitimate archive, not staging"
                )
                assert cipher.exists() and cipher.with_suffix(".key").exists()
            after = states()
            status = call("GET", f"/api/v1/backups/{identifier}")["status"]
            assert status == ("completed" if phase == "crash-restore" else "failed")
            if phase in ("crash-hash", "crash-restore"):
                assert all(
                    after[kind]["started"] == at_crash[kind]["started"] for kind in ids
                )
            rows.append(
                {
                    "phase": phase,
                    "marker": evidence,
                    "before": before,
                    "at_crash": at_crash,
                    "after": after,
                    "backup_status": status,
                    "ready": True,
                    "real_uvicorn_exit": 81,
                    "restore_staging_removed": phase == "crash-restore"
                    and not staged_plaintext.exists(),
                }
            )
            Path(output).write_text(json.dumps(rows, indent=2))
            stop_server()
        launch("")
        failure = backup(
            target(
                "posthook-failure",
                post=shlex.join([sys.executable, __file__, "--hook", "fail"]),
            )
        )
        row = terminal(failure["id"], {"failed"})
        assert "Hook" in row["error_message"]
        ready()
        rows.append(
            {
                "phase": "posthook-failure",
                "backup_status": row["status"],
                "ready": True,
                "workloads": states(),
            }
        )
        import tempfile

        browse_url = base + f"/api/v1/backups/{seed['id']}/files"
        cached_listing = http.post(
            browse_url, json={"private_key": keys["private_key"]}, timeout=30
        )
        assert cached_listing.status_code == 200
        wrong_key = http.post(
            browse_url, json={"private_key": "dv231-invalid-key"}, timeout=30
        )
        assert wrong_key.status_code == 400
        unauthenticated = requests.post(
            browse_url, json={"private_key": keys["private_key"]}, timeout=30
        )
        assert unauthenticated.status_code == 401
        temporary = Path(tempfile.gettempdir())
        before_files = {str(path) for path in temporary.glob("*.tar")}
        before_stream = states()
        response = http.post(
            base + f"/api/v1/backups/{seed['id']}/files/source/payload",
            json={"private_key": keys["private_key"]},
            stream=True,
            timeout=30,
        )
        assert response.status_code == 200
        assert next(response.iter_content(65536))
        response.close()
        deadline = time.monotonic() + 20
        while call("GET", "/api/v1/dv231/state")["download_active"]:
            assert time.monotonic() < deadline
            time.sleep(0.025)
        assert {str(path) for path in temporary.glob("*.tar")} == before_files
        assert states() == before_stream
        rows.append(
            {
                "phase": "cancel-http-stream",
                "first_chunk_received": True,
                "decrypted_temp_removed": True,
                "workloads_unchanged": True,
                "cached_listing_authenticated": True,
                "wrong_key_status": wrong_key.status_code,
                "unauthenticated_status": unauthenticated.status_code,
            }
        )
        # Build the explicitly supported historical CBC/raw-DEK sidecar fixture
        # from a real app backup, using the SAME age key created above.
        from app.encryption import decrypt_backup
        import hashlib

        archive = Path(call("GET", f"/api/v1/backups/{seed['id']}")["file_path"])
        sidecar = archive.with_suffix(".key")
        plaintext = asyncio.run(
            decrypt_backup(
                archive, sidecar, keys["private_key"], ROOT / "legacy-input.tmp"
            )
        )
        password_file = ROOT / "legacy-password"
        legacy = archive.with_suffix(".legacy-fixture")
        dek = os.urandom(32)
        descriptor = os.open(password_file, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        try:
            with os.fdopen(descriptor, "w") as stream:
                stream.write(dek.hex())
            subprocess.run(
                [
                    "openssl",
                    "enc",
                    "-aes-256-cbc",
                    "-pbkdf2",
                    "-iter",
                    "100000",
                    "-in",
                    str(plaintext),
                    "-out",
                    str(legacy),
                    "-pass",
                    "file:" + str(password_file),
                ],
                check=True,
                capture_output=True,
            )
            wrapped = subprocess.run(
                ["age", "-a", "-r", keys["public_key"]],
                input=dek,
                check=True,
                capture_output=True,
            ).stdout
            os.replace(legacy, archive)
            sidecar.write_bytes(wrapped)
        finally:
            password_file.unlink(missing_ok=True)
            plaintext.unlink(missing_ok=True)

        async def update_legacy_size():
            from app.database import async_session, Backup, engine

            async with async_session() as session:
                record = await session.get(Backup, seed["id"])
                record.file_size = archive.stat().st_size
                await session.commit()
            await engine.dispose()

        asyncio.run(update_legacy_size())
        payload = source / "payload"
        expected_mode, expected_mtime = payload.stat().st_mode, payload.stat().st_mtime
        with payload.open("rb") as stream:
            expected_digest = hashlib.file_digest(stream, "sha256").digest()
        payload.write_bytes(
            b"owned source deliberately changed before legacy HTTP restore"
        )
        owned()
        assert (
            call(
                "POST",
                f"/api/v1/backups/{seed['id']}/restore",
                json={"private_key": keys["private_key"]},
            )["status"]
            == "restored"
        )
        with payload.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").digest() == expected_digest
        assert (
            payload.stat().st_mode == expected_mode
            and payload.stat().st_mtime == expected_mtime
        )
        ready()
        rows.append(
            {
                "phase": "legacy-cbc-http-restore",
                "same_age_key": True,
                "byte_mode_mtime_verified": True,
                "ready": True,
                "fixture": "real app backup converted to supported legacy CBC/raw-DEK sidecar",
            }
        )
        Path(output).write_text(json.dumps(rows, indent=2))
    finally:
        RELEASE.write_text("finally release owned readers")
        stop_server()
        for log in logs:
            log.close()
        client.close()


if __name__ == "__main__":
    if sys.argv[1] == "--server":
        server(sys.argv[2])
    elif sys.argv[1] == "--controller":
        controller(sys.argv[2])
    elif sys.argv[1] == "--hook":
        if sys.argv[2] == "fail":
            sys.exit(7)
        marker("hook-child")
        while not RELEASE.exists():
            time.sleep(0.005)
        (ROOT / "hook-completed").write_text("completed")
