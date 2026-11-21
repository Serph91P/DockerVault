"""#190 native SQLite/auth/age/key files with named local transport doubles.

No production socket, mounts, data or remote provider. SSH transport executes
the generic remote script locally
under an owned HOME, not against an external SSH installation.
"""

import asyncio
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tarfile
import threading

root = Path("/app/data/settings190")
root.mkdir(parents=True, exist_ok=True)
(root / "source").mkdir(exist_ok=True)
(root / "source/payload.txt").write_bytes(b"owned settings age archive bytes\x00\xff")
os.environ.update(
    DATABASE_URL=f"sqlite+aiosqlite:///{root / 'backup.db'}",
    BACKUP_BASE_PATH="/backups",
    COOKIE_SECURE="false",
    DOCKER_SOCKET=str(root / "absent.sock"),
    ALLOWED_SSH_KEY_DIRS=str(root / "ssh_keys"),
    LOG_LEVEL="WARNING",
)
observations = {"install_methods": []}
from app import credential_encryption
from app.api import storage

credential_encryption._KEY_FILE_PATH = root / ".credential_key"
storage._SSH_KEYS_DIR = root / "ssh_keys"
real_run = storage._run


async def synthetic_ssh(cmd, env=None, input_bytes=None):
    if cmd[0] != "sshpass":
        return await real_run(cmd, env=env, input_bytes=input_bytes)
    assert env is not None and input_bytes is not None
    assert env.get("SSHPASS") and input_bytes.startswith(b"ssh-ed25519 ")
    home = root / "synthetic_remote"
    home.mkdir(exist_ok=True)
    if cmd[-1] == "install-ssh-key":
        (home / "hetzner.pub").write_bytes(input_bytes)
        observations["install_methods"].append("hetzner")
        return 0, "Synthetic Hetzner transport wrote public key", ""
    assert cmd[-3:-1] == ["sh", "-c"]
    proc = await asyncio.create_subprocess_exec(
        "sh",
        "-c",
        cmd[-1],
        env={**os.environ, "HOME": str(home)},
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await proc.communicate(input_bytes)
    observations["install_methods"].append("authorized_keys")
    return proc.returncode, stdout.decode(), stderr.decode()


storage._run = synthetic_ssh
from app.main import app
import uvicorn


@app.get("/api/v1/test-settings190/effects")
async def effects():
    with sqlite3.connect(root / "backup.db") as db:
        settings = dict(
            db.execute(
                "select key,value from app_settings where key = 'storage_check_quota_before_upload'"
            )
        )
        foreign_keys = db.execute("pragma foreign_key_check").fetchall()
    key_files = (
        list((root / "ssh_keys").glob("*")) if (root / "ssh_keys").exists() else []
    )
    public = root / "ssh_keys/settings_key.pub"
    expected = public.read_bytes().strip() if public.exists() else None
    installed = [
        root / "synthetic_remote/.ssh/authorized_keys",
        root / "synthetic_remote/hetzner.pub",
    ]
    return {
        "settings": settings,
        "foreign_key_errors": foreign_keys,
        "transport": observations,
        "ssh_files": [p.name for p in key_files],
        "installed_public_bytes_equal": expected is not None
        and all(p.exists() and p.read_bytes().strip() == expected for p in installed),
        "ssh_private_permissions_safe": all(
            p.stat().st_mode & 0o077 == 0
            for p in key_files
            if not p.name.endswith(".pub")
        ),
    }


@app.post("/api/v1/test-settings190/verify-age/{backup_id}")
async def verify_age(backup_id: int):
    with sqlite3.connect(root / "backup.db") as db:
        archive_name, encrypted = db.execute(
            "select file_path,encrypted from backups where id=?", (backup_id,)
        ).fetchone()
    assert encrypted
    archive = Path(archive_name)
    identity, output = root / "exported.key", root / "offline.tar.gz"
    process = subprocess.run(
        [
            "python",
            "/app/app/recovery.py",
            "--identity",
            str(identity),
            "--key",
            str(archive.with_suffix(".key")),
            "--archive",
            str(archive),
            "--output",
            str(output),
        ],
        capture_output=True,
    )
    assert process.returncode == 0, "Real age decrypt failed (private output withheld)"
    with tarfile.open(output) as tar:
        members = [
            m for m in tar.getmembers() if m.isfile() and m.name.endswith("payload.txt")
        ]
        assert (
            len(members) == 1
            and tar.extractfile(members[0]).read()
            == (root / "source/payload.txt").read_bytes()
        )
    output.unlink()
    return {
        "encrypted_archive": True,
        "exported_identity_decrypts": True,
        "payload_bytes_equal": True,
    }


@app.post("/api/v1/test-settings190/restart")
async def restart():
    def execute():
        os.environ["SETTINGS190_RESTARTED"] = str(
            int(os.environ.get("SETTINGS190_RESTARTED", "0")) + 1
        )
        os.execv(sys.executable, [sys.executable, __file__])

    threading.Timer(0.3, execute).start()
    return {"accepted": True, "not_completed": True}


@app.get("/api/v1/test-settings190/restarted")
async def restarted():
    return {"restarted": int(os.environ.get("SETTINGS190_RESTARTED", "0"))}


if "SETTINGS190_RESTARTED" not in os.environ:
    subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
