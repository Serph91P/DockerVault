"""Short-lived, exact copy confirmations; no filter is evaluated during execution."""

import asyncio
import copy
import hashlib
import json
import secrets
import time
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from fastapi import HTTPException
from pydantic import BaseModel, Field, model_validator
from sqlalchemy import select

from app.backup_engine import backup_engine
from app.database import (
    Backup,
    BackupStatus,
    BackupStorageSync,
    BackupTarget,
    RemoteStorage,
    WorkloadOperation,
    async_session,
)
from app.encryption import key_sidecar_path
from app.remote_storage import storage_manager, _await_io_settled


class RemovalSelection(BaseModel):
    scope: Literal["local", "remote", "all"]
    backup_ids: list[int] | None = None
    target_id: int | None = Field(default=None, gt=0)
    status: BackupStatus | None = None

    @model_validator(mode="after")
    def validate_selection(self):
        if self.backup_ids is not None and (
            not self.backup_ids or any(i <= 0 for i in self.backup_ids)
        ):
            raise ValueError("Select at least one valid archive ID")
        if self.backup_ids is not None and (
            self.target_id is not None or self.status is not None
        ):
            raise ValueError("Use archive IDs or a filter, not both")
        return self


class RemovalExecution(BaseModel):
    token: str = Field(min_length=1, max_length=128)


@dataclass
class Confirmation:
    entries: list[dict]
    retained: list[dict]
    expires: float = field(default_factory=lambda: time.monotonic() + 900)
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    result: dict | None = None


_confirmations: dict[str, Confirmation] = {}


def _remember(entries, retained):
    for token, item in list(_confirmations.items()):
        if item.expires < time.monotonic() and not item.lock.locked():
            del _confirmations[token]
    if len(_confirmations) >= 100:
        for token, item in list(_confirmations.items()):
            if item.result is not None:
                del _confirmations[token]
                break
    if len(_confirmations) >= 100:
        raise HTTPException(429, "Too many removal previews; wait for expiry")
    token = secrets.token_urlsafe(32)
    _confirmations[token] = Confirmation(
        copy.deepcopy(entries), copy.deepcopy(retained)
    )
    return token


def _stat(path):
    if not path:
        return None
    try:
        s = Path(path).stat()
        return [s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns]
    except FileNotFoundError:
        return None


def _storage_identity(storage):
    if storage is None:
        return None
    # Include effective destination/credentials, never return them to the client.
    values = {
        c.name: str(getattr(storage, c.name)) for c in RemoteStorage.__table__.columns
    }
    return hashlib.sha256(json.dumps(values, sort_keys=True).encode()).hexdigest()


async def _identity(session, backup, entry):
    common = [
        backup.target_id,
        bool(backup.encrypted),
        str(backup.created_at),
        backup.checksum,
        backup.file_size,
    ]
    if entry["copy"] == "local":
        return common + [
            backup.file_path,
            backup.encryption_key_path,
            _stat(entry.get("archive")),
            _stat(entry.get("key")),
        ]
    row = await session.get(BackupStorageSync, entry["sync_id"])
    storage = await session.get(RemoteStorage, entry["storage_id"])
    return (
        common
        + [
            row.remote_path,
            row.sync_status,
            str(row.synced_at),
            _storage_identity(storage),
        ]
        if row
        else None
    )


async def _shared_identity(session, entry):
    """Never unlink bytes still owned by another catalog identity."""
    paths = {p for p in (entry["archive"], entry["key"]) if p}
    if entry["copy"] == "local":
        paths = {str(Path(p).resolve()) for p in paths}
        others = (
            await session.execute(
                select(Backup.file_path, Backup.encryption_key_path).where(
                    Backup.id != entry["backup_id"]
                )
            )
        ).all()
        return any(str(Path(p).resolve()) in paths for row in others for p in row if p)
    others = (
        await session.execute(
            select(BackupStorageSync.remote_path, Backup.encrypted)
            .join(Backup, Backup.id == BackupStorageSync.backup_id)
            .where(
                BackupStorageSync.storage_id == entry["storage_id"],
                BackupStorageSync.id != entry["sync_id"],
            )
        )
    ).all()
    return any(
        paths & ({path, key_sidecar_path(path)} if encrypted else {path})
        for path, encrypted in others
        if path
    )


