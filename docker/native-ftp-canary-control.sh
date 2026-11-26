#!/usr/bin/env bash
# Sourced only by the hosted FTP journey. Control setup ends before the baseline is captured.
control_ref=39124ea3a98963b6307a69a0fbaf5902a17cde3d
control_source=/app/data/r14-python-source
control_target=/backups/r14-python-output
[[ $(docker image inspect -f '{{.Os}}/{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}' "$control_image") == "$arch $control_ref" ]]
[[ $(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$fixture") == "$arch" ]]

control_api() {
    local path=$1
    shift
    docker exec -i -u 1000:1000 "$control" curl -fsS --max-time 45 -b /tmp/r14-control-cookie \
        -H 'Content-Type: application/json' -X POST "$@" "http://127.0.0.1/api/v1$path"
}

control_state() {
    docker run --rm --pull=never -i --network none --volumes-from "$control:ro" --user 1000:1000 \
        "$fixture" python - "$control_source" "$control_target" "$control_storage" <<'PY'
from hashlib import sha256
import json
from pathlib import Path
import sqlite3
import sys

source, destination = map(Path, sys.argv[1:3])
with sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True) as db:
    db.execute("BEGIN")
    assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
    rows = db.execute("SELECT host_path,remote_storage_ids FROM backup_targets").fetchall()
    (path, storages), = rows
    assert path == str(source) and json.loads(storages) == [int(sys.argv[3])]
    assert db.execute("SELECT storage_type,base_path FROM remote_storages").fetchall() == [("ftp", "/control-copies")]
    assert db.execute("SELECT count(*) FROM backups").fetchone() == (0,)
    catalog = sha256("\n".join(db.iterdump()).encode()).hexdigest()
files = []
for root in (source, destination):
    file, = sorted(root.rglob("*"))
    assert file == root / "reference" and file.is_file() and not file.is_symlink()
    stat = file.stat()
    files.append([str(file), stat.st_mode, stat.st_mtime_ns, sha256(file.read_bytes()).hexdigest()])
print(json.dumps({"catalog": catalog, "files": files}, sort_keys=True))
PY
}

control_objects() {
    docker exec -i "$provider" python - <<'PY'
from hashlib import sha256
from pathlib import Path

root = Path("/fixture/python/control-copies")
file, = sorted(root.rglob("*"))
assert file == root / "reference" and file.is_file() and not file.is_symlink()
stat = file.stat()
print(file, stat.st_mode, stat.st_mtime_ns, sha256(file.read_bytes()).hexdigest())
PY
}

prepare_control() {
    local mounts
    phase=python-control-fixture
    control=$(docker create --pull=never --network none --cpus 2 --memory 6g \
        -e "BACKUP_BASE_PATH=$control_target" "$control_image")
    docker start "$control" >/dev/null
    ready "$control"
    docker top "$control" -eo pid,args | grep -E '^[[:space:]]*[0-9]+[[:space:]]+[^ ]*python[^ ]* [^ ]*/uvicorn app\.main:app '
    docker top "$app" -eo pid,args | grep -E '^[[:space:]]*[0-9]+[[:space:]]+/usr/local/bin/dockervault-service '
    mounts=$(docker inspect "$control" "$app" "$provider" | jq -ce --arg provider "$provider" '
        select(([.[].Id] | unique | length) == 3 and all(.[]; .State.Running and
            (.HostConfig.Privileged | not) and (.HostConfig.PortBindings | length) == 0)) |
        select(.[0].HostConfig.NetworkMode == "none" and .[1].HostConfig.NetworkMode == ("container:" + $provider) and
            .[2].HostConfig.NetworkMode == "none" and all(.[2].Mounts[]; .Type == "tmpfs" and .Destination == "/fixture")) |
        .[:2] | select(all(.[]; ([.Mounts[].Destination] | sort) == ["/app/data", "/backups"] and
            all(.Mounts[]; .Type == "volume" and .RW))) |
        select(([.[].Mounts[].Name] | unique | length) == 4) |
        map({container: .Id, image: .Image, volumes: (.Mounts | map({path:.Destination, name:.Name}))})')
    volumes=$(jq -r '.[].volumes[].name' <<< "$mounts")
    printf 'R14 Canary binding: %s Rust=%s Python=%s FTP=%s fixture=%s\n%s\n' \
        "$arch" "$rev" "$control_ref" "$provider" "$(docker image inspect -f '{{.Id}}' "$fixture")" "$mounts"

    jq -nc --arg pass "$(tr -d '-' </proc/sys/kernel/random/uuid)" \
        '{username:"r14-control",password:$pass,confirm_password:$pass}' |
        control_api /auth/setup -c /tmp/r14-control-cookie --data-binary @- >/dev/null
    docker exec -u 1000:1000 "$control" sh -ec '
        mkdir -p "$1" "$2"
        printf "Python reference source\n" > "$1/reference"
        printf "Python reference destination\n" > "$2/reference"
    ' sh "$control_source" "$control_target"
    control_storage=$(control_api /storage --data '{"name":"Control FTP","storage_type":"ftp","enabled":true,"host":"127.0.0.1","port":32121,"username":"control","password":"public-control-only","base_path":"/control-copies"}' | jq -er '.id | numbers')
    jq -nc --arg path "$control_source" --argjson id "$control_storage" \
        '{name:"Control plan",target_type:"path",host_path:$path,stop_container:false,remote_storage_ids:[$id]}' |
        control_api /targets --data-binary @- | jq -e '.id | numbers' >/dev/null
    before=$(control_state)
    objects_before=$(control_objects)
    printf 'R14 Canary control before: %s; FTP object: %s\n' "$before" "$objects_before"
}

verify_control() {
    local after objects_after control_db canary_db
    phase=canary-readback-and-control-comparison
    ready "$control"
    after=$(control_state)
    objects_after=$(control_objects)
    [[ "$before" == "$after" && "$objects_before" == "$objects_after" ]]
    control_db=$(docker exec "$control" stat -c '%d:%i' /app/data/backup.db)
    canary_db=$(docker exec "$app" stat -c '%d:%i' /app/data/backup.db)
    [[ "$control_db" != "$canary_db" ]]
    printf 'R14 Canary DB device:inode: Python=%s Rust=%s\n' "$control_db" "$canary_db"
    printf 'R14 Canary control after:  %s; FTP object: %s\n' "$after" "$objects_after"
}
