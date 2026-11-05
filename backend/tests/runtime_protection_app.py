"""Owned #184 browser fixture: real app/SQLite/archive engine, synthetic discovery/lifecycle.

No Docker socket, host mounts, Komodo or remote-provider connection is used. Only
these Docker discovery/lifecycle boundaries are doubled; API/auth/scheduler,
worker/fallback archive filtering, local copies and restore remain production code.
"""

import json
from importlib import import_module
from pathlib import Path
import subprocess
from types import SimpleNamespace

import uvicorn
from app.docker_client import docker_client, ContainerInfo, VolumeInfo, StackInfo

root = Path("/app/data/184-fixtures")


# These are owned container-layer directories, NOT host mounts or real Docker volumes.
def volume_path(name):
    return Path("/var/lib/docker/volumes") / ("dv184-" + name) / "_data"


for name in ("path", "docker-data", "docker-logs", "remote-one", "remote-two"):
    source = volume_path(name) if name.startswith("docker-") else root / name
    (source / "config/cache").mkdir(parents=True, exist_ok=True)
    (source / "config/keep.txt").write_bytes(b"owned nested protection fixture")
    (source / "config/cache/drop.txt").write_bytes(b"excluded cache bytes")
    (source / "outside.txt").write_bytes(b"outside include selection")


def container(name, mounts, state="running"):
    return ContainerInfo(
        id="id-" + name,
        name=name,
        image="synthetic-fixture",
        status=state,
        state=state,
        created="2026-11-05T00:00:00Z",
        labels={},
        mounts=[
            dict(
                type="volume",
                name=n,
                source=str(volume_path(n)),
                destination=d,
                mode="rw",
                rw=True,
            )
            for n, d in mounts
        ],
        networks=[],
    )


containers = [
    container(
        "service",
        [
            ("docker-data", "/var/lib/app"),
            ("docker-data", "/second"),
            ("docker-logs", "/logs"),
        ],
    ),
    container("stack-db", [("docker-data", "/data")]),
    container("dependency", [], "running"),
    container("originally-stopped", [], "exited"),
]
registry = {alias: c for c in containers for alias in (c.id, c.name)}
events = []


class SDKContainer:
    def __init__(self, info):
        self.info = info
        self.id = info.id

    @property
    def status(self):
        return self.info.status

    def stop(self, timeout=30):
        self.info.status = self.info.state = "exited"
        events.append(["stop", self.info.name])
        (root / "lifecycle.json").write_text(json.dumps(events))

    def start(self):
        self.info.status = self.info.state = "running"
        events.append(["start", self.info.name])
        (root / "lifecycle.json").write_text(json.dumps(events))


async def list_containers(all=True):
    return containers


async def list_volumes():
    return [
        VolumeInfo(
            name=name,
            driver="local",
            mountpoint=str(volume_path(name)),
            labels={},
            created_at="2026-11-05T00:00:00Z",
            used_by=["service"],
        )
        for name in ("docker-data", "docker-logs")
    ]


async def stacks():
    return [
        StackInfo(
            name="owned-stack",
            containers=containers[:2],
            volumes=["docker-data", "docker-logs"],
            networks=[],
            stop_order=["service", "stack-db"],
            start_order=["stack-db", "service"],
        )
    ]


async def ping():
    return True


docker_client._client = SimpleNamespace(
    containers=SimpleNamespace(get=lambda key: SDKContainer(registry[key])),
    close=lambda: None,
)
docker_client.list_containers = list_containers
docker_client.list_volumes = list_volumes
docker_client.get_stacks = stacks
docker_client.ping = ping

app = import_module("app.main").app

subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
