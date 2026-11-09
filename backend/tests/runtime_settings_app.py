"""#190 native SQLite/auth/age/key files with named local transport doubles.

No production socket, mounts, data or remote provider. Komodo is a synthetic
loopback HTTP server; SSH transport executes the generic remote script locally
under an owned HOME, not against an external SSH installation.
"""

import asyncio
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
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
    KOMODO_ENABLED="true",
    KOMODO_API_URL="http://127.0.0.1:9100/env",
    KOMODO_ALLOWED_HOSTS="127.0.0.1",
    LOG_LEVEL="WARNING",
)
os.environ.setdefault("KOMODO_API_KEY", secrets.token_hex(24))
os.environ.setdefault("KOMODO_API_SECRET", secrets.token_hex(24))
observations = {
    "requests": 0,
    "path": "",
    "headers_present": False,
    "install_methods": [],
}
fake_mode = "good"


class SyntheticKomodo(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        observations.update(
            requests=observations["requests"] + 1,
            path=self.path,
            headers_present=bool(
                self.headers.get("x-api-key") and self.headers.get("x-api-secret")
            ),
        )
        content = json.dumps(
            {"version": "synthetic-190"} if body.get("type") == "GetVersion" else []
        ).encode()
        self.send_response(200 if fake_mode == "good" else 401)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)


server = ThreadingHTTPServer(("127.0.0.1", 9100), SyntheticKomodo)
threading.Thread(target=server.serve_forever, daemon=True).start()
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
from app.komodo import komodo_client
from app.komodo_config import load_komodo_config
import uvicorn


@app.post("/api/v1/test-settings190/komodo/{mode}")
async def set_mode(mode: str):
    global fake_mode
    assert mode in ("good", "failed")
    fake_mode = mode
    return {"synthetic_mode": mode}


@app.get("/api/v1/test-settings190/effects")
async def effects():
    config = await load_komodo_config()
    with sqlite3.connect(root / "backup.db") as db:
        settings = dict(
            db.execute(
                "select key,value from app_settings where key in ('storage_check_quota_before_upload','komodo_enabled','komodo_api_url')"
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
        "client_matches_effective": komodo_client.enabled == config.enabled
        and komodo_client.api_url == config.api_url
        and komodo_client.api_key == config.api_key
        and komodo_client.api_secret == config.api_secret,
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
