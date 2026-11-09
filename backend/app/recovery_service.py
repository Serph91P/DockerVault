"""Portable logical state for the encrypted recovery bundle.

The explicit table/setting inventory excludes accounts, sessions, both operation
journals and logs. Validation stages in SQLite memory, never in the live engine.
"""

import enum
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import NoReturn

from sqlalchemy import Boolean, DateTime, Integer, String, select
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app import database
from app.config import settings
from app.credential_encryption import decrypt_value, encrypt_value
from app.cron import next_run
from app.komodo_config import effective_config

# Order is also the FK-safe insertion order; NEVER include Base.metadata wholesale.
PORTABLE_MODELS = (
    database.RetentionPolicy,
    database.Schedule,
    database.RemoteStorage,
    database.EncryptionConfig,
    database.AppSettings,
    database.BackupTarget,
    database.Backup,
    database.BackupSchedule,
    database.BackupStorageSync,
)
TABLES = {model.__tablename__: model.__table__ for model in PORTABLE_MODELS}
PORTABLE_SETTINGS = frozenset(
    {
        "komodo_enabled",
        "komodo_api_url",
        "komodo_api_key",
        "komodo_api_secret",
        "storage_check_quota_before_upload",
        "encryption_onboarding_deferred",
    }
)
STORAGE_SECRETS = ("password", "s3_access_key", "s3_secret_key")
KOMODO_SECRETS = frozenset({"komodo_api_key", "komodo_api_secret"})


class RecoveryStateError(ValueError):
    """A redacted validation failure safe for the setup UI."""


def _invalid() -> NoReturn:
    raise RecoveryStateError("Invalid or incompatible recovery state") from None


def _canonical(value) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode()


def _digest(value):
    return hashlib.sha256(_canonical(value)).hexdigest()


def state_manifest(payload: dict) -> dict:
    """Integrity inventory authenticated inside the envelope, not a signature."""
    return {
        "tables": {
            name: {"count": len(rows), "sha256": _digest(rows)}
            for name, rows in payload["tables"].items()
        },
        "ssh_public_keys": _digest(payload["ssh_public_keys"]),
        "rclone": _digest(payload["rclone"]),
    }


def _portable(value):
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, enum.Enum):
        return value.value
    return value


async def export_state(session) -> dict:
    """Snapshot portable records; caller owns the consistent read transaction."""
    tables = {}
    for name, table in TABLES.items():
        rows = (await session.execute(select(table).order_by(table.c.id))).mappings()
        tables[name] = [
            {key: _portable(value) for key, value in row.items()} for row in rows
        ]
    for row in tables["remote_storages"]:
        for field in STORAGE_SECRETS:
            row[field] = decrypt_value(row[field])
    raw_settings = {row["key"]: row["value"] for row in tables["app_settings"]}
    komodo = effective_config(raw_settings)
    effective = {
        "komodo_enabled": "true" if komodo.enabled else "false",
        "komodo_api_url": komodo.api_url,
        "komodo_api_key": komodo.api_key,
        "komodo_api_secret": komodo.api_secret,
    }
    next_id = max((row["id"] for row in tables["app_settings"]), default=0) + 1
    kept = []
    for row in tables["app_settings"]:
        if row["key"] in PORTABLE_SETTINGS:
            if row["key"] in effective:
                row["value"] = effective.pop(row["key"])
            kept.append(row)
    for key, value in sorted(effective.items()):
        kept.append({"id": next_id, "key": key, "value": value, "updated_at": None})
        next_id += 1
    tables["app_settings"] = sorted(kept, key=lambda row: row["id"])
    rclone_storages = [
        row for row in tables["remote_storages"] if row["storage_type"] == "rclone"
    ]
    rclone = {"schema_version": 1, "roots": [], "remotes": {}, "files": {}}
    if rclone_storages:
        from app.rclone_recovery import export_rclone

        rclone = await export_rclone(session, rclone_storages)
    payload = {
        "schema_version": 1,
        "application": "dockervault",
        "compatibility_version": 1,
        "exported_at": datetime.now(timezone.utc).isoformat(),
        "backup_root": str(Path(settings.BACKUP_BASE_PATH).absolute()),
        "tables": tables,
        "ssh_public_keys": await _public_keys(),
        "rclone": rclone,
    }
    payload["manifest"] = state_manifest(payload)
    return payload


