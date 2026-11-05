"""Local storage operations anchored to directory descriptors, never symlink targets."""

import asyncio
import errno
import logging
import os
import shutil
import stat
import threading
import time
from collections import OrderedDict
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path, PureWindowsPath
from typing import Literal

from anyio import CancelScope

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class DeleteResult:
    path: str
    success: bool
    kind: Literal["file", "directory", "symlink"] | None = None
    error: str | None = None
    status_code: int = 200


def validate_relative_path(path: str, *, allow_root: bool = False) -> str:
    """Reject lexical escapes before normalization can erase their evidence."""
    if (
        "\x00" in path
        or "\\" in path
        or path.startswith("/")
        or PureWindowsPath(path).drive
        or ".." in path.split("/")
    ):
        raise ValueError("Invalid file path")
    parts = [part for part in path.split("/") if part not in ("", ".")]
    if not parts and not allow_root:
        raise ValueError("Storage root is not a file target")
    return "/".join(parts)


@contextmanager
def open_local_directory(root: Path, relative_path: str = "", *, expected_root=None):
    """Pin each parent with O_NOFOLLOW; replacement names cannot redirect I/O."""
    path = validate_relative_path(relative_path, allow_root=True)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    fd = os.open(root, flags)
    try:
        if expected_root is not None:
            info = os.fstat(fd)
            if [info.st_dev, info.st_ino] != list(expected_root):
                raise ValueError("Storage root changed")
        for part in path.split("/") if path else ():
            child = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd
    finally:
        os.close(fd)


def delete_local_target(
    root: Path,
    relative_path: str,
    *,
    missing_ok: bool = False,
    allow_directories: bool = True,
    expected_root=None,
) -> DeleteResult:
    kind: Literal["file", "directory", "symlink"] | None = None
    try:
        path = validate_relative_path(relative_path)
        parent, _, name = path.rpartition("/")
        with open_local_directory(root, parent, expected_root=expected_root) as fd:
            info = os.stat(name, dir_fd=fd, follow_symlinks=False)
            kind = (
                "symlink"
                if stat.S_ISLNK(info.st_mode)
                else "directory"
                if stat.S_ISDIR(info.st_mode)
                else "file"
            )
            if kind == "directory":
                if not allow_directories:
                    return DeleteResult(
                        relative_path, False, kind, "Target is not a file", 409
                    )
                if not shutil.rmtree.avoids_symlink_attacks:
                    raise OSError("Safe recursive deletion is unavailable")
                shutil.rmtree(name, dir_fd=fd)
            else:
                os.unlink(name, dir_fd=fd)
        return DeleteResult(relative_path, True, kind)
    except ValueError:
        return DeleteResult(relative_path, False, kind, "Invalid file path", 400)
    except FileNotFoundError:
        return DeleteResult(
            relative_path,
            missing_ok,
            kind,
            None if missing_ok else "Target not found",
            200 if missing_ok else 404,
        )
    except PermissionError:
        return DeleteResult(relative_path, False, kind, "Permission denied", 403)
    except OSError as exc:
        if exc.errno in (errno.ELOOP, errno.ENOTDIR):
            return DeleteResult(relative_path, False, kind, "Invalid file path", 400)
        logger.exception("Local storage deletion failed")
        return DeleteResult(relative_path, False, kind, "Delete failed", 500)
    except Exception:
        logger.exception("Local storage deletion failed")
        return DeleteResult(relative_path, False, kind, "Delete failed", 500)


def list_local_files(root: Path, relative_path: str):
    with open_local_directory(root, relative_path) as fd:
        files = []
        with os.scandir(fd) as entries:
            for entry in entries:
                info = entry.stat(follow_symlinks=False)
                is_dir = stat.S_ISDIR(info.st_mode)
                files.append(
                    {
                        "name": entry.name,
                        "size": info.st_size if stat.S_ISREG(info.st_mode) else None,
                        "is_dir": is_dir,
                        "modified": info.st_mtime,
                    }
                )
        return files