def _public(entry):
    return {
        k: entry[k]
        for k in (
            "backup_id",
            "target_id",
            "target_name",
            "copy",
            "storage_id",
            "label",
        )
    }


async def preview_removal(selection: RemovalSelection):
    from app.api.backups import _resolve_backup_path

    async with async_session() as session:
        query = select(Backup).order_by(Backup.id)
        if selection.backup_ids is not None:
            query = query.where(Backup.id.in_(set(selection.backup_ids)))
        if selection.target_id is not None:
            query = query.where(Backup.target_id == selection.target_id)
        if selection.status is not None:
            query = query.where(Backup.status == selection.status)
        backups = list((await session.scalars(query)).all())
        if selection.backup_ids is not None and {b.id for b in backups} != set(
            selection.backup_ids
        ):
            raise HTTPException(
                409, "Selection changed: archive not found; review again"
            )
        entries, retained = [], []
        for backup in backups:
            target = await session.get(BackupTarget, backup.target_id)
            base = {
                "backup_id": backup.id,
                "target_id": backup.target_id,
                "target_name": target.name if target else "Historical archive",
            }
            copies = []
            if backup.file_path or backup.encryption_key_path:
                archive = _resolve_backup_path(backup.file_path) or backup.file_path
                key = (
                    _resolve_backup_path(backup.encryption_key_path)
                    or backup.encryption_key_path
                )
                copies.append(
                    base
                    | {
                        "copy": "local",
                        "storage_id": None,
                        "label": "Local copy",
                        "archive": archive,
                        "key": key,
                    }
                )
            rows = (
                await session.scalars(
                    select(BackupStorageSync).where(
                        BackupStorageSync.backup_id == backup.id
                    )
                )
            ).all()
            for row in rows:
                storage = await session.get(RemoteStorage, row.storage_id)
                copies.append(
                    base
                    | {
                        "copy": "remote",
                        "storage_id": row.storage_id,
                        "sync_id": row.id,
                        "label": storage.name
                        if storage
                        else f"Storage {row.storage_id}",
                        "archive": row.remote_path,
                        "key": key_sidecar_path(row.remote_path)
                        if backup.encrypted and row.remote_path
                        else None,
                    }
                )
            for entry in copies:
                if selection.scope != "all" and entry["copy"] != selection.scope:
                    retained.append(_public(entry) | {"outcome": "retained"})
                    continue
                entry["identity"] = await _identity(session, backup, entry)
                # Failed substeps stay in the retry scope; completed ones never repeat.
                entry["remaining"] = [p for p in (entry["archive"], entry["key"]) if p]
                entry["require_key_removal"] = selection.scope == "all"
                entries.append(entry)
        token = _remember(entries, retained)
        return {
            "token": token,
            "scope": selection.scope,
            "archive_count": len(backups),
            "copy_count": len(entries),
            "items": [_public(e) for e in entries],
            "retained": retained,
            "expires_in_seconds": 900,
        }


async def _remove_entry(session, backup, entry):
    from app.api.backups import _db_to_storage_config

    if entry["copy"] == "local":
        remote_remains = await session.scalar(
            select(BackupStorageSync.id)
            .where(BackupStorageSync.backup_id == backup.id)
            .limit(1)
        )
        for path in list(entry["remaining"]):
            if path == entry["key"] and remote_remains:
                continue  # Required by retained remote recovery points.
            Path(path).unlink(missing_ok=True)
            entry["remaining"].remove(path)
        backup.file_path = None
        backup.backup_metadata = dict(backup.backup_metadata or {}) | {
            "local_copy_removed": True
        }
        if not remote_remains:
            backup.encryption_key_path = None
    else:
        storage = await session.get(RemoteStorage, entry["storage_id"])
        if not storage or not storage.enabled:
            raise OSError("Remote storage is unavailable")
        config = _db_to_storage_config(storage)
        if (
            not storage_manager.get_backend(storage.id)
            or storage_manager.configs.get(storage.id) != config
        ):
            storage_manager.add_storage(config)
        backend = storage_manager.get_backend(storage.id)
        if not backend:
            raise OSError("Remote storage is unavailable")
        for path in list(entry["remaining"]):
            if not await _await_io_settled(backend.delete(path)):
                raise OSError("Remote provider did not confirm deletion")
            entry["remaining"].remove(path)
        if not entry["archive"]:
            raise OSError("Remote copy has no confirmed path")
        await session.delete(await session.get(BackupStorageSync, entry["sync_id"]))