async def _public_keys():
    from app.api.storage import _SSH_KEYS_DIR, _SSH_NAME_RE
    from cryptography.hazmat.primitives.serialization import load_ssh_public_key

    result = []
    for path in sorted(_SSH_KEYS_DIR.glob("*.pub")):
        if not _SSH_NAME_RE.fullmatch(path.stem) or path.is_symlink():
            continue
        # No private counterpart is opened or copied, including in metadata lookup.
        with path.open("rb") as stream:
            raw = stream.read(16385)
        if len(raw) > 16384:
            _invalid()
        try:
            load_ssh_public_key(raw)
            public = raw.decode().strip()
        except (ValueError, UnicodeError):
            _invalid()
        result.append({"name": path.stem, "public_key": public})
    return result


def _path(value, *, absolute=False, root=None):
    if type(value) is not str or not value or "\x00" in value or "\\" in value:
        _invalid()
    path = PurePosixPath(value)
    if ".." in value.split("/") or path.is_absolute() != absolute:
        _invalid()
    if root is not None:
        try:
            relative = path.relative_to(PurePosixPath(root))
        except ValueError:
            _invalid()
        if not relative.parts:
            _invalid()
        current = Path(settings.BACKUP_BASE_PATH)
        if current.is_symlink():
            _invalid()
        for part in relative.parts:
            current /= part
            if current.is_symlink():
                _invalid()
    return path


def _typed_rows(payload):
    if set(payload) != {
        "schema_version",
        "application",
        "compatibility_version",
        "exported_at",
        "backup_root",
        "tables",
        "ssh_public_keys",
        "rclone",
        "manifest",
    }:
        _invalid()
    for key in ("schema_version", "compatibility_version"):
        if type(payload[key]) is not int or payload[key] != 1:
            _invalid()
    if payload["application"] != "dockervault":
        _invalid()
    try:
        datetime.fromisoformat(payload["exported_at"])
    except (ValueError, TypeError):
        _invalid()
    _path(payload["backup_root"], absolute=True)
    if type(payload["tables"]) is not dict or set(payload["tables"]) != set(TABLES):
        _invalid()
    if payload["manifest"] != state_manifest(payload):
        _invalid()
    result = {}
    for name, table in TABLES.items():
        rows = payload["tables"][name]
        if type(rows) is not list or len(rows) > 100000:
            _invalid()
        converted = []
        for row in rows:
            if type(row) is not dict or set(row) != set(table.columns.keys()):
                _invalid()
            typed = {}
            for column in table.columns:
                value = row[column.name]
                if value is None:
                    if not column.nullable and not column.primary_key:
                        _invalid()
                elif isinstance(column.type, DateTime):
                    if type(value) is not str:
                        _invalid()
                    value = datetime.fromisoformat(value)
                elif isinstance(column.type, Boolean):
                    if type(value) is not bool:
                        _invalid()
                elif isinstance(column.type, Integer):
                    if type(value) is not int or not -(2**63) <= value < 2**63:
                        _invalid()
                elif getattr(column.type, "enum_class", None):
                    value = column.type.enum_class(value)
                elif isinstance(column.type, String):
                    if type(value) is not str:
                        _invalid()
                    # SQLite and the storage API accept long provider credentials;
                    # VARCHAR metadata is not their plaintext length contract.
                    credential = (
                        name == "remote_storages" and column.name in STORAGE_SECRETS
                    )
                    if (
                        column.type.length
                        and len(value) > column.type.length
                        and not credential
                    ):
                        _invalid()
                typed[column.name] = value
            if type(row["id"]) is not int or row["id"] <= 0:
                _invalid()
            converted.append(typed)
        result[name] = converted
    _validate_references_and_paths(payload)
    return result


