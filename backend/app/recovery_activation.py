"""Shared first-run claim and durable recovery preparation journal.

The final SQL transaction is the ONLY activation point. Its generation's encrypted
rclone files already exist; DB rows, their file reference and admin session commit
as one unit. A killed pre-commit attempt leaves only a fenced preparation marker.
"""

import asyncio
import fcntl
import hashlib
import json
import os
import re
import shutil
import uuid
from contextlib import asynccontextmanager, nullcontext
from pathlib import Path
from weakref import WeakKeyDictionary

from sqlalchemy import delete, func, select, text

from app.database import AppSettings, User

_MEMORY_CLAIMS = WeakKeyDictionary()
_MEMORY_CRYPTO = WeakKeyDictionary()
_GENERATION = re.compile(r"[a-f0-9]{32}\Z")


class SetupClaimError(ValueError):
    """Safe conflict for Create New and Restore, never a successful setup."""


def _database_path(sessions):
    bind = sessions.kw["bind"]
    if bind.dialect.name != "sqlite":
        raise SetupClaimError("System recovery requires SQLite")
    name = bind.url.database
    return Path(name).absolute() if name and name != ":memory:" else None


def generation_directory(sessions, generation):
    if type(generation) is not str or not _GENERATION.fullmatch(generation):
        raise SetupClaimError("Invalid recovery generation")
    database = _database_path(sessions)
    if database is None:
        raise SetupClaimError("Durable recovery requires a file-backed database")
    root = database.parent / ".recovery"
    if root.is_symlink():
        raise SetupClaimError("Unsafe recovery directory")
    return root / generation


@asynccontextmanager
async def _lock(sessions, *, crypto=False):
    path = _database_path(sessions)
    if path is None:
        gates = _MEMORY_CRYPTO if crypto else _MEMORY_CLAIMS
        gate = gates.setdefault(sessions.kw["bind"], asyncio.Lock())
        if gate.locked():
            raise SetupClaimError("Setup is already in progress")
        async with gate:
            yield
        return
    suffix = ".recovery-crypto.lock" if crypto else ".setup.lock"
    descriptor = os.open(
        str(path) + suffix, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600
    )
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SetupClaimError("Setup is already in progress") from None
        yield
    finally:
        os.close(descriptor)


async def _discard_incomplete(sessions):
    async with sessions() as session:
        await session.execute(text("BEGIN IMMEDIATE"))
        record = (
            await session.execute(
                select(AppSettings).where(AppSettings.key == "recovery_activation")
            )
        ).scalar_one_or_none()
        if record is None or (await session.scalar(select(func.count(User.id)))):
            return False
        try:
            value = json.loads(record.value)
            if value["state"] != "preparing":
                return False
            directory = generation_directory(sessions, value["generation"])
        except (ValueError, KeyError, TypeError):
            raise SetupClaimError("Recovery preparation journal needs repair") from None
        if directory.is_symlink():
            raise SetupClaimError("Unsafe recovery generation")
        if directory.exists():
            # Only this journaled, generated directory; never source configs or archives.
            await asyncio.to_thread(shutil.rmtree, directory)
        if value.get("instance_key_preparing"):
            from app import credential_encryption as credentials

            key = credentials._KEY_FILE_PATH
            staging = key.with_name(".dv-recovery-key-" + value["generation"])
            if staging.is_symlink():
                raise SetupClaimError("Unsafe instance key preparation")
            owned = False
            if key.is_file() and not key.is_symlink():
                if staging.exists():
                    owned = os.path.samestat(key.stat(), staging.stat())
                elif value.get("key_digest"):
                    info = key.stat()
                    with key.open("rb") as stream:
                        digest = hashlib.sha256(stream.read(1024)).hexdigest()
                    owned = (
                        info.st_ino == value["key_inode"]
                        and info.st_dev == value["key_device"]
                        and digest == value["key_digest"]
                    )
            if owned:
                key.unlink()
            staging.unlink(missing_ok=True)
            credentials._fernet = None
            descriptor = os.open(
                key.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
            )
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        await session.execute(
            delete(AppSettings).where(AppSettings.key == "recovery_activation")
        )
        await session.commit()
        return True


@asynccontextmanager
async def crypto_claim(sessions):
    async with _lock(sessions, crypto=True):
        yield


async def recover_incomplete_setup(sessions):
    """Startup can clean an abandoned generation, never a living claimant's state."""
    try:
        async with _lock(sessions):
            return await _discard_incomplete(sessions)
    except SetupClaimError:
        return False


@asynccontextmanager
async def setup_claim(sessions):
    async with _lock(sessions):
        async with sessions() as session:
            if await session.scalar(select(func.count(User.id))):
                raise SetupClaimError("Setup already completed. Use login instead.")
        await _discard_incomplete(sessions)
        yield


async def recovery_pending(session):
    value = await session.scalar(
        select(AppSettings.value).where(AppSettings.key == "recovery_activation")
    )
    if value is None:
        return False
    try:
        return json.loads(value).get("state") != "active"
    except (ValueError, TypeError, AttributeError):
        return True


async def require_recovery_ready(session):
    if await recovery_pending(session):
        raise SetupClaimError("System recovery setup is in progress")


