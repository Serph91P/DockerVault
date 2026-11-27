#!/usr/bin/env bash
# Caller supplies the already completed, synthetic Python LOCAL backup from #143.
set -euo pipefail
[[ $# == 7 && ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || exit 2
source_app=$1 python_image=$2 rust_image=$3 key=$4 backup=$5 python_ref=$6 rust_ref=$7
[[ "$key" == *.tar.gz.enc && "$key" != /* && "/$key/" != */../* && "/$key/" != */./* ]]
platform=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$python_image")
[[ $(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$python_image") == "$python_ref" ]]
[[ $(docker image inspect -f '{{.Os}}/{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}' "$rust_image") == "$platform $rust_ref" ]]
printf 'R14 shadow: %s Python %s (%s), Rust test build %s (%s)\n' "$platform" \
    "$python_ref" "$(docker image inspect -f '{{.Id}}' "$python_image")" \
    "$rust_ref" "$(docker image inspect -f '{{.Id}}' "$rust_image")"

work=$(mktemp -d "${RUNNER_TEMP:?}/dockervault-shadow.XXXXXX")
cleanup() {
    local status=$?
    rm -r -- "$work" || return 1
    printf 'R14 shadow: snapshot and comparison scratch removed\n'
    return "$status"
}
trap cleanup EXIT
mkdir "$work/snapshot"
# SQLite online backup takes a consistent page snapshot, including committed WAL
# state. Only the disposable destination is switched to rollback journal mode.
docker exec -i -u 1000:1000 "$source_app" python - "$key" <<'PY' |
import contextlib
import shutil
import sqlite3
import sys
import tarfile
import tempfile
from pathlib import Path

key = sys.argv[1]
with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    data = root / "data"
    data.mkdir()
    with contextlib.closing(sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True)) as source:
        with contextlib.closing(sqlite3.connect(data / "backup.db")) as copy:
            source.backup(copy)
            assert copy.execute("PRAGMA journal_mode=DELETE").fetchone() == ("delete",)
    for name in (".credential_key", "ssh_keys", "ssh_known_hosts"):
        path = Path("/app/data") / name
        if path.is_dir():
            shutil.copytree(path, data / name)
        elif path.is_file():
            shutil.copyfile(path, data / name)
    shutil.copyfile("/tmp/recovery-identity", root / "identity")
    for name in (key, key.removesuffix(".enc") + ".key"):
        dest = root / "archives" / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(Path("/app/data/recovery-local") / name, dest)
    with tarfile.open(fileobj=sys.stdout.buffer, mode="w|") as out:
        for entry in root.iterdir():
            out.add(entry, arcname=entry.name)
PY
    tar --extract --no-same-owner -C "$work/snapshot"
# The runner owns this tree under a 0700 parent; both readers mount it read-only.
chmod -R a+rX "$work/snapshot"
inventory() {
    (
        cd "$work/snapshot"
        find . -printf '%y %p\n' | LC_ALL=C sort
        find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum
    )
}
inventory > "$work/before"
mount="type=bind,src=$work/snapshot,dst=/shadow,readonly"
isolation=(--rm --pull=never --network none --no-healthcheck --cap-drop ALL
    --security-opt no-new-privileges --user 1000:1000 --mount "$mount")
docker run -i "${isolation[@]}" --read-only --tmpfs /tmp:rw,nosuid,nodev,size=16m \
    -e 'DATABASE_URL=sqlite+aiosqlite:///file:/shadow/data/backup.db?mode=ro&uri=true' \
    --entrypoint python "$python_image" - "$backup" > "$work/python.json" <<'PY'
import asyncio
import contextlib
import json
import sqlite3
import sys
import tarfile
import tempfile
from pathlib import Path

from app.api.backups import _scan_archive_entries, get_backup
from app.database import engine
from app.encryption import decrypt_backup, key_sidecar_path

async def read():
    root = Path("/shadow")
    assert not Path("/var/run/docker.sock").exists()
    with contextlib.closing(sqlite3.connect("file:/shadow/data/backup.db?mode=ro", uri=True)) as db:
        assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []
        assert db.execute("PRAGMA journal_mode").fetchone() == ("delete",)
    catalog = (await get_backup(int(sys.argv[1]))).model_dump()
    await engine.dispose()
    copy, = catalog["remote_sync_details"]
    assert catalog["status"] == copy["status"] == "completed"
    assert catalog["target_name"] == "Recovery plan" and catalog["encrypted"]
    archive = root / "archives" / copy["remote_path"]
    with tempfile.TemporaryDirectory() as tmp:
        plain = Path(tmp) / "archive.tar.gz"
        await decrypt_backup(archive, Path(key_sidecar_path(str(archive))), (root / "identity").read_text(), plain)
        entries, total = _scan_archive_entries(str(plain), "r:gz", 16, 0)
        assert 1 <= total == len(entries)
        with tarfile.open(plain, "r:gz") as tar:
            members = [[e.path, e.is_dir, e.size, None if e.is_dir else list(tar.extractfile(e.path).read())] for e in entries]
    payload, = [m for m in members if not m[1]]
    assert payload[0].split("/")[-1] == "payload"
    assert bytes(payload[3]) == b"DockerVault143 retained archive bytes\0\xff\n"
    fields = "id target_id target_name backup_type status file_path file_size checksum encrypted remote_synced"
    print(json.dumps(dict(catalog=[catalog[k] for k in fields.split()],
                          copy=[copy[k] for k in ("storage_id", "status", "remote_path")], members=members)))

asyncio.run(read())
PY
docker run "${isolation[@]}" --cpus 2 --memory 6g -e CARGO_BUILD_JOBS=2 \
    "$rust_image" cargo test -p dockervault-service --lib --locked --offline \
    http::backups::browse::shadow_test::copied_python_catalog_and_archive \
    -- --exact --ignored --nocapture > "$work/rust.log" || {
    cat "$work/rust.log" >&2
    exit 1
}
grep '^shadow=' "$work/rust.log" | cut -d= -f2- > "$work/rust.json"
jq -e -s 'length == 2 and .[0] == .[1]' "$work/python.json" "$work/rust.json" >/dev/null
inventory > "$work/after"
cmp "$work/before" "$work/after"
printf 'R14 shadow: catalog + LOCAL sync, complete member list and payload bytes match; inventory and all file SHA-256 unchanged (including DB/sidecars and keys)\n'
sha256sum "$work/before" "$work/after"