def _validate_references_and_paths(payload):
    from app.storage_validation import STORAGE_REQUIRED

    tables = payload["tables"]
    storage_ids = {row["id"] for row in tables["remote_storages"]}
    for row in tables["app_settings"]:
        if row["key"] not in PORTABLE_SETTINGS:
            _invalid()
    for row in tables["schedules"] + tables["backup_schedules"]:
        next_run(row["cron_expression"])
    for row in tables["backup_targets"]:
        if row["target_type"] not in {"container", "volume", "path", "stack"}:
            _invalid()
        ids = row["remote_storage_ids"]
        if type(ids) is not list or any(
            type(x) is not int or x not in storage_ids for x in ids
        ):
            _invalid()
        if row["schedule_cron"]:
            next_run(row["schedule_cron"])
        if row["host_path"]:
            _path(row["host_path"], absolute=True)
    for row in tables["remote_storages"]:
        if row["storage_type"] not in STORAGE_REQUIRED:
            _invalid()
        if row["base_path"]:
            _path(row["base_path"], absolute=row["base_path"].startswith("/"))
        if row["ssh_key_path"]:
            _path(row["ssh_key_path"], absolute=True)
    for row in tables["backups"]:
        for field in ("file_path", "encryption_key_path"):
            if row[field]:
                _path(row[field], absolute=True, root=payload["backup_root"])
        if row["encryption_key_path"] and row["file_path"]:
            archive_path = PurePosixPath(row["file_path"])
            expected_key = (
                str(archive_path.with_suffix(".key"))
                if archive_path.suffix == ".enc"
                else row["file_path"] + ".key"
            )
            if row["encryption_key_path"] != expected_key:
                _invalid()
    for row in tables["backup_storage_syncs"]:
        if row["remote_path"]:
            _path(row["remote_path"])
    if any(
        row["storage_type"] == "rclone" for row in tables["remote_storages"]
    ) or payload["rclone"] != {
        "schema_version": 1,
        "roots": [],
        "remotes": {},
        "files": {},
    }:
        from app.rclone_recovery import validate_rclone

        validate_rclone(payload["rclone"], tables["remote_storages"])
    if type(payload["ssh_public_keys"]) is not list:
        _invalid()
    from app.api.storage import _SSH_NAME_RE
    from cryptography.hazmat.primitives.serialization import load_ssh_public_key

    names = set()
    for key in payload["ssh_public_keys"]:
        if type(key) is not dict or set(key) != {"name", "public_key"}:
            _invalid()
        if not _SSH_NAME_RE.fullmatch(key["name"]) or key["name"] in names:
            _invalid()
        names.add(key["name"])
        if len(key["public_key"]) > 16384:
            _invalid()
        load_ssh_public_key(key["public_key"].encode())


async def insert_state(session, payload: dict, *, protect_credentials=True):
    """Insert validated logical rows; NEVER commit or mutate external files."""
    rows = _typed_rows(payload)
    for name, table in TABLES.items():
        for row in rows[name]:
            if name == "remote_storages" and protect_credentials:
                for field in STORAGE_SECRETS:
                    row[field] = encrypt_value(row[field])
            if (
                name == "app_settings"
                and row["key"] in KOMODO_SECRETS
                and protect_credentials
            ):
                row["value"] = encrypt_value(row["value"])
            if name == "backups":
                for field in ("file_path", "encryption_key_path"):
                    if row[field]:
                        relative = PurePosixPath(row[field]).relative_to(
                            payload["backup_root"]
                        )
                        row[field] = str(Path(settings.BACKUP_BASE_PATH) / relative)
            await session.execute(table.insert().values(**row))


def _credentials_required(row, payload):
    from app.storage_validation import storage_errors

    if storage_errors(row):
        return True
    if row["storage_type"] in {"ssh", "sftp"} and row["ssh_key_path"]:
        return True
    if row["storage_type"] == "rclone":
        root = next(
            (
                root
                for root in payload["rclone"]["roots"]
                if root["storage_id"] == row["id"]
            ),
            None,
        )
        return root is None or root["state"] != "portable"
    return False


async def validate_state(payload: dict) -> dict:
    """Offline compatibility/manifest/relational/path validation and redacted preview."""
    stage = create_async_engine("sqlite+aiosqlite:///:memory:")
    try:
        async with stage.begin() as connection:
            await connection.run_sync(database.Base.metadata.create_all)
        async with async_sessionmaker(stage)() as session:
            await insert_state(session, payload, protect_credentials=False)
            await session.flush()
        return {
            "counts": {name: len(rows) for name, rows in payload["tables"].items()},
            "exported_at": payload["exported_at"],
            "storages": [
                {
                    "id": row["id"],
                    "type": row["storage_type"],
                    "state": "credentials_required"
                    if _credentials_required(row, payload)
                    else "not_tested",
                }
                for row in payload["tables"]["remote_storages"]
            ],
            "archives_included": False,
            "archive_private_keys_included": False,
        }
    except (ValueError, TypeError, KeyError, OverflowError, SQLAlchemyError):
        _invalid()
    finally:
        await stage.dispose()


