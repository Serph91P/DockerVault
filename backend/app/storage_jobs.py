"""SQLite-authoritative storage deletion, one owned worker, no destructive replay."""

import asyncio
import hashlib
import json
import logging
import os
from pathlib import Path
from uuid import uuid4

from fastapi import HTTPException
from sqlalchemy import func, select

from app import database
from app.config import settings
from app.database import RemoteStorage, StorageOperation, StorageOperationItem, _utcnow
from app.remote_storage import LocalStorage, RemoteStorageManager, _await_io_settled
from app.storage_operations import open_local_directory, validate_relative_path

logger = logging.getLogger(__name__)
ACTIVE = ("queued", "running")
OUTCOMES = ("succeeded", "failed", "skipped", "unknown")


class StorageJobs:
    """A single serial worker also serializes overlapping roots and selections."""

    def __init__(self, sessions=None, *, capacity=32):
        self.sessions = sessions or database.async_session
        self.capacity = capacity
        self.lock = asyncio.Lock()
        self.wake = asyncio.Event()
        self.worker = None
        self.stopping = False
        self.root_pins = {}

    async def _storage(self, db, storage_id):
        from app.api.storage import _db_to_config, _get_local_backend

        if storage_id == "local":
            backend = _get_local_backend()
            identity = {"root": str(settings.BACKUP_BASE_PATH)}
        else:
            row = await db.get(RemoteStorage, int(storage_id))
            if row is None:
                raise HTTPException(404, "Storage not found")
            if not row.enabled:
                raise HTTPException(409, "Storage is disabled")
            # Bind ALL configuration and creation identity, not a recyclable row ID.
            encoded = json.dumps(
                {c.name: str(getattr(row, c.name)) for c in row.__table__.columns},
                sort_keys=True,
            )
            identity = {"config": hashlib.sha256(encoded.encode()).hexdigest()}
            config = _db_to_config(row)
            backend = RemoteStorageManager._backends[config.storage_type](config)
        if isinstance(backend, LocalStorage):
            try:
                with open_local_directory(Path(backend.config.base_path)) as fd:
                    info = os.fstat(fd)
                    identity["root_inode"] = [info.st_dev, info.st_ino]
            except OSError:
                raise HTTPException(409, "Storage root is unavailable")
        return backend, identity

    async def _owned(self, db, id, user_id):
        row = await db.get(StorageOperation, id)
        if row is None or row.user_id != user_id:
            raise HTTPException(404, "Operation not found")
        return row

    async def _items(self, db, id):
        return (
            (
                await db.execute(
                    select(StorageOperationItem)
                    .where(StorageOperationItem.operation_id == id)
                    .order_by(StorageOperationItem.id)
                )
            )
            .scalars()
            .all()
        )

    def _count(self, row, items):
        for outcome in OUTCOMES:
            setattr(row, outcome, sum(item.state == outcome for item in items))
        row.completed = sum(getattr(row, outcome) for outcome in OUTCOMES)
        row.updated_at = _utcnow()

    async def submit(self, user_id, request, *, parent_id=None, expected_identity=None):
        data = request.model_dump(mode="json")
        request_id = data.pop("request_id")
        data["storage_id"] = str(data["storage_id"])
        try:
            data["path"] = validate_relative_path(data["path"], allow_root=True)
        except ValueError:
            raise HTTPException(422, "Invalid browser path")
        # Invalid targets remain itemized; never normalize away lexical escapes.
        paths = []
        for path in data["paths"]:
            try:
                path = validate_relative_path(path)
            except ValueError:
                pass
            if path not in paths:
                paths.append(path)
        if not paths or len(paths) > 1000 or any(not path.strip() for path in paths):
            raise HTTPException(422, "Select between 1 and 1000 nonempty paths")
        data["paths"] = paths
        data["parent_id"] = parent_id
        async with self.lock, self.sessions() as db:
            existing = (
                await db.execute(
                    select(StorageOperation).where(
                        StorageOperation.user_id == user_id,
                        StorageOperation.request_id == request_id,
                    )
                )
            ).scalar_one_or_none()
            if existing:
                if existing.payload != data:
                    raise HTTPException(
                        409, "Request ID already used for another selection"
                    )
                return existing.id
            if self.stopping:
                raise HTTPException(503, "Storage worker is shutting down")
            count = await db.scalar(
                select(func.count())
                .select_from(StorageOperation)
                .where(StorageOperation.state.in_(ACTIVE))
            )
            if count >= self.capacity:
                raise HTTPException(429, "Storage operation queue is full")
            backend, identity = await self._storage(db, data["storage_id"])
            await self._close_backend(backend)
            if expected_identity is not None and identity != expected_identity:
                raise HTTPException(
                    409,
                    "Storage identity changed; inspect before starting a new operation",
                )
            row = StorageOperation(
                id=str(uuid4()),
                user_id=user_id,
                request_id=request_id,
                payload=data,
                storage_identity=identity,
                parent_id=parent_id,
                action=data["action"],
                storage_id=data["storage_id"],
                path=data["path"],
                total=len(paths),
            )
            root_fd = None
            if isinstance(backend, LocalStorage):
                # Pin the accepted inode against deletion/reallocation until settlement.
                with open_local_directory(
                    Path(backend.config.base_path), expected_root=identity["root_inode"]
                ) as fd:
                    root_fd = os.dup(fd)
            try:
                db.add(row)
                await db.flush()
                db.add_all(
                    [
                        StorageOperationItem(operation_id=row.id, path=path)
                        for path in paths
                    ]
                )
                await db.commit()
                if root_fd is not None:
                    self.root_pins[row.id] = root_fd
                    root_fd = None
            finally:
                if root_fd is not None:
                    os.close(root_fd)
            self.wake.set()
            return row.id

    async def detail(self, user_id, id):
        # SQLite's legacy SELECT transaction mode does not pin a read snapshot.
        async with self.lock, self.sessions() as db:
            row = await self._owned(db, id, user_id)
            items = await self._items(db, id)
            fields = (
                "id",
                "action",
                "path",
                "state",
                "stage",
                "total",
                "completed",
                *OUTCOMES,
                "cancel_requested",
                "parent_id",
            )
            result = {key: getattr(row, key) for key in fields}
            result["storage_id"] = (
                "local" if row.storage_id == "local" else int(row.storage_id)
            )
            for key in ("created_at", "updated_at", "finished_at"):
                value = getattr(row, key)
                result[key] = value.isoformat() + "Z" if value else None
            result["items"] = [
                {"path": item.path, "state": item.state, "error": item.error}
                for item in items
            ]
            return result

    async def list(self, user_id, storage_id=None, limit=20):
        async with self.sessions() as db:
            query = select(StorageOperation.id).where(
                StorageOperation.user_id == user_id
            )
            if storage_id is not None:
                query = query.where(StorageOperation.storage_id == str(storage_id))
            # Active jobs first, then recent results, bounded by the public limit.
            ids = (
                (
                    await db.execute(
                        query.order_by(
                            StorageOperation.state.in_(ACTIVE).desc(),
                            StorageOperation.created_at.desc(),
                        ).limit(limit)
                    )
                )
                .scalars()
                .all()
            )
        return [await self.detail(user_id, id) for id in ids]

    async def cancel(self, user_id, id):
        async with self.lock, self.sessions() as db:
            row = await self._owned(db, id, user_id)
            if row.state in ACTIVE:
                row.cancel_requested = True
                row.updated_at = _utcnow()
                await db.commit()
                self.wake.set()
        return await self.detail(user_id, id)

    async def retry(self, user_id, id, request):
        from app.api.operations import CreateOperation

        async with self.sessions() as db:
            row = await self._owned(db, id, user_id)
            if row.state in ACTIVE:
                raise HTTPException(409, "Wait for operation settlement")
            failed = [i.path for i in await self._items(db, id) if i.state == "failed"]
            if not failed:
                raise HTTPException(
                    409, "No failed items to retry; inspect unknown outcomes manually"
                )
            creation = CreateOperation(
                action=row.action,
                storage_id="local"
                if row.storage_id == "local"
                else int(row.storage_id),
                path=row.path,
                paths=failed,
                request_id=request.request_id,
            )
            identity = row.storage_identity
        return await self.submit(
            user_id, creation, parent_id=id, expected_identity=identity
        )

    async def recover(self):
        """Startup only: uncertain effects stay unknown; no automatic replay."""
        async with self.lock, self.sessions() as db:
            rows = (
                (
                    await db.execute(
                        select(StorageOperation).where(
                            StorageOperation.state.in_(ACTIVE)
                        )
                    )
                )
                .scalars()
                .all()
            )
            for row in rows:
                items = await self._items(db, row.id)
                for item in items:
                    if item.state == "running":
                        item.state, item.error = (
                            "unknown",
                            "Interrupted; inspect the actual target before taking further action",
                        )
                    elif item.state == "pending":
                        item.state, item.error = (
                            "skipped",
                            "Not started before interruption",
                        )
                self._count(row, items)
                row.state, row.stage, row.finished_at = (
                    "failed",
                    "interrupted",
                    _utcnow(),
                )
            await db.commit()
            for row in rows:
                self._release_root(row.id)

    async def start(self):
        if self.worker is None or self.worker.done():
            self.stopping = False
            self.worker = asyncio.create_task(self._run(), name="storage-delete-worker")
            self.wake.set()

    async def shutdown(self):
        self.stopping = True
        self.wake.set()
        if self.worker is not None:
            # Never cancel physical I/O or release ownership on a timeout.
            await _await_io_settled(self.worker)
            self.worker = None
        await self.recover()
        for id in list(self.root_pins):
            self._release_root(id)

    def _release_root(self, id):
        fd = self.root_pins.pop(id, None)
        if fd is not None:
            os.close(fd)

    async def _close_backend(self, backend):
        from app.remote_storage import S3Storage

        if isinstance(backend, S3Storage):
            await backend.close()

    async def _notify(self, id, user_id):
        try:
            from app.websocket import manager

            await manager.send_to_user(
                user_id, {"type": "operation_update", "operation_id": id}
            )
        except Exception:
            logger.warning("Storage notification unavailable")

    async def _run(self):
        try:
            while not self.stopping:
                self.wake.clear()
                async with self.sessions() as db:
                    id = await db.scalar(
                        select(StorageOperation.id)
                        .where(StorageOperation.state == "queued")
                        .order_by(StorageOperation.created_at)
                        .limit(1)
                    )
                if id is None:
                    await self.wake.wait()
                    continue
                await self._execute(id)
                self._release_root(id)
        except Exception:
            logger.exception("Storage worker failed; preserving uncertain outcomes")
            # Stop admission rather than replaying or losing task exceptions.
            self.stopping = True
            await self.recover()

    async def _execute(self, id):
        backend = None
        try:
            async with self.lock, self.sessions() as db:
                row = await db.get(StorageOperation, id)
                row.state, row.stage = "running", "deleting"
                row.updated_at = _utcnow()
                await db.commit()
                user_id = row.user_id
            await self._notify(id, user_id)
            while True:
                async with self.lock, self.sessions() as db:
                    row = await db.get(StorageOperation, id)
                    items = await self._items(db, id)
                    pending = next((i for i in items if i.state == "pending"), None)
                    if row.cancel_requested or self.stopping:
                        for item in items:
                            if item.state == "pending":
                                item.state, item.error = (
                                    "skipped",
                                    "Not started; cancellation requested",
                                )
                        pending = None
                    if pending is None:
                        self._count(row, items)
                        row.state = (
                            "completed"
                            if row.succeeded == row.total
                            else "failed"
                            if row.failed or row.unknown
                            else "cancelled"
                        )
                        row.stage, row.finished_at = "finished", _utcnow()
                        await db.commit()
                        break
                    item_id, path = pending.id, pending.path
                    try:
                        current, identity = await self._storage(db, row.storage_id)
                        if identity != row.storage_identity:
                            await self._close_backend(current)
                            raise HTTPException(409, "Storage identity changed")
                        if backend is not None:
                            await self._close_backend(backend)
                        backend = current
                        identity_error = None
                    except HTTPException:
                        identity_error = "Storage identity changed or unavailable; target not deleted"
                    pending.state = "running"
                    row.updated_at = _utcnow()
                    await db.commit()
                    root_identity = row.storage_identity.get("root_inode")
                await self._notify(id, user_id)
                if identity_error:
                    success, error = False, identity_error
                else:
                    from app.api.storage import _delete_browser_target

                    try:
                        # Cancellation requests never cancel the current physical effect.
                        if isinstance(backend, LocalStorage):
                            result = await backend.delete_result(
                                path, expected_root=root_identity
                            )
                        else:
                            result = await _delete_browser_target(backend, path)
                        success, error = result.success, result.error
                    except Exception:
                        success, error = False, "Delete failed"
                async with self.lock, self.sessions() as db:
                    item = await db.get(StorageOperationItem, item_id)
                    item.state, item.error = (
                        ("succeeded", None) if success else ("failed", error)
                    )
                    row = await db.get(StorageOperation, id)
                    await db.flush()
                    self._count(row, await self._items(db, id))
                    await db.commit()
                await self._notify(id, user_id)
            await self._notify(id, user_id)
        finally:
            if backend is not None:
                await self._close_backend(backend)


storage_jobs = StorageJobs()
