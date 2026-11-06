"""Component time/I/O attribution in an owned product image (baseline/candidate).

Copy into a disposable image container sharing only the network namespace of the
owned SeaweedFS service. Set DV231_OWNED=1. The final argument is an output path.
Includes native cProfile statistics, rusage(self+children), /proc logical I/O and
cgroup physical I/O/peak. No measurement is a throughput or outage claim.
"""

import asyncio
import cProfile
import hashlib
import io
import json
import os
from pathlib import Path
import pstats
import random
import resource
import sys
import tarfile
import time


async def main(output):
    assert os.environ.get("DV231_OWNED") == "1"
    from app.api.backups import _list_archive_entries
    from app.backup_engine import BackupEngine
    from app.encryption import encrypt_backup, decrypt_backup, generate_key_pair
    from app.remote_storage import (
        StorageConfig,
        StorageType,
        S3Storage,
        LocalStorage,
        RemoteStorageManager,
    )

    root = Path("/app/data/dv231-profile")
    root.mkdir()
    payload = root / "large"
    rng = random.Random(231)
    with payload.open("wb") as stream:
        for _ in range(1024):
            stream.write(rng.randbytes(1024 * 1024))
    archive = root / "small.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        for index in range(25000):
            member = tarfile.TarInfo(f"owned/entry-{index:05d}")
            member.size = 32
            tar.addfile(member, io.BytesIO(b"x" * 32))
    rows = []

    def snapshot():
        result = {}
        for kind, which in (
            ("self", resource.RUSAGE_SELF),
            ("children", resource.RUSAGE_CHILDREN),
        ):
            usage = resource.getrusage(which)
            result[kind] = {
                name: getattr(usage, name)
                for name in (
                    "ru_utime",
                    "ru_stime",
                    "ru_maxrss",
                    "ru_inblock",
                    "ru_oublock",
                )
            }
        for path in (
            "/proc/self/io",
            "/sys/fs/cgroup/io.stat",
            "/sys/fs/cgroup/memory.peak",
            "/sys/fs/cgroup/cpu.stat",
        ):
            try:
                result[path] = Path(path).read_text()
            except OSError:
                result[path] = None
        return result

    async def measure(name, coroutine):
        before = snapshot()
        profile = cProfile.Profile()
        started = time.monotonic()
        profile.enable()
        value = await coroutine
        profile.disable()
        elapsed = time.monotonic() - started
        summary = io.StringIO()
        pstats.Stats(profile, stream=summary).sort_stats("cumulative").print_stats(25)
        rows.append(
            {
                "operation": name,
                "wall_seconds": elapsed,
                "before": before,
                "after": snapshot(),
                "profile": summary.getvalue(),
            }
        )
        Path(output).write_text(
            json.dumps(
                {
                    "python": sys.version,
                    "payload_bytes": payload.stat().st_size,
                    "small_members": 25000,
                    "rows": rows,
                },
                indent=2,
            )
        )
        return value

    engine = BackupEngine()
    keys = await generate_key_pair()
    config = StorageConfig(
        id=1,
        name="dv231-seaweed",
        storage_type=StorageType.S3,
        s3_bucket="dv231-owned",
        s3_access_key="owned",
        s3_secret_key="owned",
        s3_endpoint_url="http://127.0.0.1:8333",
        base_path="profile",
    )
    storage = S3Storage(config)
    try:
        client = await storage._get_client()
        try:
            await client.create_bucket(Bucket="dv231-owned")
        except client.exceptions.BucketAlreadyOwnedByYou:
            pass
        assert (await storage.upload(payload, "large"))[0]
        for repeat in range(3):
            await measure(
                f"hash-1GiB-{repeat}", engine._calculate_checksum(str(payload))
            )
            await measure(
                f"s3-download-1GiB-{repeat}",
                storage.download("large", root / "download"),
            )
            with payload.open("rb") as left, (root / "download").open("rb") as right:
                assert (
                    hashlib.file_digest(left, "sha256").digest()
                    == hashlib.file_digest(right, "sha256").digest()
                )
            (root / "download").unlink()
            manager = RemoteStorageManager()
            manager.backends[1] = storage
            manager.configs[1] = config
            local_config = StorageConfig(
                id=2,
                name="dv231-local",
                storage_type=StorageType.LOCAL,
                base_path=str(root / "remote"),
            )
            manager.backends[2] = LocalStorage(local_config)
            manager.configs[2] = local_config
            result = await measure(
                f"upload-verify-two-targets-1GiB-{repeat}",
                manager.upload_to_all(payload, "verified"),
            )
            assert all(success for success, error in result.values()), result
            for offset in (0, 2000, 24000):
                entries, total = await measure(
                    f"list-25000-{repeat}-{offset}",
                    _list_archive_entries(str(archive), "r:gz", 2000, offset),
                )
                assert total == 25000 and len(entries) == min(2000, total - offset)
            encrypted_source = root / "encrypt.tar"
            # Copy is fixture preparation and deliberately outside measured encryption.
            import shutil

            shutil.copyfile(payload, encrypted_source)
            result = await measure(
                f"encrypt-1GiB-{repeat}",
                encrypt_backup(encrypted_source, keys.public_key),
            )
            restored = await measure(
                f"decrypt-1GiB-{repeat}",
                decrypt_backup(
                    result.encrypted_path, result.key_path, keys.private_key
                ),
            )
            with restored.open("rb") as left, payload.open("rb") as right:
                assert (
                    hashlib.file_digest(left, "sha256").digest()
                    == hashlib.file_digest(right, "sha256").digest()
                )
            for path in (restored, Path(result.encrypted_path), Path(result.key_path)):
                path.unlink(missing_ok=True)
    finally:
        await storage.close()


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1]))
