"""Owned #186 acceptance fixture; real app, isolated Docker, no product doubles."""

from pathlib import Path
import subprocess
import uvicorn
from app.main import app
from app.docker_client import docker_client
from app.database import async_session, Backup, BackupTarget
from app.encryption import generate_key_pair

ROOT = Path("/backups/restore186")
PAYLOAD = {
    "nested/file.txt": b"owned nested restore bytes\n",
    "second.bin": bytes(range(256)),
}
BUSYBOX = "sha256:c6348fa86ba0fb2108c9334f5fe913ddc6d853313e655891f133a0127c30099f"


def content(path):
    base = Path(path)
    return {
        name: (base / name).is_file() and (base / name).read_bytes() == data
        for name, data in PAYLOAD.items()
    }


@app.post("/api/v1/test-restore186/seed")
async def seed():
    (ROOT / "offsite").mkdir(parents=True, exist_ok=True)
    client = docker_client.client
    for name in ("restore186-original", "restore186-alternative"):
        client.volumes.create(name)
    mount = Path(client.volumes.get("restore186-original").attrs["Mountpoint"])
    for base in (ROOT / "source", mount):
        for name, data in PAYLOAD.items():
            (base / name).parent.mkdir(parents=True, exist_ok=True)
            (base / name).write_bytes(data)
    for name in ("restore186-running", "restore186-stopped"):
        child = client.containers.run(
            BUSYBOX,
            ["sh", "-c", "trap 'exit 0' TERM; while :; do sleep 1 & wait $!; done"],
            name=name,
            network_mode="none",
            detach=True,
            labels={"dockervault.test": "restore186-owned"},
        )
        if name.endswith("stopped"):
            child.stop(timeout=1)
    return {
        "source": str(ROOT / "source"),
        "original_volume": "restore186-original",
        "alternative_volume": "restore186-alternative",
        "dependencies": ["restore186-running", "restore186-stopped"],
    }


@app.post("/api/v1/test-restore186/wrong-key")
async def wrong_key():
    return {"private_key": (await generate_key_pair()).private_key}


@app.get("/api/v1/test-restore186/proof")
async def proof():
    client = docker_client.client
    states = {}
    for name in ("restore186-running", "restore186-stopped"):
        child = client.containers.get(name)
        states[name] = {
            key: child.attrs["State"][key]
            for key in ("Status", "StartedAt", "FinishedAt")
        }
    mounts = {
        name: client.volumes.get(name).attrs["Mountpoint"]
        for name in ("restore186-original", "restore186-alternative")
    }
    paths = {
        "source": ROOT / "source",
        "custom": ROOT / "custom",
        "encrypted": ROOT / "encrypted",
        "remote": ROOT / "remote",
        "unknown": ROOT / "unknown",
        "stale": ROOT / "stale",
        **mounts,
    }
    return {
        "states": states,
        "bytes": {name: content(path) for name, path in paths.items()},
        "mounts": mounts,
        "exists": {name: Path(path).exists() for name, path in paths.items()},
    }


@app.post("/api/v1/test-restore186/change")
async def change(data: dict):
    # Only fixed owned synthetic resources may be mutated by this test server.
    if data["action"] == "mutate":
        for base in (
            ROOT / "source",
            Path(
                docker_client.client.volumes.get("restore186-original").attrs[
                    "Mountpoint"
                ]
            ),
        ):
            (base / "nested/file.txt").write_bytes(b"changed after backup")
            (base / "unrelated").write_bytes(b"keep unrelated bytes")
    elif data["action"] == "remote-only":
        async with async_session() as db:
            backup = await db.get(Backup, data["id"])
            if backup.file_path:
                Path(backup.file_path).unlink(missing_ok=True)
            if backup.encryption_key_path:
                Path(backup.encryption_key_path).unlink(missing_ok=True)
    elif data["action"] == "unsupported":
        async with async_session() as db:
            backup = await db.get(Backup, data["id"])
            target = await db.get(BackupTarget, backup.target_id)
            target.target_type = data["kind"]
            target.stop_container = False
            target.dependencies = []
            await db.commit()
    elif data["action"] == "remove-sidecar":
        for path in (ROOT / "offsite").rglob("*.key"):
            path.unlink()
    elif data["action"] == "cleanup":
        for name in ("restore186-running", "restore186-stopped"):
            docker_client.client.containers.get(name).remove(force=True)
        for name in ("restore186-original", "restore186-alternative"):
            docker_client.client.volumes.get(name).remove()
        assert not docker_client.client.containers.list(all=True)
        assert not docker_client.client.volumes.list()
    return {"ok": True}


subprocess.Popen(["nginx", "-g", "daemon off;"])
uvicorn.run(app, host="127.0.0.1", port=8000, access_log=False, log_level="warning")
