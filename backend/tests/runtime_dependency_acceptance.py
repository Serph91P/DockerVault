"""Owned native dependency and process-loss acceptance for R8, without host networking."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

import docker


def main():
    client = docker.from_env()
    probe = Path(__file__).with_name("runtime_dependency_recovery.py")
    label = {"dockervault.task": "t_539de28b"}
    for kind in ("backup", "restore"):
        for version in ("ranked", "legacy"):
            workloads, volume = [], None
            try:
                name = "dv-t539-r8-" + uuid.uuid4().hex[:10]
                volume = client.volumes.create(name, labels=label)
                mounts = {volume.name: {"bind": "/owned", "mode": "rw"}}
                db = client.containers.create(
                    "node:24-alpine",
                    name=name + "-db",
                    labels=label,
                    network_disabled=True,
                    volumes=mounts,
                    command=[
                        "node",
                        "-e",
                        "const fs=require('fs');process.on('SIGTERM',()=>{fs.unlinkSync('/owned/ready');process.exit(0)});fs.writeFileSync('/owned/ready','ready');setInterval(()=>{},1000)",
                    ],
                )
                workloads.append(db)
                db.start()
                result = db.exec_run(
                    [
                        "sh",
                        "-c",
                        "for i in $(seq 1 200); do test -f /owned/ready && exit 0; sleep 0.025; done; exit 1",
                    ]
                )
                assert result.exit_code == 0
                app = client.containers.create(
                    "node:24-alpine",
                    name=name + "-app",
                    labels=label,
                    network_disabled=True,
                    volumes=mounts,
                    command=[
                        "node",
                        "-e",
                        "process.on('SIGTERM',()=>process.exit(0));if(!require('fs').existsSync('/owned/ready')){console.error('dependency-unavailable');process.exit(7)};console.log('dependency-ready');setInterval(()=>{},1000)",
                    ],
                )
                workloads.append(app)
                app.start()
                deadline = time.monotonic() + 20
                while b"dependency-ready" not in app.logs():
                    app.reload()
                    assert app.status == "running", app.logs().decode()
                    assert time.monotonic() < deadline
                    time.sleep(0.05)
                pre = client.containers.create(
                    "node:24-alpine",
                    name=name + "-pre",
                    labels=label,
                    network_disabled=True,
                    command=[
                        "sh",
                        "-c",
                        "trap 'exit 0' TERM; while :; do sleep 0.1; done",
                    ],
                )
                workloads.append(pre)
                pre.start()
                pre.stop(timeout=10)
                identities = {"app": app.id, "db": db.id, "pre-stopped": pre.id}
                env = {**os.environ, "OWNED_DEPENDENCY_IDS": json.dumps(identities)}
                with tempfile.TemporaryDirectory(prefix="dv-r8-") as directory:

                    def run(mode):
                        return subprocess.run(
                            [
                                sys.executable,
                                str(probe),
                                directory,
                                mode,
                                kind,
                                version,
                            ],
                            env=env,
                            text=True,
                            capture_output=True,
                            timeout=45,
                        )

                    crashed = run("crash")
                    assert crashed.returncode == 81, crashed.stdout + crashed.stderr
                    first = run("recover")
                    assert first.returncode == 0, first.stdout + first.stderr
                    repeated = run("recover")
                    assert repeated.returncode == 0, repeated.stdout + repeated.stderr
                    assert json.loads(
                        first.stdout.strip().splitlines()[-1]
                    ) == json.loads(repeated.stdout.strip().splitlines()[-1])
                print(
                    f"PASS R8 {kind}/{version}: fresh process after real stop; native consumer requires dependency readiness; pre-stopped preserved; replay unchanged",
                    flush=True,
                )
            finally:
                for workload in reversed(workloads):
                    workload.reload()
                    assert workload.labels.get("dockervault.task") == "t_539de28b"
                    workload.remove(force=True)
                    try:
                        client.containers.get(workload.id)
                        raise AssertionError("Owned fixture remains")
                    except docker.errors.NotFound:
                        pass
                if volume:
                    volume.reload()
                    assert (
                        volume.attrs["Labels"].get("dockervault.task") == "t_539de28b"
                    )
                    volume.remove()
    client.close()
    print("PASS owned workloads/test volumes removed; no production workload touched")


if __name__ == "__main__":
    main()
