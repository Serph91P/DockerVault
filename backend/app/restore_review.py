"""Narrow restore impact binding, not a dry run or a durable operation."""

import hashlib
import json
import os
from pathlib import Path

from app.config import settings
from app.docker_client import docker_client


class RestoreReviewChanged(ValueError):
    """The confirmed impact no longer describes the execution."""


def path_identity(path: str) -> dict:
    resolved = Path(os.path.realpath(path))
    parent = resolved
    while not parent.exists():
        if parent == parent.parent:
            raise ValueError("Destination is unavailable")
        parent = parent.parent
    stat = parent.stat()
    if not parent.is_dir():
        raise ValueError("Destination is not a directory")
    return {
        "path": str(resolved),
        "ancestor": str(parent),
        "device": stat.st_dev,
        "inode": stat.st_ino,
    }


def target_identity(target) -> dict:
    return {
        field: getattr(target, field)
        for field in (
            "id",
            "target_type",
            "host_path",
            "volume_name",
            "container_name",
            "stop_container",
            "dependencies",
        )
    }


async def restore_impact(backup, target, selection) -> dict:
    if not target:
        raise ValueError("Source plan is unavailable")
    kind = selection.destination_type or (
        "path" if selection.target_path else "original"
    )
    volume = None
    if kind == "volume" or (kind == "original" and target.target_type == "volume"):
        name = selection.target_volume if kind == "volume" else target.volume_name
        volumes = await docker_client.list_volumes()
        volume = next((v for v in volumes if v.name == name), None)
        if not volume or not volume.mountpoint:
            raise ValueError("Destination volume is unavailable")
        path = volume.mountpoint
    elif kind == "path":
        path = selection.target_path
        resolved = os.path.realpath(path)
        allowed = [
            os.path.realpath(settings.BACKUP_BASE_PATH),
            "/var/lib/docker/volumes",
        ]
        if not any(
            resolved == root or resolved.startswith(root + os.sep) for root in allowed
        ):
            raise ValueError("Invalid restore path: must be within allowed directories")
    elif target.target_type == "path" and target.host_path:
        path = target.host_path
    else:
        raise ValueError(
            "Original container/stack restore is unsupported; choose a custom destination"
        )
    destination = {
        "type": kind,
        "path": os.path.realpath(path),
        "volume": volume.name if volume else None,
    }
    names = []
    if target.stop_container and target.container_name:
        names.append(target.container_name)
    names.extend(target.dependencies or [])
    containers = []
    for name in dict.fromkeys(names):
        identity = await docker_client.get_container_identity(name)
        state = await docker_client.get_container_state(identity)
        if not identity or state is None:
            raise ValueError("Configured workload is unavailable")
        containers.append({"name": name, "id": identity, "state": state})
    unique_containers: dict[str, dict[str, str]] = {}
    for container in containers:
        unique_containers.setdefault(container["id"], container)
    workloads = list(unique_containers.values())
    layout = (backup.backup_metadata or {}).get("archive_layout", {})
    impact = {
        "archive": {
            "id": backup.id,
            "target_id": backup.target_id,
            "file_path": backup.file_path,
            "checksum": backup.checksum,
            "encrypted": bool(backup.encrypted),
            "key_path": backup.encryption_key_path,
            "layout": layout,
            "status": backup.status.value,
        },
        "target": target_identity(target),
        "destination": destination,
        "destination_identity": path_identity(path),
        "volume_identity": {
            "name": volume.name,
            "driver": volume.driver,
            "created_at": volume.created_at,
        }
        if volume
        else None,
        "containers": containers,
        "containers_to_stop": [c["name"] for c in workloads if c["state"] == "running"],
        "containers_to_restart": [
            c["name"] for c in reversed(workloads) if c["state"] == "running"
        ],
        "layout": "Contents of the single archive root merge into the destination"
        if layout.get("kind") == "single"
        or (not layout and target.target_type in ("path", "volume"))
        else "Archive roots are preserved under the destination",
        "overwrite": "Colliding files are overwritten; unrelated files remain. No transactional rollback or application-health guarantee.",
        "key_required": bool(backup.encrypted),
    }
    impact["fingerprint"] = hashlib.sha256(
        json.dumps(impact, sort_keys=True).encode()
    ).hexdigest()
    return impact


def public_impact(impact: dict) -> dict:
    return {
        key: impact[key]
        for key in (
            "fingerprint",
            "destination",
            "containers_to_stop",
            "containers_to_restart",
            "layout",
            "overwrite",
            "key_required",
        )
    }
