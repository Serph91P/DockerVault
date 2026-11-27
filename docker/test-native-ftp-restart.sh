#!/usr/bin/env bash
# Reuse the plain FTP backup for either the restart/restore or the R14 isolation proof.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-ftp-restart.sh APP_IMAGE FTP_FIXTURE_IMAGE [PYTHON_CONTROL_IMAGE]}
fixture=${2:?FTP fixture image required}
control_image=${3:-}
[[ ${RUNNER_ENVIRONMENT:-} == github-hosted && ( $# == 2 ||
    ( $# == 3 && -n "$control_image" && ${GITHUB_ACTIONS:-} == true ) ) ]] || {
    printf 'Run this container test only on the GitHub-hosted CI matrix\n' >&2
    exit 2
}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
arch=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$image")
[[ "$rev" == "$(git rev-parse HEAD)" && ( "$arch" == linux/amd64 || "$arch" == linux/arm64 ) ]]
prefix=/copies
source=/app/data/ftp-restart-source
if [[ -n "$control_image" ]]; then
    case "$(uname -m):$arch" in x86_64:linux/amd64|aarch64:linux/arm64) ;; *) exit 2 ;; esac
    prefix=/canary-copies
    source=/app/data/r14-rust-source
    source docker/native-ftp-canary-control.sh
fi
app= provider= control= volumes= phase=startup
cleanup() {
    local status=$? id volume remaining
    if ((status != 0)); then
        printf 'Plain FTP roundtrip failed at %s (%s, %s)\n' "$phase" "$arch" "$rev" >&2
    fi
    for id in "$app" "$control" "$provider"; do
        [[ -n "$id" ]] || continue
        docker rm -fv "$id" >/dev/null || status=1
        if [[ -n "$control_image" ]]; then
            remaining=$(docker ps -aq --no-trunc --filter "id=$id") && [[ -z "$remaining" ]] || status=1
        fi
    done
    for volume in $volumes; do
        remaining=$(docker volume ls -q --filter "name=$volume") && [[ -z "$remaining" ]] || status=1
    done
    if ((status == 0)); then
        if [[ -n "$control" ]]; then printf 'R14 Canary cleanup: three containers, fixture tmpfs and four anonymous volumes removed\n'
        else printf 'FTP fixture and app containers and app volumes removed\n'; fi
    fi
    exit "$status"
}
trap cleanup EXIT
# Share only the fixture's isolated loopback. No published ports, egress or host mounts.
provider=$(docker create --pull=never --network none --user 1000:1000 \
    --tmpfs /fixture:rw,uid=1000,gid=1000,mode=0700,size=16m "$fixture" python -u -c '
import asyncio
from pathlib import Path
import sys
import aioftp

async def main():
    root = Path("/fixture")
    prefix = sys.argv[1]
    control = root / "python"
    if prefix == "/canary-copies":
        root /= "rust"
        (control / "control-copies").mkdir(parents=True)
        (control / "control-copies/reference").write_bytes(b"pre-existing Python provider object\n")
    (root / prefix[1:]).mkdir(parents=True)
    users = [aioftp.User("fixture", "public-fixture-only", base_path=root)]
    if prefix == "/canary-copies":
        users.append(aioftp.User("control", "public-control-only", base_path=control))
    server = aioftp.Server(users)
    await server.start("127.0.0.1", 32121)
    await asyncio.Event().wait()

asyncio.run(main())
' "$prefix")
docker start "$provider" >/dev/null
app=$(docker create --pull=never --network "container:$provider" --cpus 2 --memory 6g "$image")
docker start "$app" >/dev/null

service=/run/dockervault/services/backend
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "${1:-$app}" curl -fsS --max-time 2 http://127.0.0.1/health 2>/dev/null | \
            jq -e '.status == "healthy"' >/dev/null; then return; fi
        sleep 1
    done
    return 1
}
api() {
    local method=$1 path=$2
    shift 2
    docker exec -i -u 1000:1000 "$app" curl -fsS --max-time 45 -b /tmp/ftp-restart-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
authenticate() {
    printf '%s' "$credentials" | api POST "/auth/$1" -c /tmp/ftp-restart-cookie --data-binary @- >/dev/null
}
ftp_read() {
    # Standalone FTP client reads every byte and checks the terminal reply, outside the app.
    docker exec -i "$provider" python - "$prefix" "${source##*/}" "$@" <<'PY'
from ftplib import FTP
from hashlib import sha256
from io import BytesIO
from pathlib import Path
import sys
import tarfile

prefix, root, *keys = sys.argv[1:]
with FTP() as ftp:
    ftp.connect("127.0.0.1", 32121, timeout=10)
    ftp.login("fixture", "public-fixture-only")
    ftp.cwd(prefix)
    if keys:
        key, = keys
        data = BytesIO()
        ftp.retrbinary("RETR " + key, data.write)
        assert data.tell() > 0
        if prefix == "/canary-copies":
            base = Path("/fixture/rust/canary-copies")
            assert {str(p.relative_to(base)) for p in base.rglob("*")} == {key, key.split("/")[0]}
            data.seek(0)
            with tarfile.open(fileobj=data, mode="r:") as archive:
                directory, content = archive.getmembers()
                assert directory.isdir() and directory.name == root
                assert content.isfile() and content.name == f"{root}/content"
                assert content.mode == 0o640 and content.mtime == 1700000000
                assert archive.extractfile(content).read() == b"\x00plain FTP fixture\xff\n"
        print(sha256(data.getbuffer()).hexdigest(), len(data.getbuffer()))
PY
}
ready
for ((attempt=0; attempt<30; attempt++)); do
    if ftp_read >/dev/null 2>&1; then break; fi
    sleep 1
done
ftp_read
if [[ -n "$control_image" ]]; then prepare_control; fi
credentials=$(jq -nc --arg password "$(tr -d '-' </proc/sys/kernel/random/uuid)" \
    '{username:"ftp-restart",password:$password,confirm_password:$password}')
authenticate setup
storage=$(jq -nc --arg prefix "$prefix" \
    '{name:"FTP fixture",storage_type:"ftp",enabled:true,host:"127.0.0.1",port:32121,username:"fixture",password:"public-fixture-only",base_path:$prefix}' |
    api POST /storage --data-binary @- | jq -er '.id | numbers')
docker exec -u 1000:1000 "$app" sh -ec '
    mkdir "$1"; printf "\000plain FTP fixture\377\n" > "$1/content"
    chmod 640 "$1/content"; touch -d @1700000000 "$1/content"
' sh "$source"

phase=backup-and-sync
plan=$(jq -nc --arg path "$source" --argjson id "$storage" \
    '{name:"FTP restart",target_type:"path",host_path:$path,stop_container:false,compression_enabled:false,remote_storage_ids:[$id],delete_local_after_sync:true}' |
    api POST /targets --data-binary @- | jq -er '.id | numbers')
backup=$(api POST /backups --data "{\"target_id\":$plan,\"backup_type\":\"full\"}" | jq -er '.id | numbers')
entry() {
    jq -er --argjson storage "$storage" --argjson target "$plan" '
        select(.target_id == $target and .status == "completed" and .encrypted == false and .local_available == false and .remote_synced == true) |
        .remote_sync_details | select(length == 1) | .[0] |
        select(.storage_id == $storage and .status == "completed") | .remote_path |
        strings | select(test("^dockervault-[0-9a-f]{32}/[^/]+\\.tar$"))'
}
key=
for ((attempt=0; attempt<30; attempt++)); do
    item=$(api GET "/backups/$backup" --max-time 5)
    [[ $(jq -er '.status' <<< "$item") != failed ]]
    if key=$(entry <<< "$item"); then break; fi
    sleep 1
done
[[ -n "$key" ]]
primary_empty() {
    local entries
    entries=$(docker exec "$app" find /backups -mindepth 1 ! -type d -print)
    [[ -z "$entries" ]]
}
primary_empty
archive=$(ftp_read "$key")
read -r archive_hash archive_size <<< "$archive"
# Query only the app's anonymous data volume, read-only; never copy its credentials or write SQLite.
catalog() {
    docker run --rm --pull=never -i --network none --volumes-from "$app:ro" --user 1000:1000 \
        "$fixture" python - "$backup" "$plan" "$storage" "$key" "$archive_size" "$archive_hash" "$prefix" "$source" <<'PY'
import json
from pathlib import Path
import sqlite3
import sys

backup, target, storage, path, size, digest, prefix, source = sys.argv[1:]
with sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True) as db:
    db.execute("BEGIN")
    assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
    assert db.execute("PRAGMA foreign_key_check").fetchall() == []
    assert db.execute("SELECT id,storage_type,enabled FROM remote_storages").fetchall() == [(int(storage), "ftp", 1)]
    (destinations, cleanup), = db.execute("SELECT remote_storage_ids,delete_local_after_sync FROM backup_targets WHERE id=?", (target,))
    assert json.loads(destinations) == [int(storage)] and cleanup == 1
    rows = db.execute("""
        SELECT b.target_id, b.status, b.encrypted, b.file_size, b.file_path, b.checksum,
               s.storage_id, s.remote_path, s.sync_status, s.synced_at, s.error_message
        FROM backups b JOIN backup_storage_syncs s ON s.backup_id=b.id WHERE b.id=?
        """, (backup,)).fetchall()
    row, = rows
    assert row[:4] == (int(target), "COMPLETED", 0, int(size)), row
    assert row[6:9] == (int(storage), path, "completed") and row[9] and row[10] is None, row
    if prefix == "/canary-copies":
        assert db.execute("SELECT base_path FROM remote_storages").fetchall() == [(prefix,)]
        assert db.execute("SELECT id,host_path FROM backup_targets").fetchall() == [(int(target), source)]
        assert db.execute("SELECT id FROM backups").fetchall() == [(int(backup),)]
        assert db.execute("SELECT backup_id,storage_id FROM backup_storage_syncs").fetchall() == [(int(backup), int(storage))]
        assert db.execute("SELECT count(*) FROM workload_operations").fetchone() == (0,)
        assert row[5] == digest
        assert Path(row[4]).is_relative_to("/backups") and not Path(row[4]).is_relative_to("/backups/r14-python-output")
    print(json.dumps(row))
PY
}
settled=$(catalog)
printf 'Durable FTP sync: backup=%s target=%s storage=%s bytes=%s sha256=%s; primary archive absent\n' \
    "$backup" "$plan" "$storage" "$archive_size" "$archive_hash"
if [[ -n "$control" ]]; then
    verify_control
    printf 'R14 Canary: source=%s; one completed sync and full FTP RETR/tar payload at %s/%s; catalog=%s\n' \
        "$source" "$prefix" "$key" "$settled"
    exit 0
fi
docker exec -u 1000:1000 "$app" sh -ec '
    printf "mutated\n" > "$1/content"; chmod 600 "$1/content"; touch -d @1710000000 "$1/content"
' sh "$source"

phase=clean-backend-restart
inode=$(docker exec "$app" stat -c '%d:%i' /app/data/backup.db)
old_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$old_pid" =~ ^[0-9]+$ && "$old_pid" -gt 1 ]]
docker exec "$app" s6-svc -wD -T 30000 -d "$service"
[[ $(docker exec "$app" s6-svstat -o up,exitcode "$service") == 'false 0' ]]
docker exec "$app" test ! -e "/proc/$old_pid"
docker exec "$app" s6-svc -wu -T 30000 -u "$service"
ready
new_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$new_pid" =~ ^[0-9]+$ && "$new_pid" -gt 1 && "$new_pid" != "$old_pid" ]]
[[ $(docker exec "$app" stat -c '%d:%i' /app/data/backup.db) == "$inode" ]]
docker exec -u 1000:1000 "$app" rm /tmp/ftp-restart-cookie
authenticate login
unset credentials
[[ $(api GET "/backups/$backup" | entry) == "$key" ]]
[[ $(catalog) == "$settled" ]]
primary_empty
printf 'Clean backend restart: PID %s -> %s, same SQLite device:inode %s; durable FTP row unchanged\n' "$old_pid" "$new_pid" "$inode"

phase=restore-and-verify
docker exec -u 1000:1000 "$app" sh -ec '
    printf "mutated\n" | cmp - "$1/content"
    test "$(stat -c "%a:%Y" "$1/content")" = 600:1710000000
' sh "$source"
fingerprint=$(api POST "/backups/$backup/restore-review" --data '{"destination_type":"original"}' |
    jq -er --arg dest "$source" 'select(.destination.path == $dest and .key_required == false) | .fingerprint | strings | select(length > 0)')
jq -nc --arg fingerprint "$fingerprint" '{destination_type:"original",review_fingerprint:$fingerprint}' |
    api POST "/backups/$backup/restore" --data-binary @- | jq -e '.status == "restored"' >/dev/null
docker exec -u 1000:1000 "$app" sh -ec '
    printf "\000plain FTP fixture\377\n" | cmp - "$1/content"
    test "$(stat -c "%a:%Y" "$1/content")" = 640:1700000000
' sh "$source"
primary_empty
[[ $(ftp_read "$key") == "$archive" ]]
[[ $(api GET "/backups/$backup" | entry) == "$key" ]]
[[ $(catalog) == "$settled" ]]
printf 'Plain FTP-only restore: exact bytes, mode 0640, mtime 1700000000; unchanged FTP SHA-256 and consistent SQLite catalog/sync (%s, %s)\n' "$arch" "$rev"
