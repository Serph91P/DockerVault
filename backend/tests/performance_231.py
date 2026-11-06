"""Owned, reproducible real-image HTTP backup/restore measurements for issue #231.

Run from the repository root: python3 backend/tests/performance_231.py IMAGE OUTPUT
No production paths, global cache flushes or shared Docker resources are used.
OUTPUT is outside the repository; each JSONL row is durable before the next run.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import threading
import time
import uuid


CASES = [
    ("large-compressible", False, False, 1),
    ("large-random", False, False, 1),
    ("large-compressible", True, True, 1),
    ("large-random", True, True, 1),
    ("small", False, False, 1),
    ("small", True, True, 1),
    ("large-random", False, True, 2),
    ("large-compressible", True, False, 4),
    ("small", False, True, 2),
    ("small", True, False, 4),
]


def command(*args, **kwargs):
    return subprocess.run(
        args, check=True, text=True, capture_output=True, **kwargs
    ).stdout


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def counters():
    root = Path("/sys/fs/cgroup")
    data = {}
    for name in ("cpu.stat", "io.stat", "memory.peak", "memory.current"):
        try:
            data[name] = (root / name).read_text()
        except OSError:
            data[name] = None
    processes = {}
    for path in Path("/proc").glob("[0-9]*/cmdline"):
        try:
            if b"app.main:app" in path.read_bytes():
                fields = {}
                for field in ("io", "stat", "status"):
                    try:
                        fields[field] = (path.parent / field).read_text()
                    except OSError as exc:
                        fields[field] = {"unavailable_errno": exc.errno}
                processes[path.parent.name] = fields
        except OSError:
            pass
    return {"cgroup": data, "backend_processes": processes}


def verify_manifests(roots, manifests):
    for root, manifest in zip(roots, manifests):
        assert set(path.name for path in root.iterdir()) == set(manifest)
        for name, expected in manifest.items():
            path = root / name
            if "link" in expected:
                assert path.is_symlink() and os.readlink(path) == expected["link"]
            else:
                assert digest(path) == expected["sha256"], name
                assert path.stat().st_size == expected["size"], name
                assert path.stat().st_mode & 0o777 == expected["mode"], name
                assert path.stat().st_mtime == expected["mtime"], name


def probe(case, output, quick=False):
    import requests

    assert os.environ.get("DV231_OWNED") == "1"
    dataset, compression, encrypted, jobs = case
    client = requests.Session()
    base = "http://127.0.0.1"
    workloads = json.loads(os.environ.get("DV231_WORKLOADS", "{}"))
    workload_samples = []
    phase = "fixture"
    stop_sampling = threading.Event()
    docker_api = None
    if workloads:
        import docker

        docker_api = docker.from_env()

    def verify_owned():
        if docker_api:
            for identifier in workloads.values():
                item = docker_api.containers.get(identifier)
                assert (
                    item.id == identifier
                    and item.labels.get("dockervault.fixture") == "dv231"
                )

    def sample_workloads():
        while not stop_sampling.is_set():
            sample = {"time": time.monotonic(), "phase": phase, "containers": {}}
            for name, identifier in workloads.items():
                item = docker_api.containers.get(identifier)
                assert item.labels.get("dockervault.fixture") == "dv231"
                ready = False
                if name in ("app", "dependency"):
                    try:
                        ready = (
                            requests.get(
                                f"http://127.0.0.1:{9123 if name == 'app' else 9124}",
                                timeout=0.1,
                            ).content
                            == b"dv231-ready"
                        )
                    except requests.RequestException:
                        pass
                sample["containers"][name] = {
                    "id": item.id,
                    "running": item.attrs["State"]["Running"],
                    "ready": ready,
                }
            workload_samples.append(sample)
            stop_sampling.wait(0.025)

    verify_owned()
    sampler = (
        threading.Thread(target=sample_workloads, daemon=True) if workloads else None
    )
    if sampler:
        sampler.start()

    def call(method, path, **kwargs):
        response = client.request(method, base + path, timeout=900, **kwargs)
        assert response.status_code == 200, (
            path,
            response.status_code,
            response.text[:300],
        )
        return response

    deadline = time.monotonic() + 90
    while True:
        try:
            if requests.get(base + "/health", timeout=2).status_code == 200:
                break
        except requests.RequestException:
            pass
        assert time.monotonic() < deadline
        time.sleep(0.1)
    password = "dv231-owned-fixture-password"
    call(
        "POST",
        "/api/v1/auth/setup",
        json={"username": "dv231", "password": password, "confirm_password": password},
    )
    call("POST", "/api/v1/auth/login", json={"username": "dv231", "password": password})
    client.headers["Authorization"] = "Bearer " + client.cookies.get("session_token")
    private_key = None
    if encrypted:
        private_key = call("POST", "/api/v1/encryption/setup").json()["private_key"]
        call("POST", "/api/v1/encryption/confirm-setup", json={"confirmed": True})
    s3 = None
    s3_prefix = "macro-" + uuid.uuid4().hex
    storage_options = {
        "name": "dv231-owned-local-remote",
        "storage_type": "local",
        "base_path": "/app/data/dv231-remote",
    }
    if os.environ.get("DV231_S3") == "1":
        import boto3

        s3 = boto3.client(
            "s3",
            endpoint_url="http://127.0.0.1:8333",
            aws_access_key_id="owned",
            aws_secret_access_key="owned",
            region_name="us-east-1",
        )
        try:
            s3.create_bucket(Bucket="dv231-owned")
        except s3.exceptions.BucketAlreadyOwnedByYou:
            pass
        storage_options = {
            "name": "dv231-owned-s3",
            "storage_type": "s3",
            "base_path": s3_prefix,
            "s3_bucket": "dv231-owned",
            "s3_access_key": "owned",
            "s3_secret_key": "owned",
            "s3_endpoint_url": "http://127.0.0.1:8333",
        }
    storage = call("POST", "/api/v1/storage", json=storage_options).json()
    roots, targets, manifests = [], [], []
    for job in range(jobs):
        source = Path(f"/app/data/dv231-source-{job}")
        source.mkdir()
        if dataset == "small":
            rng = random.Random(231)
            for index in range(64 if quick else 25000):
                (source / f"entry-{index:05d}").write_bytes(rng.randbytes(256))
        else:
            rng = random.Random(231)
            with (source / "payload").open("wb") as stream:
                for _ in range(4 if quick else 1024):
                    stream.write(
                        rng.randbytes(1024 * 1024)
                        if dataset == "large-random"
                        else b"dv231-0123456789\n" * 65536
                    )
        (source / "link").symlink_to(next(source.iterdir()).name)
        manifest = {}
        for path in source.iterdir():
            if path.is_symlink():
                manifest[path.name] = {"link": os.readlink(path)}
            else:
                os.chmod(path, 0o640)
                os.utime(path, (1700000000, 1700000000))
                manifest[path.name] = {
                    "sha256": digest(path),
                    "size": path.stat().st_size,
                    "mode": path.stat().st_mode & 0o777,
                    "mtime": path.stat().st_mtime,
                }
            os.chown(path, 1000, 1000, follow_symlinks=False)
        os.chown(source, 1000, 1000)
        roots.append(source)
        manifests.append(manifest)
        targets.append(
            call(
                "POST",
                "/api/v1/targets",
                json={
                    "name": source.name,
                    "target_type": "path",
                    "host_path": str(source),
                    "stop_container": bool(workloads),
                    "container_name": docker_api.containers.get(workloads["app"]).name
                    if workloads
                    else None,
                    "dependencies": [
                        docker_api.containers.get(identifier).name
                        for name, identifier in workloads.items()
                        if name != "app"
                    ],
                    "compression_enabled": compression,
                    "remote_storage_ids": [storage["id"]],
                },
            ).json()
        )
    verify_owned()
    phase = "backup"
    before = counters()
    started = time.monotonic()
    backups = [
        call("POST", "/api/v1/backups", json={"target_id": target["id"]}).json()
        for target in targets
    ]
    health_latencies = []
    while any(
        backup["status"] != "completed" or not backup.get("remote_synced")
        for backup in backups
    ):
        for index, backup in enumerate(backups):
            backups[index] = call("GET", f"/api/v1/backups/{backup['id']}").json()
            assert backups[index]["status"] not in ("failed", "cancelled"), backups[
                index
            ].get("error_message")
        check = time.monotonic()
        call("GET", "/health")
        health_latencies.append(time.monotonic() - check)
        assert time.monotonic() - started < 1800
        time.sleep(0.05)
    backup_seconds = time.monotonic() - started
    after_backup = counters()
    phase = "listing"
    listings = []
    for backup in backups:
        for offset in (0, 2000, 24000):
            start = time.monotonic()
            path = f"/api/v1/backups/{backup['id']}/files?offset={offset}&limit=100"
            response = (
                call("POST", path, json={"private_key": private_key})
                if encrypted
                else call("GET", path)
            )
            listings.append(
                {
                    "offset": offset,
                    "seconds": time.monotonic() - start,
                    "total": response.headers["X-Total-Count"],
                    "returned": len(response.json()),
                }
            )
    downloads = []
    for backup, root, manifest in zip(backups, roots, manifests):
        member_name = sorted(
            name for name, entry in manifest.items() if "sha256" in entry
        )[-1]
        request_path = f"/api/v1/backups/{backup['id']}/files/{root.name}/{member_name}"
        download_before = counters()
        download_started = time.monotonic()

        def download_file():
            response = (
                call(
                    "POST", request_path, json={"private_key": private_key}, stream=True
                )
                if encrypted
                else call("GET", request_path, stream=True)
            )
            with response:
                hasher = hashlib.sha256()
                for chunk in response.iter_content(65536):
                    hasher.update(chunk)
                assert hasher.hexdigest() == manifest[member_name]["sha256"]

        latencies, timeouts = [], 0
        with ThreadPoolExecutor(max_workers=1) as pool:
            pending = pool.submit(download_file)
            while not pending.done():
                check = time.monotonic()
                try:
                    assert requests.get(base + "/health", timeout=1).status_code == 200
                    latencies.append(time.monotonic() - check)
                except requests.Timeout:
                    timeouts += 1
                time.sleep(0.01)
            pending.result()
        downloads.append(
            {
                "seconds": time.monotonic() - download_started,
                "before": download_before,
                "after": counters(),
                "health_seconds": latencies,
                "health_timeouts": timeouts,
                "bytes_verified": manifest[member_name]["size"],
            }
        )
    logs = [
        call("GET", f"/api/v1/backups/{backup['id']}/logs").json() for backup in backups
    ]
    for root in roots:
        for path in root.iterdir():
            if path.is_symlink():
                path.unlink()
            else:
                path.write_bytes(b"source deliberately changed before restore")
    verify_owned()
    phase = "restore"
    before_restore = counters()
    restore_started = time.monotonic()

    def restore(backup):
        result = call(
            "POST",
            f"/api/v1/backups/{backup['id']}/restore",
            json={"private_key": private_key} if encrypted else {},
        ).json()
        assert result["status"] == "restored", result

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        list(pool.map(restore, backups))
    restore_seconds = time.monotonic() - restore_started
    after_restore = counters()
    phase = "verification"
    verify_manifests(roots, manifests)
    remote_restore = None
    if s3 is not None:
        for backup, root in zip(backups, roots):
            call(
                "DELETE",
                f"/api/v1/backups/{backup['id']}",
                params={"delete_local": "true", "delete_remote": "false"},
            )
            assert not Path(backup["file_path"]).exists()
            if encrypted:
                Path(backup["file_path"]).with_suffix(".key").unlink(missing_ok=True)
            for path in root.iterdir():
                if path.is_symlink():
                    path.unlink()
                else:
                    path.write_bytes(b"changed again before remote-only restore")
        phase = "remote_restore"
        remote_before = counters()
        start = time.monotonic()
        with ThreadPoolExecutor(max_workers=jobs) as pool:
            list(pool.map(restore, backups))
        remote_restore = {
            "seconds": time.monotonic() - start,
            "before": remote_before,
            "after": counters(),
        }
        phase = "verification"
        verify_manifests(roots, manifests)
        for page in s3.get_paginator("list_objects_v2").paginate(
            Bucket="dv231-owned", Prefix=s3_prefix
        ):
            objects = [{"Key": item["Key"]} for item in page.get("Contents", [])]
            if objects:
                s3.delete_objects(Bucket="dv231-owned", Delete={"Objects": objects})
        s3.close()
    record = {
        "case": case,
        "quick": quick,
        "backup_seconds": backup_seconds,
        "restore_seconds": restore_seconds,
        "remote_only_restore": remote_restore,
        "runtime_python": sys.version,
        "backup_effect": [
            {"status": backup["status"], "remote_synced": backup["remote_synced"]}
            for backup in backups
        ],
        "before": before,
        "after_backup": after_backup,
        "before_restore": before_restore,
        "after_restore": after_restore,
        "listing": listings,
        "single_file": downloads,
        "measurement_version": 2,
        "backup_duration_scope": "accepted HTTP backup through completed catalog AND verified remote_synced effect",
        "health_max_seconds": max(health_latencies),
        "logs": logs,
        "verified": "bytes/mode/mtime/symlink",
        "workload_interruption": "not applicable: path target without workload; measured separately",
        "cache": "fresh owned paths; host page cache not flushed; fixture generation and byte hashes warm source",
        "product_job_capacity": 2,
    }
    stop_sampling.set()
    if sampler:
        sampler.join(timeout=10)
        assert not sampler.is_alive() and workload_samples
        assert all(
            not sample["containers"]["initially-stopped"]["running"]
            for sample in workload_samples
        )
        assert all(
            docker_api.containers.get(workloads[name]).attrs["State"]["Running"]
            for name in ("app", "dependency")
        )
    record["workload_samples"] = workload_samples
    record["memory_peak_scope"] = (
        "whole disposable cgroup lifetime, includes fixture generation, page cache and probe; not phase RSS or a peak delta"
    )
    Path(output).write_text(json.dumps(record) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("output", type=Path)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--case", type=int, action="append")
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--workloads", action="store_true")
    parser.add_argument("--s3-container")
    parser.add_argument("--phase-suite", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    image_id = command(
        "docker",
        "image",
        "inspect",
        args.image,
        "--format",
        "{{.Id}} {{.Config.Labels}}",
    )
    (args.output / "environment.json").write_text(
        json.dumps(
            {
                "image": args.image,
                "image_identity": image_id,
                "docker": command("docker", "version"),
                "host_load": Path("/proc/loadavg").read_text(),
                "host_mem": Path("/proc/meminfo").read_text(),
                "limits": {"cpus": 2, "memory": "6g"},
                "python": sys.version,
            },
            indent=2,
        )
    )
    for index in args.case if args.case is not None else range(len(CASES)):
        for repeat in range(args.repeats):
            name = "dv231-bench-" + uuid.uuid4().hex[:12]
            started = False
            workload_ids = {}
            try:
                mounts = (
                    ["-v", "/var/run/docker.sock:/var/run/docker.sock"]
                    if args.workloads
                    else []
                )
                command(
                    "docker",
                    "run",
                    "--rm",
                    "-d",
                    "--name",
                    name,
                    "--network",
                    "container:" + args.s3_container if args.s3_container else "none",
                    "--cpus",
                    "2",
                    "--memory",
                    "6g",
                    "--label",
                    "dockervault.fixture=dv231",
                    "-e",
                    "DV231_OWNED=1",
                    *mounts,
                    args.image,
                )
                started = True
                if args.workloads:
                    assert CASES[index][3] == 1, (
                        "Independent parallel jobs are measured separately from the shared-workload case"
                    )
                    for kind, port in (
                        ("dependency", 9124),
                        ("app", 9123),
                        ("initially-stopped", 9125),
                    ):
                        dependency = (
                            "import urllib.request;urllib.request.urlopen('http://127.0.0.1:9124',timeout=5).read();"
                            if kind == "app"
                            else ""
                        )
                        script = (
                            "import signal,sys;signal.signal(signal.SIGTERM,lambda *args:sys.exit(0));"
                            + dependency
                            + "from http.server import BaseHTTPRequestHandler,HTTPServer;"
                            + "\nclass Handler(BaseHTTPRequestHandler):\n def do_GET(self):\n  self.send_response(200);self.end_headers();self.wfile.write(b'dv231-ready')\n def log_message(self,*args): pass\n"
                            + f"HTTPServer(('127.0.0.1',{port}),Handler).serve_forever()"
                        )
                        dependency_labels = (
                            ["--label", "backup.depends_on=" + name + "-dependency"]
                            if kind == "app"
                            else []
                        )
                        identifier = command(
                            "docker",
                            "create",
                            "--name",
                            name + "-" + kind,
                            "--network",
                            "container:" + name,
                            "--label",
                            "dockervault.fixture=dv231",
                            "--label",
                            "dockervault.task=t_4cb347a3",
                            *dependency_labels,
                            "--entrypoint",
                            "python",
                            args.image,
                            "-c",
                            script,
                        ).strip()
                        workload_ids[kind] = identifier
                        (args.output / (name + "-register.json")).write_text(
                            json.dumps(workload_ids, indent=2)
                        )
                        if kind != "initially-stopped":
                            assert (
                                json.loads(command("docker", "inspect", identifier))[0][
                                    "Config"
                                ]["Labels"]["dockervault.fixture"]
                                == "dv231"
                            )
                            command("docker", "start", identifier)
                            for _ in range(100):
                                ready = subprocess.run(
                                    [
                                        "docker",
                                        "exec",
                                        name,
                                        "python",
                                        "-c",
                                        f"import urllib.request;assert urllib.request.urlopen('http://127.0.0.1:{port}',timeout=.2).read()==b'dv231-ready'",
                                    ],
                                    capture_output=True,
                                )
                                if ready.returncode == 0:
                                    break
                                time.sleep(0.05)
                            assert ready.returncode == 0
                command(
                    "docker",
                    "cp",
                    str(Path(__file__).resolve()),
                    name + ":/app/dv231-probe.py",
                )
                probe_args = [
                    "/app/dv231-probe.py",
                    "--probe",
                    json.dumps(CASES[index]),
                    "/app/data/result.json",
                    *(["--quick"] if args.quick else []),
                ]
                if args.phase_suite:
                    assert args.workloads and not args.s3_container
                    command("docker", "exec", name, "supervisorctl", "stop", "backend")
                    command(
                        "docker",
                        "cp",
                        str(Path(__file__).with_name("runtime_phases_231.py")),
                        name + ":/app/runtime_phases_231.py",
                    )
                    probe_args = [
                        "/app/runtime_phases_231.py",
                        "--controller",
                        "/app/data/result.json",
                    ]
                command(
                    "docker",
                    "exec",
                    "--user",
                    "dockervault",
                    "-e",
                    "DV231_WORKLOADS=" + json.dumps(workload_ids),
                    "-e",
                    "DV231_S3=" + ("1" if args.s3_container else "0"),
                    name,
                    "python",
                    *probe_args,
                    timeout=3600,
                )
                destination = args.output / f"case-{index}-repeat-{repeat}.json"
                command(
                    "docker", "cp", name + ":/app/data/result.json", str(destination)
                )
                print(f"PASS case={index} repeat={repeat} -> {destination}", flush=True)
            except BaseException as exc:
                if isinstance(exc, subprocess.CalledProcessError):
                    (
                        args.output / f"case-{index}-repeat-{repeat}-probe-failure.log"
                    ).write_text((exc.stdout or "") + (exc.stderr or ""))
                if started:
                    (
                        args.output / f"case-{index}-repeat-{repeat}-failure.log"
                    ).write_text(command("docker", "logs", name))
                raise
            finally:
                if started and args.phase_suite:
                    log_data = command(
                        "docker",
                        "exec",
                        name,
                        "python",
                        "-c",
                        "from pathlib import Path;import json;print(json.dumps({p.name:p.read_text() for p in Path('/app/data/dv231-phases').glob('server-*.log')}))",
                    )
                    (
                        args.output / f"case-{index}-repeat-{repeat}-phase-logs.json"
                    ).write_text(log_data)
                for identifier in reversed(list(workload_ids.values())):
                    item = json.loads(command("docker", "inspect", identifier))[0]
                    assert (
                        item["Id"] == identifier
                        and item["Config"]["Labels"]["dockervault.fixture"] == "dv231"
                    )
                    command("docker", "rm", "--force", "--volumes", identifier)
                    assert (
                        subprocess.run(
                            ["docker", "inspect", identifier], capture_output=True
                        ).returncode
                        != 0
                    )
                if started:
                    command("docker", "stop", "--time", "15", name)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--probe"]:
        probe(json.loads(sys.argv[2]), sys.argv[3], "--quick" in sys.argv)
    else:
        main()
