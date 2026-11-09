"""#143 standard-product fixture: two NEW SQLite/key installations, retained synthetic media.

Only fixture seeding/fault injection/inspection endpoints are added. Product recovery,
auth, scheduler, encryption, catalog, rclone and frontend bytes are not overlaid.
"""

import asyncio

# ruff: noqa: E402 -- isolated deployment environment must precede product imports.
from contextlib import asynccontextmanager
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import threading

MODE = os.environ.get("DV143_INSTALLATION", "source")
if len(sys.argv) > 1 and sys.argv[1] == "--inspect":
    MODE = sys.argv[2]
ROOT = Path("/app/data") / ("system143-" + MODE)
SHARED = Path("/app/data/system143-retained")
SOURCE = Path("/app/data/system143-source")
ROOT.mkdir(parents=True, exist_ok=True)
SHARED.mkdir(parents=True, exist_ok=True)
seed = os.environ["DV143_SEED"]


def marker(label):
    return hashlib.sha256((seed + ":" + label).encode()).hexdigest()


os.environ.update(
    DATABASE_URL=f"sqlite+aiosqlite:///{ROOT / 'backup.db'}",
    BACKUP_BASE_PATH="/backups/system143-" + MODE,
    COOKIE_SECURE="false",
    DOCKER_SOCKET=str(ROOT / "absent.sock"),
    ALLOWED_SSH_KEY_DIRS=str(ROOT / "ssh_keys"),
    RCLONE_CONFIG="/nonexistent/explicit-synthetic-source",
    KOMODO_ENABLED="false",
    KOMODO_ALLOWED_HOSTS="127.0.0.1",
    LOG_LEVEL="WARNING",
)
os.environ.pop("CREDENTIAL_ENCRYPTION_KEY", None)
from app import credential_encryption as credentials

credentials._KEY_FILE_PATH = ROOT / ".credential_key"
from app import database
from app.recovery_service import TABLES, export_state
from sqlalchemy import select, text
from app.api import storage

storage._SSH_KEYS_DIR = ROOT / "ssh_keys"