async def begin_recovery(session):
    """Journal the generated material before creating it; caller commits this claim."""
    await session.execute(text("BEGIN IMMEDIATE"))
    if await session.scalar(select(func.count(User.id))):
        raise SetupClaimError("Setup already completed. Use login instead.")
    generation = uuid.uuid4().hex
    session.add(
        AppSettings(
            key="recovery_activation",
            value=json.dumps({"state": "preparing", "generation": generation}),
        )
    )
    await session.flush()
    return generation


class RecoveryActivationError(ValueError):
    """An internal activation failure, not an expected offline connection result."""


def _sync_generation(directory):
    for path in (directory, directory.parent, directory.parent.parent):
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


async def _prepare_instance_key(sessions, generation):
    """Journal ownership before lazy credential encryption creates deployment state."""
    from app import credential_encryption as credentials
    from app.config import settings
    from app.remote_storage import _await_io_settled
    from cryptography.fernet import Fernet

    key = credentials._KEY_FILE_PATH
    if settings.CREDENTIAL_ENCRYPTION_KEY or key.exists():
        return
    staging = key.with_name(".dv-recovery-key-" + generation)

    async def journal(fields):
        async with sessions() as session:
            record = (
                await session.scalars(
                    select(AppSettings).where(AppSettings.key == "recovery_activation")
                )
            ).one()
            value = json.loads(record.value)
            value.update(fields)
            record.value = json.dumps(value)
            await _await_io_settled(session.commit())

    await journal({"instance_key_preparing": True})

    def prepare():
        key.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(
            staging, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600
        )
        with os.fdopen(descriptor, "wb") as stream:
            value = Fernet.generate_key()
            stream.write(value)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(
            staging, key
        )  # Same directory/filesystem; never replace an existing key.
        descriptor = os.open(key.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        info = key.stat()
        return {
            "key_digest": hashlib.sha256(value).hexdigest(),
            "key_inode": info.st_ino,
            "key_device": info.st_dev,
        }

    identity = await _await_io_settled(asyncio.to_thread(prepare))
    await journal(identity)
    staging.unlink()
    descriptor = os.open(key.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    credentials._fernet = None


async def activate_state(payload, username, password, sessions, *, projection=None):
    """Logically activate only after EVERY fallible internal preparation succeeds."""
    from app.auth import create_session, hash_password
    from app.recovery_service import (
        TABLES,
        inspect_import,
        insert_state,
        validate_state,
    )
    from app.rclone_recovery import create_managed_config
    from app.remote_storage import _await_io_settled

    await validate_state(payload)
    if (
        not isinstance(username, str)
        or not 3 <= len(username) <= 255
        or not isinstance(password, str)
        or not 8 <= len(password) <= 128
    ):
        raise RecoveryActivationError("Invalid administrator account")
    async with setup_claim(sessions):
        async with sessions() as session:
            generation = await begin_recovery(session)
            await session.commit()
        directory = generation_directory(sessions, generation)
        try:
            await _prepare_instance_key(sessions, generation)
            directory.parent.mkdir(mode=0o700, exist_ok=True)
            managed = None
            if payload["rclone"]["roots"]:
                managed = await create_managed_config(payload["rclone"], directory)
            else:
                directory.mkdir(mode=0o700)
            async with sessions() as session:
                await session.execute(text("BEGIN IMMEDIATE"))
                if await session.scalar(select(func.count(User.id))):
                    raise SetupClaimError("Setup already completed. Use login instead.")
                # The journal survives SQLite rollback; imported tables never commit partially.
                for table in reversed(tuple(TABLES.values())):
                    await session.execute(table.delete())
                await insert_state(session, payload)
                if managed is not None:
                    session.add(
                        AppSettings(key="recovery_rclone", value=json.dumps(managed))
                    )
                session.add(
                    AppSettings(
                        key="recovery_ssh_public_keys",
                        value=json.dumps(payload["ssh_public_keys"]),
                    )
                )
                result = await inspect_import(session, payload, managed, directory)
                user = User(
                    username=username,
                    password_hash=await asyncio.to_thread(hash_password, password),
                    is_admin=True,
                )
                session.add(user)
                await session.flush()
                token = await create_session(user.id, session, commit=False)
                session.add(
                    AppSettings(
                        key="recovery_activation",
                        value=json.dumps({"state": "active", "generation": generation}),
                    )
                )
                session.add(
                    AppSettings(key="recovery_result", value=json.dumps(result))
                )
                await session.flush()
                response = {
                    "token": token,
                    "user": {
                        "id": user.id,
                        "username": user.username,
                        "is_admin": True,
                        "created_at": user.created_at.isoformat(),
                        "last_login": None,
                    },
                    "result": result,
                }
                context = (
                    projection.recovery_projection(session, generation)
                    if projection is not None
                    else nullcontext()
                )
                cancellation = None
                async with context:
                    await _await_io_settled(
                        asyncio.to_thread(_sync_generation, directory)
                    )
                    # Settle the durable decision before compensating RAM state on disconnect.
                    commit = asyncio.create_task(session.commit())
                    try:
                        await _await_io_settled(commit)
                    except asyncio.CancelledError as error:
                        commit.result()
                        cancellation = error
                if cancellation is not None:
                    raise cancellation
                return response
        except BaseException as error:
            # Failed pre-commit work rolled back; a settled successful commit stays authoritative.
            # After a completed commit, user presence deliberately protects its generation.
            await _discard_incomplete(sessions)
            if isinstance(error, asyncio.CancelledError):
                raise
            if isinstance(error, SetupClaimError):
                raise
            raise RecoveryActivationError(
                "System recovery activation failed; check setup status before retrying"
            ) from None