async def inspect_import(session, payload, managed, directory):
    """One non-destructive post-insert observation, not a second health engine."""
    import asyncio
    import httpx

    from app.api.backups import _resolve_backup_path
    from app.api.storage import _db_to_config
    from app.komodo_config import validate_komodo_url
    from app.remote_storage import RemoteStorageManager, SSHStorage, StorageType
    from app.rclone_recovery import run_managed

    manager = RemoteStorageManager()
    connections, catalog = [], []
    backends, states = {}, {}
    try:
        for row in (
            await session.scalars(
                select(database.RemoteStorage).order_by(database.RemoteStorage.id)
            )
        ).all():
            config = _db_to_config(row)
            if _credentials_required(
                {
                    column.name: getattr(row, column.name)
                    for column in row.__table__.columns
                },
                payload,
            ):
                state = "credentials_required"
            else:
                if config.storage_type == StorageType.RCLONE:
                    config.extra_options["recovery_state"] = managed
                backend = manager.add_storage(config)
                backends[row.id] = backend
                if isinstance(backend, SSHStorage):
                    backend.KNOWN_HOSTS_FILE = str(directory / "ssh_known_hosts")
                try:
                    async with asyncio.timeout(10):
                        if config.storage_type == StorageType.LOCAL:
                            # The ordinary local connection test writes a probe; recovery only reads.
                            await asyncio.to_thread(
                                lambda: next(Path(config.base_path).iterdir(), None)
                            )
                            connected = True
                        elif config.storage_type == StorageType.RCLONE:
                            code, _, _ = await run_managed(
                                managed,
                                config.rclone_remote,
                                [
                                    "lsd",
                                    f"{config.rclone_remote}:",
                                    "--retries",
                                    "1",
                                    "--low-level-retries",
                                    "1",
                                ],
                                timeout=10,
                            )
                            connected = code == 0
                        else:
                            connected = (await backend.test_connection())[
                                "success"
                            ] is True
                    state = "connected" if connected else "offline"
                except (OSError, ValueError, TimeoutError):
                    state = "offline"
            states[row.id] = state
            connections.append({"id": row.id, "type": row.storage_type, "state": state})
        for backup in (
            await session.scalars(select(database.Backup).order_by(database.Backup.id))
        ).all():
            local = _resolve_backup_path(backup.file_path)
            local_state = "available" if local else "unavailable"
            if (
                local
                and backup.encrypted
                and (
                    not backup.encryption_key_path
                    or not Path(backup.encryption_key_path).is_file()
                )
            ):
                local_state = "credentials_required"
            copies = []
            for sync in (
                await session.scalars(
                    select(database.BackupStorageSync).where(
                        database.BackupStorageSync.backup_id == backup.id
                    )
                )
            ).all():
                state = states[sync.storage_id]
                if state == "connected":
                    backend = backends[sync.storage_id]
                    try:
                        async with asyncio.timeout(10):
                            parent = str(PurePosixPath(sync.remote_path).parent)
                            entries = await backend.list_files(
                                "" if parent == "." else parent
                            )
                            found = any(
                                item.get("name") == PurePosixPath(sync.remote_path).name
                                and not item.get("is_dir")
                                for item in entries
                            )
                        state = "available" if found else "unavailable"
                    except FileNotFoundError:
                        state = "unavailable"
                    except (OSError, ValueError, TimeoutError):
                        state = "offline"
                copies.append({"storage_id": sync.storage_id, "state": state})
            catalog.append({"id": backup.id, "local": local_state, "copies": copies})
        values = {
            row.key: row.value
            for row in (await session.scalars(select(database.AppSettings))).all()
        }
        komodo = effective_config(values)
        komodo_state = "disabled"
        if komodo.enabled:
            komodo_state = "credentials_required"
            if komodo.api_url and komodo.api_key and komodo.api_secret:
                try:
                    await asyncio.to_thread(validate_komodo_url, komodo.api_url)
                    async with httpx.AsyncClient(
                        timeout=5, follow_redirects=False, trust_env=False
                    ) as client:
                        response = await client.post(
                            komodo.api_url + "/read",
                            json={"type": "GetVersion"},
                            headers={
                                "X-Api-Key": komodo.api_key,
                                "X-Api-Secret": komodo.api_secret,
                            },
                        )
                        connected = response.status_code == 200 and isinstance(
                            response.json().get("version"), str
                        )
                    komodo_state = "connected" if connected else "offline"
                except (OSError, ValueError, httpx.HTTPError):
                    komodo_state = "offline"
        return {
            "storages": connections,
            "komodo": {"state": komodo_state},
            "catalog": catalog,
            "counts": {name: len(rows) for name, rows in payload["tables"].items()},
            "archives_modified": False,
        }
    finally:
        await manager.close()
