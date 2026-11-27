#!/usr/bin/env bash
# #143 LOCAL oracle: bundle metadata and retained encrypted bytes are separate inputs.
# Use the standard package and the backup/restore HTTP routes, not a seeded database.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-system-recovery.sh RUST_IMAGE [PYTHON_IMAGE PYTHON_REVISION TARGET_REVISION]}
source_image=${2:-$image}
python_ref=${3:-}
if (($# != 1)); then
    if [[ $# != 4 || ${GITHUB_ACTIONS:-} != true || ${RUNNER_ENVIRONMENT:-} != github-hosted ]]; then
        printf 'Python package recovery requires four arguments and a GitHub-hosted Actions runner\n' >&2
        exit 2
    fi
    [[ "$python_ref" =~ ^[a-f0-9]{40}$ && "$4" =~ ^[a-f0-9]{40}$ && "$python_ref" != "$4" ]]
fi
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
arch=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$image")
[[ "$rev" =~ ^[a-f0-9]{40}$ && ( "$arch" == linux/amd64 || "$arch" == linux/arm64 ) ]]
if [[ -n "$python_ref" ]]; then
    case "$(uname -m)" in
        x86_64) runner_arch=linux/amd64 ;;
        aarch64) runner_arch=linux/arm64 ;;
        *) exit 2 ;;
    esac
    [[ "$arch" == "$runner_arch" && "$rev" == "$4" ]]
    [[ $(docker image inspect -f '{{.Os}}/{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}}' "$source_image") == "$arch $python_ref" ]]
    printf '#143 Python→Rust packages: %s source=%s@%s (%s), target=%s@%s (%s)\n' \
        "$arch" "$source_image" "$python_ref" "$(docker image inspect -f '{{.Id}}' "$source_image")" \
        "$image" "$rev" "$(docker image inspect -f '{{.Id}}' "$image")"
fi
source_app= restored_app=
phase=startup
cleanup() {
    local status=$? app
    if ((status != 0)); then printf '#143 LOCAL failed at %s (%s, %s)\n' "$phase" "$arch" "$rev" >&2; fi
    for app in "$source_app" "$restored_app"; do
        [[ -z "$app" ]] || docker rm -fv "$app" >/dev/null || status=1
    done
    if ((status == 0)); then printf '#143 LOCAL: both isolated containers and anonymous volumes removed (%s, %s)\n' "$arch" "$rev"; fi
    return "$status"
}
trap cleanup EXIT

start() { docker run --detach --pull=never --network none --cpus 2 --memory 6g "$1"; }
ready() {
    local app=$1 attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health 2>/dev/null |
            jq -e '.status == "healthy"' >/dev/null; then return; fi
        sleep 1
    done
    return 1
}
api() {
    local app=$1 method=$2 path=$3
    shift 3
    docker exec -i -u 1000:1000 "$app" curl -fsS --max-time 45 -X "$method" \
        -H 'Content-Type: application/json' "$@" "http://127.0.0.1/api/v1$path"
}
source_api() { api "$source_app" "$1" "$2" -b /tmp/source-cookie "${@:3}"; }
restored_api() { api "$restored_app" "$1" "$2" -b /tmp/restored-cookie "${@:3}"; }
setup_incomplete() {
    api "$1" GET /auth/status | jq -e '.setup_complete == false and .authenticated == false' >/dev/null
}

source_app=$(start "$source_image")
restored_app=$(start "$image")
ready "$source_app"
ready "$restored_app"
setup_incomplete "$source_app"
setup_incomplete "$restored_app"
if [[ -n "$python_ref" ]]; then
    # Docker requires the PID column even when verifying only command arguments.
    docker top "$source_app" -eo pid,args | grep -E '^[[:space:]]*[0-9]+[[:space:]]+[^ ]*python[^ ]* [^ ]*/uvicorn app\.main:app '
    docker top "$restored_app" -eo pid,args | grep -E '^[[:space:]]*[0-9]+[[:space:]]+/usr/local/bin/dockervault-service '
    docker inspect "$source_app" "$restored_app" | jq -e '
        all(.[]; .HostConfig.NetworkMode == "none" and
            ([.Mounts[].Destination] | sort) == ["/app/data", "/backups"] and
            all(.Mounts[]; .Type == "volume")) and
        ([.[].Mounts[].Name] | unique | length) == 4' >/dev/null
    printf '#143 Python→Rust: both stock packages healthy and uninitialized, four disjoint anonymous volumes\n'
else
    printf '#143 LOCAL: both default packages healthy and uninitialized (%s, %s)\n' "$arch" "$rev"
fi

phase=source-fixture
fixture=/app/data/recovery-source
store=/app/data/recovery-local
source_password=$(tr -d '-' </proc/sys/kernel/random/uuid)
admin_password=$(tr -d '-' </proc/sys/kernel/random/uuid)
bundle_password=$(tr -d '-' </proc/sys/kernel/random/uuid)
jq -nc --arg pass "$source_password" '{username:"source-admin",password:$pass,confirm_password:$pass}' |
    source_api POST /auth/setup -c /tmp/source-cookie --data-binary @- >/dev/null
source_api GET /auth/status | jq -e '.setup_complete and .authenticated and .user.is_admin and .user.username == "source-admin"' >/dev/null
source_api POST /encryption/setup --data '{}' |
    jq -er '.private_key | strings | select(startswith("AGE-SECRET-KEY-"))' |
    docker exec -i -u 1000:1000 "$source_app" sh -ec 'umask 077; cat >/tmp/recovery-identity; test -s /tmp/recovery-identity'
source_api POST /encryption/confirm-setup --data '{"confirmed":true}' >/dev/null

# Identical paths but independent volumes. The restored instance starts with a different payload.
for app in "$source_app" "$restored_app"; do
    docker exec -u 1000:1000 "$app" mkdir "$fixture" "$store"
done
docker exec -u 1000:1000 "$source_app" sh -ec 'printf "DockerVault143 retained archive bytes\000\377\n" > "$1/payload"' sh "$fixture"
docker exec -u 1000:1000 "$restored_app" sh -ec 'printf "Not yet restored\n" > "$1/payload"' sh "$fixture"
storage=$(jq -nc --arg path "$store" '{name:"Recovery LOCAL",storage_type:"local",base_path:$path,enabled:true}' |
    source_api POST /storage --data-binary @- | jq -er '.id | numbers')
plan=$(jq -nc --arg path "$fixture" --argjson storage "$storage" \
    '{name:"Recovery plan",target_type:"path",host_path:$path,stop_container:false,compression_enabled:true,remote_storage_ids:[$storage]}' |
    source_api POST /targets --data-binary @- | jq -er '.id | numbers')
backup=$(source_api POST /backups --data "{\"target_id\":$plan,\"backup_type\":\"full\"}" | jq -er '.id | numbers')
local_copy() {
    jq -er --argjson storage "$storage" --argjson plan "$plan" --argjson backup "$backup" '
        select(.id == $backup and .target_id == $plan and .status == "completed" and .encrypted == true and .remote_synced == true) |
        .remote_sync_details | select(length == 1) | .[0] |
        select(.storage_id == $storage and .status == "completed") |
        .remote_path | strings | select(endswith(".tar.gz.enc") and test("^[^/]+(/[^/]+)*$") and (split("/") | all(. != "." and . != "..")))'
}
key=
for ((attempt=0; attempt<30; attempt++)); do
    item=$(source_api GET "/backups/$backup" --max-time 5)
    status=$(jq -er '.status | strings' <<< "$item")
    [[ "$status" != failed && "$status" != cancelled ]] || exit 1
    if key=$(local_copy <<< "$item"); then break; fi
    sleep 1
done
[[ -n "$key" ]]
sidecar=${key%.enc}.key
source_api GET "/backups/$backup/logs" | jq -e 'length > 0' >/dev/null
catalog() { jq -cS '{id,target_id,target_name,status,backup_type,file_path,file_size,checksum,encrypted,remote_sync_details: [.remote_sync_details[] | {storage_id,remote_path,status}]}'; }
source_catalog=$(catalog <<< "$item")
# The HTTP DTO omits the sync row's own ID. Read only the owned test databases.
sync_row() {
    docker run --rm -i --pull=never --network none --volumes-from "$1:ro" \
        --user 1000:1000 --entrypoint python "$source_image" - "$backup" "$storage" "$key" <<'PY'
import json
import sqlite3
import sys

backup, storage, path = sys.argv[1:]
with sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True) as db:
    row, = db.execute("SELECT id, backup_id, storage_id, sync_status, remote_path FROM backup_storage_syncs")
    assert row[0] > 0 and row[1:] == (int(backup), int(storage), "completed", path)
    print(json.dumps(row))
PY
}
if [[ -n "$python_ref" ]]; then
    source_sync=$(sync_row "$source_app")
    public_key=$(source_api GET /encryption/status | jq -er '
        select(.setup_completed and .encryption_enabled) | .public_key | strings | select(startswith("age1"))')
fi
archive_hashes() {
    docker exec -u 1000:1000 "$1" sh -ec '
        cd "$1"; shift
        for file do test -f "$file" && test ! -L "$file" && test -s "$file" || exit 1; done
        sha256sum -- "$@"
    ' sh "$store" "$key" "$sidecar"
}
retained_hashes=$(archive_hashes "$source_app")

phase=export
export_request=$(jq -nc --arg pass "$bundle_password" '{password:$pass,confirm_password:$pass}')
[[ $(printf '%s' "$export_request" | api "$source_app" POST /recovery/export --no-fail -o /dev/null -w '%{http_code}' --data-binary @-) == 401 ]]
# Keep the encrypted download and headers in the disposable source container.
printf '%s' "$export_request" | source_api POST /recovery/export --data-binary @- \
    -D /tmp/recovery-headers -o /tmp/system.dockervault
docker exec "$source_app" grep -qi '^content-disposition: attachment; filename="dockervault-system-backup\.dockervault"' /tmp/recovery-headers
docker exec "$source_app" grep -qi '^content-type: application/octet-stream' /tmp/recovery-headers
docker exec "$source_app" cat /tmp/system.dockervault |
    jq -e '.format == "dockervault-recovery" and .envelope_version == 1 and .kdf.name == "argon2id" and .aead.name == "aes-256-gcm" and (.ciphertext_b64 | length > 0)' >/dev/null
# Keep the exact download, including any trailing newline, out of shell substitution.
docker exec "$source_app" cat /tmp/system.dockervault |
    docker exec -i -u 1000:1000 "$restored_app" sh -ec 'umask 077; cat >/tmp/system.dockervault'
bundle_hash=$(docker exec "$source_app" sha256sum /tmp/system.dockervault)
[[ $(docker exec "$restored_app" sha256sum /tmp/system.dockervault) == "$bundle_hash" ]]
printf '#143 LOCAL: completed encrypted backup and LOCAL catalog copy; anonymous export 401, admin download V1 Argon2id/AES-GCM\n'
if [[ -n "$python_ref" ]]; then
    printf '#143 Python→Rust: identical V1 bundle SHA-256 %s; separately retained archive/sidecar:\n%s\n' "${bundle_hash%% *}" "$retained_hashes"
fi

# Compare directory inventory and file bytes, including SQLite main/WAL. SHM is SQLite's volatile lock/index.
# Preview may create the empty, persistent flock anchor for crypto serialization, not application data.
fresh_state() {
    docker exec "$restored_app" sh -ec '
        lock=/app/data/backup.db.recovery-crypto.lock
        if [ -e "$lock" ] || [ -L "$lock" ]; then
            test -f "$lock" && test ! -L "$lock" && test ! -s "$lock" || exit 1
        fi
        find /app/data /backups ! -path /app/data/backup.db-shm ! -path "$1" -printf "%y %p\n"
        find /app/data /backups -type f ! -path /app/data/backup.db-shm ! -path "$1" -exec sha256sum {} +
    ' sh "${1:-}" | LC_ALL=C sort
}
setup_incomplete "$restored_app"
crypto_lock=/app/data/backup.db.recovery-crypto.lock
before=$(fresh_state "$crypto_lock")
preview_request() {
    docker exec "$restored_app" cat /tmp/system.dockervault | jq -Rsc --arg pass "$1" '{bundle:.,password:$pass}'
}
restore_request() {
    preview_request "$1" | jq -c --arg pass "$admin_password" \
        '. + {username:"restored-admin",admin_password:$pass,confirm_admin_password:$pass}'
}
phase=preview
preview=$(preview_request "$bundle_password" | api "$restored_app" POST /recovery/preview --data-binary @-)
printf '%s\n' "$preview" | jq -c '{counts,storages,archives_included,archive_private_keys_included}'
printf '%s\n' "$preview" |
    jq -e --argjson storage "$storage" '
        .counts.backup_targets == 1 and .counts.backups == 1 and .counts.remote_storages == 1 and .counts.backup_storage_syncs == 1 and
        (.counts | has("users") or has("sessions") or has("backup_logs") | not) and
        .storages == [{id:$storage,type:"local",state:"not_tested"}] and
        .archives_included == false and .archive_private_keys_included == false' >/dev/null
setup_incomplete "$restored_app"
after=$(fresh_state "$crypto_lock")
[[ "$before" == "$after" ]]

if [[ -z "$python_ref" ]]; then
    phase=wrong-password
    before=$(fresh_state)
    http=$(restore_request 'wrong-143-test-password' | api "$restored_app" POST /recovery/restore \
        --no-fail -o /tmp/recovery-rejection -w '%{http_code}' --data-binary @-)
    [[ "$http" == 400 ]]
    docker exec "$restored_app" cat /tmp/recovery-rejection |
        jq -e '.detail == "Invalid or incompatible recovery bundle or password"' >/dev/null
    setup_incomplete "$restored_app"
    after=$(fresh_state)
    [[ "$before" == "$after" ]]
    printf '#143 LOCAL: preview preserved application data (empty crypto lock allowed); wrong import password 400, still uninitialized, identical data inventory and file hashes\n'
else
    printf '#143 Python→Rust: preview kept setup incomplete and app-data inventory and hashes identical (empty crypto lock allowed)\n'
fi

phase=activation
# Transfer only retained archive/sidecar, never the primary, source DB or sessions.
docker exec -u 1000:1000 "$source_app" tar -C "$store" -cf - -- "$key" "$sidecar" |
    docker exec -i -u 1000:1000 "$restored_app" tar -C "$store" -xf -
[[ $(archive_hashes "$restored_app") == "$retained_hashes" ]]
primary_empty() {
    local entries
    entries=$(docker exec "$restored_app" find /backups -mindepth 1 ! -type d -print) || return 1
    [[ -z "$entries" ]]
}
primary_empty
restore_request "$bundle_password" | api "$restored_app" POST /recovery/restore --data-binary @- |
    jq -e --argjson storage "$storage" --argjson backup "$backup" '
        .user.username == "restored-admin" and .user.is_admin and .result.archives_modified == false and
        .result.storages == [{id:$storage,type:"local",state:"connected"}] and
        .result.catalog == [{id:$backup,local:"unavailable",copies:[{storage_id:$storage,state:"available"}]}]' >/dev/null
ready "$restored_app"
api "$restored_app" GET /auth/status | jq -e '.setup_complete and (.authenticated | not)' >/dev/null
# Send the actual source cookie as a synthetic negative control, without logging it.
docker exec "$source_app" cat /tmp/source-cookie |
    docker exec -i -u 1000:1000 "$restored_app" sh -ec 'umask 077; cat >/tmp/source-cookie'
api "$restored_app" GET /auth/status -b /tmp/source-cookie | jq -e '.setup_complete and (.authenticated | not)' >/dev/null
jq -nc --arg pass "$admin_password" '{username:"restored-admin",password:$pass}' |
    restored_api POST /auth/login -c /tmp/restored-cookie --data-binary @- >/dev/null
restored_api GET /auth/status | jq -e '.setup_complete and .authenticated and .user.is_admin and .user.username == "restored-admin"' >/dev/null
restored_api GET "/targets/$plan" | jq -e --arg path "$fixture" --argjson storage "$storage" --argjson plan "$plan" '
    .id == $plan and .name == "Recovery plan" and .target_type == "path" and .host_path == $path and .remote_storage_ids == [$storage]' >/dev/null
restored_api GET "/storage/$storage" | jq -e --arg path "$store" --argjson storage "$storage" '
    .id == $storage and .name == "Recovery LOCAL" and .storage_type == "local" and .base_path == $path and .enabled' >/dev/null
item=$(restored_api GET "/backups/$backup")
[[ $(local_copy <<< "$item") == "$key" && $(catalog <<< "$item") == "$source_catalog" ]]
jq -e '.local_available == false' <<< "$item" >/dev/null
restored_api GET "/backups/$backup/logs" | jq -e '. == []' >/dev/null
[[ $(archive_hashes "$restored_app") == "$retained_hashes" ]]
printf '#143 LOCAL: setup activated and healthy; fresh admin login, source session/logs excluded, target/storage/catalog preserved\n'
if [[ -n "$python_ref" ]]; then
    [[ $(sync_row "$restored_app") == "$source_sync" ]]
    restored_api GET /encryption/status | jq -e --arg key "$public_key" '
        .setup_completed and .encryption_enabled and .public_key == $key' >/dev/null
    printf '#143 Python→Rust: original public identity and sync row preserved: %s\n' "$source_sync"
fi

phase=archive-restore
fingerprint=$(restored_api POST "/backups/$backup/restore-review" --data '{"destination_type":"original"}' |
    jq -er --arg dest "$fixture" 'select(.destination.path == $dest and .key_required == true) | .fingerprint | strings | select(length > 0)')
docker exec "$source_app" cat /tmp/recovery-identity |
    jq -Rsc --arg fp "$fingerprint" '{destination_type:"original",review_fingerprint:$fp,private_key:rtrimstr("\n")}' |
    restored_api POST "/backups/$backup/restore" --data-binary @- | jq -e '.status == "restored"' >/dev/null
docker exec -u 1000:1000 "$restored_app" sh -ec '
    cd "$1"; test -f payload && test ! -L payload
    printf "DockerVault143 retained archive bytes\000\377\n" | cmp - payload
    sha256sum payload
' sh "$fixture"
primary_empty
[[ $(archive_hashes "$source_app") == "$retained_hashes" && $(archive_hashes "$restored_app") == "$retained_hashes" ]]
[[ $(restored_api GET "/backups/$backup" | catalog) == "$source_catalog" ]]
if [[ -n "$python_ref" ]]; then
    [[ $(sync_row "$restored_app") == "$source_sync" ]]
    [[ $(docker exec "$restored_app" sha256sum /tmp/system.dockervault) == "$bundle_hash" ]]
fi
printf '#143 LOCAL: real restore with retained identity and sidecar; payload byte-for-byte, retained pair unchanged, no primary archive\n'
if [[ -n "${DOCKERVAULT_SHADOW_RUST_IMAGE:-}" ]]; then
    phase=readonly-shadow
    bash docker/test-native-shadow.sh "$source_app" "$source_image" "$DOCKERVAULT_SHADOW_RUST_IMAGE" \
        "$key" "$backup" "$python_ref" "$rev"
fi