async def execute_removal(token: str):
    # Keep copy claims and catalog settlement alive through client cancellation.
    return await _await_io_settled(_execute_removal(token))


async def _execute_removal(token: str):
    confirmation = _confirmations.get(token)
    if confirmation is None or confirmation.expires < time.monotonic():
        raise HTTPException(
            409, "Preview expired or server restarted; review the scope again"
        )
    async with confirmation.lock:
        if confirmation.result is not None:
            return confirmation.result
        items, failed = [], []
        # Remove remote copies first so local key cleanup knows the final retained scope.
        for entry in sorted(confirmation.entries, key=lambda e: e["copy"] == "local"):
            outcome, message = (
                "skipped",
                "Archive or copy no longer exists; review again",
            )
            async with AsyncExitStack() as claims:
                lock = backup_engine._get_target_lock(entry["target_id"])
                if lock.locked():
                    items.append(
                        _public(entry)
                        | {
                            "outcome": "failed",
                            "message": "Active backup, restore or sync; retry when it finishes",
                        }
                    )
                    failed.append(entry)
                    continue
                await claims.enter_async_context(lock)
                if entry["copy"] == "remote":
                    for path in sorted(
                        {p for p in (entry["archive"], entry["key"]) if p}
                    ):
                        remote_lock = backup_engine._remote_sync_locks.setdefault(
                            (entry["storage_id"], path), asyncio.Lock()
                        )
                        await claims.enter_async_context(remote_lock)
                async with async_session() as session:
                    backup = await session.get(Backup, entry["backup_id"])
                    if backup:
                        identity = await _identity(session, backup, entry)
                        active = await session.scalar(
                            select(WorkloadOperation.id)
                            .where(
                                WorkloadOperation.backup_id == backup.id,
                                WorkloadOperation.state != "finished",
                            )
                            .limit(1)
                        )
                        if identity != entry["identity"]:
                            message = "Copy scope changed; review again before deleting"
                        elif await _shared_identity(session, entry):
                            message = "Copy is shared by another archive identity; retained for safety"
                        elif active or backup.status in (
                            BackupStatus.PENDING,
                            BackupStatus.RUNNING,
                        ):
                            outcome, message = (
                                "failed",
                                "Active or unresolved operation; retry after recovery",
                            )
                            failed.append(entry)
                        else:
                            try:
                                await _remove_entry(session, backup, entry)
                                await session.flush()
                                remaining_remote = await session.scalar(
                                    select(BackupStorageSync.id)
                                    .where(BackupStorageSync.backup_id == backup.id)
                                    .limit(1)
                                )
                                if (
                                    not backup.file_path
                                    and not backup.encryption_key_path
                                    and not remaining_remote
                                ):
                                    await session.delete(backup)
                                await session.commit()
                                outcome, message = "succeeded", "Selected copy removed"
                                if (
                                    entry["copy"] == "local"
                                    and entry["require_key_removal"]
                                    and entry["remaining"]
                                ):
                                    entry["identity"] = await _identity(
                                        session, backup, entry
                                    )
                                    outcome, message = (
                                        "failed",
                                        "Archive removed; confirmed key retained until remote deletion succeeds",
                                    )
                                    failed.append(entry)
                            except Exception:
                                # Preserve catalog identity even after partial filesystem/provider I/O.
                                await session.rollback()
                                backup = await session.get(Backup, entry["backup_id"])
                                entry["identity"] = await _identity(
                                    session, backup, entry
                                )
                                outcome, message = (
                                    "failed",
                                    "Copy removal incomplete; metadata retained for retry",
                                )
                                failed.append(entry)
                    items.append(
                        _public(entry) | {"outcome": outcome, "message": message}
                    )
        counts = {
            name: sum(i["outcome"] == name for i in items)
            for name in ("succeeded", "failed", "skipped")
        }
        result = {
            "status": "incomplete"
            if counts["failed"] or counts["skipped"]
            else "completed",
            **counts,
            "items": items + confirmation.retained,
            "retry_token": None,
        }
        if result["status"] == "incomplete":
            result["warning"] = (
                "Not all confirmed copies were removed. Retained and failed copies are not a success."
            )
        confirmation.result = result
        # Publish settlement before allocating retry scope, even at the preview cap.
        if failed:
            result["retry_token"] = _remember(failed, confirmation.retained)
        return result