def archive_hashes():
    roots = [Path("/backups/system143-source"), SHARED / "local", SHARED / "crypt"]
    return {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for root in roots
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


async def inspect():
    async with database.async_session() as session:
        tables = {
            name: [
                dict(row) for row in (await session.execute(select(table))).mappings()
            ]
            for name, table in TABLES.items()
        }
        with sqlite3.connect(ROOT / "backup.db") as db:
            users = db.execute("SELECT COUNT(*) FROM users").fetchone()[0]
            sessions = db.execute("SELECT COUNT(*) FROM sessions").fetchone()[0]
        value = {
            "users": users,
            "sessions": sessions,
            "counts": {name: len(rows) for name, rows in tables.items()},
            "digests": {
                name: hashlib.sha256(
                    json.dumps(rows, sort_keys=True, default=str).encode()
                ).hexdigest()
                for name, rows in tables.items()
            },
            "archives": archive_hashes(),
            "key_exists": credentials._KEY_FILE_PATH.exists(),
            "staging_keys": len(list(ROOT.glob(".dv-recovery-key-*"))),
            "tripwire": (SHARED / "must-not-run").exists(),
        }
        if users and MODE == "destination":
            payload = await export_state(session)
            original = json.loads((SHARED / "expected-public.json").read_text())
            value["age_public_preserved"] = (
                payload["tables"]["encryption_config"][0]["public_key"]
                == original["public_key"]
            )
            value["credentials_match"] = all(
                row[field] == marker(row["storage_type"] + ":" + field)
                for row in payload["tables"]["remote_storages"]
                for field in ["password", "s3_access_key", "s3_secret_key"]
            )
            value["fresh_local_key"] = (
                credentials._KEY_FILE_PATH.read_bytes()
                != (SOURCE / ".credential_key").read_bytes()
            )
            value["private_ssh_not_restored"] = not list((ROOT / "ssh_keys").glob("*"))
            value["rclone_reexported"] = bool(payload["rclone"]["remotes"])
        print(json.dumps(value, sort_keys=True), flush=True)
    await database.engine.dispose()


if len(sys.argv) > 1 and sys.argv[1] == "--inspect":
    asyncio.run(inspect())
    raise SystemExit(0)


class SyntheticKomodo(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        good = self.headers.get("X-Api-Key") == marker(
            "komodo:key"
        ) and self.headers.get("X-Api-Secret") == marker("komodo:secret")
        content = json.dumps(
            {"version": "synthetic143"} if body.get("type") == "GetVersion" else []
        ).encode()
        self.send_response(200 if good else 401)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)


server = ThreadingHTTPServer(("127.0.0.1", 9611), SyntheticKomodo)
threading.Thread(target=server.serve_forever, daemon=True).start()
from app.main import app
from app import rclone_recovery
from app.recovery_bundle import decode_bundle
from fastapi import Request
import uvicorn

original_lifespan = app.router.lifespan_context


@asynccontextmanager
async def lifespan(application):
    async with original_lifespan(application):
        if MODE == "destination":
            async with database.engine.begin() as connection:
                await connection.execute(
                    text(
                        "CREATE TRIGGER IF NOT EXISTS test_reject_activation BEFORE INSERT ON users WHEN NEW.username='fail-activation' BEGIN SELECT RAISE(ABORT,'synthetic final transaction fault'); END"
                    )
                )
        yield


app.router.lifespan_context = lifespan


@app.post("/api/v1/test-system143/seed")
async def populate():
    assert MODE == "source"
    files = SHARED / "source-files"
    files.mkdir(exist_ok=True)
    (files / "payload.txt").write_bytes(
        b"DockerVault143 retained archive bytes\x00\xff\n"
    )
    for name in ["local", "crypt"]:
        (SHARED / name).mkdir(exist_ok=True)
    async with database.async_session() as session:
        session.add_all(
            [
                database.RetentionPolicy(
                    id=40,
                    name="Recovered policy",
                    keep_last=4,
                    keep_daily=7,
                    keep_weekly=8,
                    keep_monthly=9,
                    keep_yearly=2,
                    max_age_days=730,
                ),
                database.Schedule(
                    id=41,
                    name="Recovered schedule",
                    cron_expression="0 0 1 1 *",
                    enabled=False,
                ),
            ]
        )
        for index, kind in enumerate(
            ["local", "ssh", "sftp", "webdav", "s3", "ftp", "rclone"], 1
        ):
            session.add(
                database.RemoteStorage(
                    id=index,
                    name="Recovered " + kind,
                    storage_type=kind,
                    enabled=True,
                    host="127.0.0.1",
                    port=9,
                    username="synthetic",
                    password=credentials.encrypt_value(marker(kind + ":password")),
                    ssh_key_path=str(SOURCE / "ssh_keys/original")
                    if kind in {"ssh", "sftp"}
                    else None,
                    base_path=str(SHARED / "local") if kind == "local" else "/",
                    webdav_url="http://127.0.0.1:9",
                    s3_endpoint_url="http://127.0.0.1:9",
                    s3_bucket="fixture",
                    s3_region="us-east-1",
                    s3_access_key=credentials.encrypt_value(
                        marker(kind + ":s3_access_key")
                    ),
                    s3_secret_key=credentials.encrypt_value(
                        marker(kind + ":s3_secret_key")
                    ),
                    rclone_remote="vault" if kind == "rclone" else None,
                )
            )
        session.add_all(
            [
                database.AppSettings(key="komodo_enabled", value="true"),
                database.AppSettings(
                    key="komodo_api_url", value="http://127.0.0.1:9611"
                ),
                database.AppSettings(
                    key="komodo_api_key",
                    value=credentials.encrypt_value(marker("komodo:key")),
                ),
                database.AppSettings(
                    key="komodo_api_secret",
                    value=credentials.encrypt_value(marker("komodo:secret")),
                ),
                database.AppSettings(
                    key="storage_check_quota_before_upload", value="true"
                ),
            ]
        )
        await session.commit()
        session.add(
            database.BackupTarget(
                id=42,
                name="Recovered archive plan",
                target_type="path",
                host_path=str(files),
                enabled=True,
                schedule_id=41,
                retention_policy_id=40,
                remote_storage_ids=[],
                compression_enabled=True,
            )
        )
        session.add(
            database.BackupSchedule(
                id=43, target_id=42, cron_expression="0 0 1 1 *", enabled=False
            )
        )
        await session.commit()

        def obscure(label):
            result = subprocess.run(
                ["rclone", "obscure", "-"],
                input=marker(label).encode(),
                capture_output=True,
                check=True,
            )
            return result.stdout.decode().strip()

        remote = {
            "vault": {
                "type": "crypt",
                "remote": str(SHARED / "crypt"),
                "password": obscure("crypt:password"),
                "password2": obscure("crypt:salt"),
            }
        }
        rows = [{"id": 7, "storage_type": "rclone", "rclone_remote": "vault"}]
        payload = rclone_recovery.package_rclone(remote, rows)
        managed = await rclone_recovery.create_managed_config(
            payload, ROOT / "source-rclone"
        )
        session.add(
            database.AppSettings(key="recovery_rclone", value=json.dumps(managed))
        )
        await session.commit()
    return {"target_id": 42, "providers": 7}


@app.post("/api/v1/test-system143/finalize")
async def finalize():
    async with database.async_session() as session:
        backup = (
            await session.scalars(
                select(database.Backup).where(database.Backup.target_id == 42)
            )
        ).one()
        assert backup.status.value == "completed" and backup.encrypted
        managed = json.loads(
            await session.scalar(
                select(database.AppSettings.value).where(
                    database.AppSettings.key == "recovery_rclone"
                )
            )
        )
        for path in [Path(backup.file_path), Path(backup.encryption_key_path)]:
            shutil.copyfile(path, SHARED / "local" / path.name)
            code, _, _ = await rclone_recovery.run_managed(
                managed,
                "vault",
                ["copyto", str(path), "vault:" + path.name],
                timeout=20,
            )
            assert code == 0
        session.add_all(
            [
                database.BackupStorageSync(
                    backup_id=backup.id,
                    storage_id=index,
                    remote_path=Path(backup.file_path).name,
                    status="completed",
                )
                for index in [1, 7]
            ]
        )
        target = await session.get(database.BackupTarget, 42)
        target.enabled = False
        target.remote_storage_ids = [1, 7]
        target.pre_backup_command = (
            "touch " + str(SHARED / "must-not-run") + " # " + marker("pre-hook")
        )
        target.post_backup_command = "true # " + marker("post-hook")
        await session.commit()
        public = await session.scalar(select(database.EncryptionConfig.public_key))
    state = {
        "backup_id": backup.id,
        "archive_name": Path(backup.file_path).name,
        "public_key": public,
        "archives": archive_hashes(),
    }
    (SHARED / "expected-public.json").write_text(json.dumps(state))
    return state


@app.post("/api/v1/test-system143/verify-bundle")
async def verify_bundle(request: Request):
    body = await request.json()
    data = body["bundle"].encode()
    payload = decode_bundle(data, body["password"])
    assert set(payload["tables"]) == set(TABLES)
    assert len(payload["tables"]["remote_storages"]) == 7
    assert not payload["rclone"]["files"]
    rendered = json.dumps(payload)
    assert (
        "AGE-SECRET-KEY-" not in rendered
        and "BEGIN OPENSSH PRIVATE KEY" not in rendered
    )
    assert not any(
        marker(label).encode() in data
        for label in [
            "account",
            "bundle",
            "komodo:key",
            "komodo:secret",
            "pre-hook",
            "post-hook",
        ]
        + [
            kind + ":" + field
            for kind in ["local", "ssh", "sftp", "webdav", "s3", "ftp", "rclone"]
            for field in ["password", "s3_access_key", "s3_secret_key"]
        ]
    )
    return {"all_tables": True, "providers": 7, "no_private_keys_or_archives": True}


@app.post("/api/v1/test-system143/switch")
async def switch():
    assert MODE == "source"

    def execute():
        os.environ["DV143_INSTALLATION"] = "destination"
        os.environ["DV143_NGINX_STARTED"] = "true"
        os.execv(sys.executable, [sys.executable, __file__])

    threading.Timer(0.3, execute).start()
    return {"accepted": True, "not_completed": True}


@app.post("/api/v1/test-system143/restart")
async def restart():
    def execute():
        os.environ["DV143_NGINX_STARTED"] = "true"
        os.execv(sys.executable, [sys.executable, __file__])

    threading.Timer(0.3, execute).start()
    return {"accepted": True, "not_completed": True}


if "DV143_NGINX_STARTED" not in os.environ:
    subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
