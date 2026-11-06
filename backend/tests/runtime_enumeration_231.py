"""Deterministic real-daemon list/inspect race, using only registered owned objects.

Run with the project's backend environment:
  python tests/runtime_enumeration_231.py IMAGE /absolute/evidence.json
No production container is stopped or changed. Created subjects use AutoRemove.
"""

import asyncio
import json
from pathlib import Path
import sys
import uuid

import docker
from docker.errors import NotFound

from app.docker_client import DockerClientWrapper


def main(image, output):
    client = docker.from_env()
    registry = []
    volumes = set()
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    labels = {"dockervault.fixture": "dv231", "dockervault.task": "t_4cb347a3"}
    original_list = client.api.containers
    removed_between_calls = False
    try:
        for role in ("victim", "survivor"):
            item = client.containers.create(
                image,
                command=["infinity"],
                entrypoint=["sleep"],
                auto_remove=True,
                name="dv231-enumeration-" + uuid.uuid4().hex[:12],
                labels=labels,
                network_mode="none",
            )
            registry.append(item.id)
            item.reload()
            volumes.update(
                mount["Name"]
                for mount in item.attrs.get("Mounts", [])
                if mount["Type"] == "volume"
            )
            output.write_text(
                json.dumps(
                    {
                        "registered_ids": registry,
                        "registered_anonymous_volumes": sorted(volumes),
                        "labels": labels,
                        "status": "running",
                    },
                    indent=2,
                )
            )
        victim, survivor = registry

        def listed_then_removed(*args, **kwargs):
            nonlocal removed_between_calls
            listed = original_list(*args, **kwargs)
            assert victim in {item["Id"] for item in listed}
            item = client.containers.get(victim)
            assert item.id == victim and all(
                item.labels.get(key) == value for key, value in labels.items()
            )
            item.remove(v=True)
            removed_between_calls = True
            return listed

        client.api.containers = listed_then_removed
        wrapper = DockerClientWrapper()
        wrapper._client = client
        rows = asyncio.run(wrapper.list_containers())
        assert removed_between_calls and survivor in {row.id for row in rows}
        assert victim not in {row.id for row in rows}
        try:
            asyncio.run(wrapper.get_container_identity(victim))
        except NotFound:
            pass
        else:
            raise AssertionError(
                "Explicitly requested removed identity must fail closed"
            )
        output.write_text(
            json.dumps(
                {
                    "registered_ids": registry,
                    "labels": labels,
                    "status": "passed",
                    "real_daemon_removed_between_list_inspect": True,
                    "survivor_retained": True,
                    "explicit_missing_identity_rejected": True,
                    "sdk_version": docker.__version__,
                },
                indent=2,
            )
        )
    finally:
        client.api.containers = original_list
        for identifier in registry:
            try:
                item = client.containers.get(identifier)
            except NotFound:
                continue
            assert item.id == identifier and all(
                item.labels.get(key) == value for key, value in labels.items()
            )
            item.remove(v=True)
            try:
                client.containers.get(identifier)
            except NotFound:
                pass
            else:
                raise AssertionError("Owned fixture cleanup failed")
        for name in volumes:
            try:
                client.volumes.get(name)
            except NotFound:
                pass
            else:
                raise AssertionError("Owned anonymous volume survived cleanup")
        record = json.loads(output.read_text())
        record.update(
            registered_anonymous_volumes=sorted(volumes), owned_cleanup_verified=True
        )
        output.write_text(json.dumps(record, indent=2))
        client.close()


if __name__ == "__main__":
    main(*sys.argv[1:])