def _stamp(info):
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def _check_cancel(cancel):
    if cancel.is_set():
        raise InterruptedError


def _fingerprints_match(root, fingerprints, cancel):
    # Check every recorded file AND directory: nested writes and new names cannot
    # hide behind the selected directory's unchanged mtime. Never follow parents.
    for path, expected in fingerprints:
        _check_cancel(cancel)
        parent, _, name = path.rpartition("/")
        with open_local_directory(root, parent) as fd:
            if _stamp(os.stat(name, dir_fd=fd, follow_symlinks=False)) != expected:
                return False
    return True


def scan_local_size(root: Path, relative_path: str, cancel: threading.Event):
    path = validate_relative_path(relative_path)
    fingerprints = []

    def walk(fd, prefix):
        _check_cancel(cancel)
        before = _stamp(os.fstat(fd))
        fingerprints.append((prefix, before))
        total = 0
        with os.scandir(fd) as entries:
            for entry in entries:
                _check_cancel(cancel)
                info = entry.stat(follow_symlinks=False)
                child_path = f"{prefix}/{entry.name}"
                if stat.S_ISDIR(info.st_mode):
                    child = os.open(
                        entry.name,
                        os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                        dir_fd=fd,
                    )
                    try:
                        if _stamp(os.fstat(child)) != _stamp(info):
                            raise OSError("Directory changed")
                        total += walk(child, child_path)
                    finally:
                        os.close(child)
                else:
                    fingerprints.append((child_path, _stamp(info)))
                    if stat.S_ISREG(info.st_mode):
                        total += info.st_size
        if _stamp(os.fstat(fd)) != before:
            raise OSError("Directory changed")
        return total

    with open_local_directory(root, path) as fd:
        total = walk(fd, path)
    if not _fingerprints_match(root, fingerprints, cancel):
        raise OSError("Directory contents changed")
    return total, fingerprints


class DirectorySizes:
    """Request-scoped worker lifetime, bounded metadata cache and I/O concurrency."""

    def __init__(self, *, max_entries=128, ttl=5.0, parallelism=2):
        self.cache = OrderedDict()
        self.max_entries = max_entries
        self.ttl = ttl
        self.semaphore = asyncio.Semaphore(parallelism)
        self.lock = threading.Lock()

    def invalidate(self, root: Path):
        with self.lock:
            for key in list(self.cache):
                if key[1] == str(root):
                    del self.cache[key]

    def _get(self, identity, root, path, cancel):
        path = validate_relative_path(path)
        with open_local_directory(root) as fd:
            root_identity = _stamp(os.fstat(fd))[:2]
        key = (identity, str(root), root_identity, path)
        with self.lock:
            cached = self.cache.pop(key, None)
        if (
            cached
            and time.monotonic() - cached[0] < self.ttl
            and _fingerprints_match(root, cached[2], cancel)
        ):
            total, fingerprints = cached[1:]
            created = cached[0]
        else:
            total, fingerprints = scan_local_size(root, path, cancel)
            created = time.monotonic()
        _check_cancel(cancel)
        # Bound both entry count and per-entry metadata; large trees still work.
        if len(fingerprints) <= 4096:
            with self.lock:
                self.cache[key] = (created, total, fingerprints)
                while len(self.cache) > self.max_entries:
                    self.cache.popitem(last=False)
        return {"path": path, "state": "completed", "bytes": total}

    async def get(self, identity, root: Path, path: str):
        cancel = threading.Event()
        async with self.semaphore:
            work = asyncio.create_task(
                asyncio.to_thread(self._get, identity, root, path, cancel)
            )
            cancellation = None
            while not work.done():
                try:
                    with CancelScope(shield=cancellation is not None):
                        await asyncio.shield(work)
                except asyncio.CancelledError as exc:
                    cancellation = exc
                    cancel.set()
                except Exception:
                    break
            if cancellation is not None:
                if not work.cancelled():
                    work.exception()
                raise cancellation
            try:
                return work.result()
            except Exception:
                return {"path": path, "state": "unavailable", "bytes": None}


directory_sizes = DirectorySizes()
