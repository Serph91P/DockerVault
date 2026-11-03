"""Real HTTP app backup/restore in a disposable product image, never a seeded catalog."""

import hashlib
import json
import os
from pathlib import Path
import signal
import time

import requests

assert os.environ.get("DOCKERVAULT_ROUNDTRIP_OWNED") == "1"
BASE = "http://127.0.0.1"
client = requests.Session()


def call(method, path, **kwargs):
    response = client.request(method, BASE + path, timeout=45, **kwargs)
    assert response.status_code == 200, (
        method,
        path,
        response.status_code,
        response.text[:400],
    )
    return response


def backend_pid():
    matches = []
    for path in Path("/proc").glob("[0-9]*/cmdline"):
        try:
            args = path.read_bytes().split(b"\0")
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        if b"app.main:app" in args and any(b"uvicorn" in part for part in args):
            matches.append(int(path.parent.name))
    assert len(matches) == 1
    return matches[0]


def wait_ready(previous_pid=None):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        try:
            pid = backend_pid()
            if (
                pid != previous_pid
                and requests.get(BASE + "/health", timeout=2).json()["status"]
                == "healthy"
            ):
                return pid
        except (AssertionError, requests.RequestException, ValueError):
            pass
        time.sleep(0.1)
    raise AssertionError("Product image did not become healthy")


def login():
    response = call(
        "POST",
        "/api/v1/auth/login",
        json={"username": "owned-roundtrip", "password": PASSWORD},
    )
    token = client.cookies.get("session_token")
    assert (
        token
        and next(
            cookie for cookie in client.cookies if cookie.name == "session_token"
        ).secure
    )
    client.headers["Authorization"] = "Bearer " + token
    assert response.json()["user"]["username"] == "owned-roundtrip"


wait_ready()
assert client.get(BASE + "/api/v1/backups", timeout=5).status_code == 403
PASSWORD = "Owned-roundtrip-test-password"
call(
    "POST",
    "/api/v1/auth/setup",
    json={
        "username": "owned-roundtrip",
        "password": PASSWORD,
        "confirm_password": PASSWORD,
    },
)
assert requests.get(BASE + "/api/v1/backups", timeout=5).status_code == 401
login()
assert (
    call("GET", "/recover_backup.py").content
    == Path("/app/app/recovery.py").read_bytes()
)
remote = Path("/app/data/owned-roundtrip-remote")
storage = call(
    "POST",
    "/api/v1/storage",
    json={
        "name": "owned-roundtrip-remote",
        "storage_type": "local",
        "base_path": str(remote),
    },
).json()
results = []
for encrypted in (False, True):
    private_key = None
    if encrypted:
        keys = call("POST", "/api/v1/encryption/setup").json()
        private_key = keys["private_key"]
        call("POST", "/api/v1/encryption/confirm-setup", json={"confirmed": True})
    source = Path("/app/data") / ("owned-encrypted" if encrypted else "owned-plain")
    (source / "nested").mkdir(parents=True)
    expected = {"payload": os.urandom(8192), "nested/value": b"owned nested bytes\n"}
    for name, payload in expected.items():
        (source / name).write_bytes(payload)
    (source / "nested-link").symlink_to("nested", target_is_directory=True)
    for path in (source, *source.rglob("*")):
        if not path.is_symlink():
            os.chown(path, 1000, 1000)
    target = call(
        "POST",
        "/api/v1/targets",
        json={
            "name": source.name,
            "target_type": "path",
            "host_path": str(source),
            "compression_enabled": not encrypted,
            "stop_container": False,
        },
    ).json()
    backup = call(
        "POST",
        "/api/v1/backups",
        json={"target_id": target["id"], "backup_type": "full"},
    ).json()
    deadline = time.monotonic() + 90
    while backup["status"] != "completed":
        assert backup["status"] not in ("failed", "cancelled"), backup.get(
            "error_message"
        )
        assert time.monotonic() < deadline, "Real app backup did not complete"
        time.sleep(0.1)
        backup = call("GET", f"/api/v1/backups/{backup['id']}").json()
    assert backup["encrypted"] is encrypted
    archive = Path(backup["file_path"])
    assert archive.is_file() and archive.stat().st_size > 0
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if not encrypted:
        assert backup["checksum"] == digest
    else:
        # Catalog checksum describes the source tar; encrypted envelope has new bytes.
        assert len(backup["checksum"]) == 64
    # First prove the actual local restore path, not a download helper.
    for name in expected:
        (source / name).write_bytes(b"damaged local restore source")
    (source / "nested-link").unlink()
    options = {"private_key": private_key} if encrypted else {}
    assert (
        call("POST", f"/api/v1/backups/{backup['id']}/restore", json=options).json()[
            "status"
        ]
        == "restored"
    )
    for name, payload in expected.items():
        assert (source / name).read_bytes() == payload
    assert (source / "nested-link").is_symlink()
    synced = call("POST", f"/api/v1/storage/{storage['id']}/sync/{backup['id']}").json()
    remote_archive = remote / synced["remote_path"]
    assert hashlib.sha256(remote_archive.read_bytes()).hexdigest() == digest
    call(
        "DELETE",
        f"/api/v1/backups/{backup['id']}",
        params={"delete_local": "true", "delete_remote": "false"},
    )
    assert not archive.exists()
    if encrypted:
        assert archive.with_suffix(
            ".key"
        ).is_file()  # Local-only delete intentionally retains recovery keys.
        assert remote_archive.with_suffix(".key").is_file()
        archive.with_suffix(
            ".key"
        ).unlink()  # Owned fixture: force sidecar recovery from remote too.
    detail = call("GET", f"/api/v1/backups/{backup['id']}").json()
    assert detail["file_path"] is None and detail["remote_synced"]
    for name in expected:
        (source / name).write_bytes(b"damaged remote restore source")
    (source / "nested-link").unlink()
    old_pid = backend_pid()
    os.kill(old_pid, signal.SIGTERM)
    assert wait_ready(old_pid) != old_pid
    # Fresh authentication and provider registry after real supervisor restart.
    client.headers.pop("Authorization", None)
    client.cookies.clear()
    login()
    detail = call("GET", f"/api/v1/backups/{backup['id']}").json()
    assert detail["file_path"] is None and detail["remote_synced"]
    before = {str(path) for path in Path("/tmp").glob("dockervault*")}
    restored = call(
        "POST", f"/api/v1/backups/{backup['id']}/restore", json=options
    ).json()
    assert restored["status"] == "restored"
    for name, payload in expected.items():
        assert (source / name).read_bytes() == payload
    assert (source / "nested-link").is_symlink() and os.readlink(
        source / "nested-link"
    ) == "nested"
    assert not (source / source.name).exists()
    assert not archive.exists()
    assert {str(path) for path in Path("/tmp").glob("dockervault*")} == before
    assert call("GET", f"/api/v1/backups/{backup['id']}").json()["file_path"] is None
    assert hashlib.sha256(remote_archive.read_bytes()).hexdigest() == digest
    results.append(
        {
            "encrypted": encrypted,
            "app_backup": "completed",
            "local_restore": "byte-identical",
            "remote_only_restart_restore": "byte-identical",
            "symlink_layout_cleanup": True,
        }
    )
print(json.dumps({"real_http_roundtrips": results}))
