#!/usr/bin/env bash
# Restore from the sole verified LOCAL storage copy after a clean native backend restart.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-local-restart.sh IMAGE plain|encrypted}
mode=${2:?Specify plain or encrypted}
case "$mode" in
    plain) encrypted=false ;;
    encrypted) encrypted=true ;;
    *) printf 'Unknown LOCAL restore mode: %s\n' "$mode" >&2; exit 2 ;;
esac
[[ ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || {
    printf 'Run this container test only on the GitHub-hosted CI matrix\n' >&2
    exit 2
}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
arch=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$image")
[[ "$rev" =~ ^[a-f0-9]{40}$ && ( "$arch" == linux/amd64 || "$arch" == linux/arm64 ) ]]
app=$(docker run --detach --rm --pull=never --network none --cpus 2 --memory 6g "$image")
phase=startup
cleanup() {
    local status=$? remaining attempt
    if ((status != 0)); then
        printf 'LOCAL %s roundtrip failed at %s (%s, %s)\n' "$mode" "$phase" "$arch" "$rev" >&2
    fi
    docker stop --time 30 "$app" >/dev/null || return 1
    for ((attempt=0; attempt<30; attempt++)); do
        remaining=$(docker ps --all --quiet --no-trunc --filter "id=$app") || return 1
        if [[ -z "$remaining" ]]; then
            if ((status == 0)); then
                printf 'LOCAL %s restore after clean restart: bytes, link target, exact layout, unchanged LOCAL artifacts and cleanup (%s, %s)\n' "$mode" "$arch" "$rev"
            fi
            return "$status"
        fi
        sleep 1
    done
    printf 'Owned test container was not removed: %s\n' "$app" >&2
    return 1
}
trap cleanup EXIT

service=/run/dockervault/services/backend
source=/app/data/local-restart-source
store=/app/data/local-restart-copies
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health 2>/dev/null | \
            jq -e '.status == "healthy"' >/dev/null; then return; fi
        sleep 1
    done
    return 1
}
api() {
    local method=$1 path=$2
    shift 2
    docker exec -i -u 1000:1000 "$app" curl -fsS --max-time 45 -b /tmp/local-restart-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
authenticate() {
    printf '%s' "$credentials" | api POST "/auth/$1" -c /tmp/local-restart-cookie --data-binary @- >/dev/null
}
ready
credentials=$(jq -nc --arg password "$(tr -d '-' </proc/sys/kernel/random/uuid)" \
    '{username:"local-restart",password:$password,confirm_password:$password}')
authenticate setup
docker exec -u 1000:1000 "$app" sh -ec '
    umask 077
    mkdir -p "$1/nested" "$2"
    printf "\000original payload\377\n" > "$1/payload"
    printf "nested fixture bytes\n" > "$1/nested/value"
    ln -s nested "$1/nested-link"
' sh "$source" "$store"
storage=$(jq -nc --arg path "$store" '{name:"LOCAL restart fixture",storage_type:"local",base_path:$path,enabled:true}' |
    api POST /storage --data-binary @- | jq -er '.id | numbers')
if [[ "$encrypted" == true ]]; then
    identity=$(api POST /encryption/setup --data '{}' | jq -er '.private_key | strings | select(startswith("AGE-SECRET-KEY-"))')
    api POST /encryption/confirm-setup --data '{"confirmed":true}' >/dev/null
fi

phase=backup-and-sync
plan=$(jq -nc --arg path "$source" --argjson storage "$storage" \
    '{name:"LOCAL restart",target_type:"path",host_path:$path,stop_container:false,compression_enabled:true,remote_storage_ids:[$storage],delete_local_after_sync:true}' |
    api POST /targets --data-binary @- | jq -er '.id | numbers')
backup=$(api POST /backups --data "{\"target_id\":$plan,\"backup_type\":\"full\"}" | jq -er '.id | numbers')
local_copy() {
    jq -er --argjson storage "$storage" --argjson encrypted "$encrypted" '
        select(.status == "completed" and .encrypted == $encrypted and .local_available == false and .remote_synced == true) |
        .remote_sync_details | select(length == 1) | .[0] |
        select(.storage_id == $storage and .status == "completed") |
        .remote_path | strings | select(test("^[^/]+(/[^/]+)*$") and (split("/") | all(. != "." and . != "..")))'
}
key=
for ((attempt=0; attempt<30; attempt++)); do
    item=$(api GET "/backups/$backup" --max-time 5)
    status=$(jq -er '.status | strings' <<< "$item")
    [[ "$status" != failed && "$status" != cancelled ]] || exit 1
    if key=$(local_copy <<< "$item"); then break; fi
    sleep 1
done
[[ -n "$key" ]]
archive="$store/$key"
artifacts=("$archive")
if [[ "$encrypted" == true ]]; then
    [[ "$archive" == *.tar.gz.enc ]]
    artifacts+=("${archive%.enc}.key")
else
    [[ "$archive" == *.tar.gz ]]
fi
copy_hashes() {
    docker exec -u 1000:1000 "$app" sh -ec '
        for path do test -f "$path" && test ! -L "$path" && test -s "$path" || exit 1; done
        sha256sum -- "$@"
    ' sh "${artifacts[@]}"
}
primary_empty() {
    local entries
    entries=$(docker exec "$app" find /backups -mindepth 1 ! -type d -print) || return 1
    [[ -z "$entries" ]]
}
primary_empty
hashes=$(copy_hashes)
expected_copies=$(printf '%s\n' "${artifacts[@]}" | LC_ALL=C sort)
copies() { docker exec "$app" find "$store" -mindepth 1 ! -type d -print | LC_ALL=C sort; }
[[ $(copies) == "$expected_copies" ]]
docker exec -u 1000:1000 "$app" sh -ec '
    printf "mutated after backup\n" > "$1/payload"
    rm "$1/nested/value" "$1/nested-link"
' sh "$source"

phase=clean-backend-restart
old_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$old_pid" =~ ^[0-9]+$ && "$old_pid" -gt 1 ]]
# Use the image's ordinary supervisor and wait for a successful exit before starting again.
docker exec "$app" s6-svc -wD -T 30000 -d "$service"
[[ $(docker exec "$app" s6-svstat -o up,exitcode "$service") == 'false 0' ]]
docker exec "$app" test ! -e "/proc/$old_pid"
docker exec "$app" s6-svc -wu -T 30000 -u "$service"
ready
new_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$new_pid" =~ ^[0-9]+$ && "$new_pid" -gt 1 && "$new_pid" != "$old_pid" ]]
docker exec -u 1000:1000 "$app" rm /tmp/local-restart-cookie
authenticate login
unset credentials
[[ $(api GET "/backups/$backup" | local_copy) == "$key" ]]
primary_empty
[[ $(copy_hashes) == "$hashes" ]]

phase=restore-and-verify
tmp_entries() { docker exec "$app" find /tmp -mindepth 1 -print | LC_ALL=C sort; }
before=$(tmp_entries)
fingerprint=$(api POST "/backups/$backup/restore-review" --data '{"destination_type":"original"}' |
    jq -er --arg dest "$source" --argjson encrypted "$encrypted" '
        select(.destination.path == $dest and .key_required == $encrypted) | .fingerprint | strings | select(length > 0)')
payload=$(jq -nc --arg fingerprint "$fingerprint" '{destination_type:"original",review_fingerprint:$fingerprint}')
if [[ "$encrypted" == true ]]; then
    # Only the fresh test identity is supplied over stdin, never logged or written to disk.
    payload=$(printf '%s\n%s\n' "$payload" "$identity" | jq -Rsc 'split("\n") | (.[0] | fromjson) + {private_key:.[1]}')
fi
printf '%s' "$payload" | api POST "/backups/$backup/restore" --data-binary @- | jq -e '.status == "restored"' >/dev/null
unset identity payload
docker exec -u 1000:1000 "$app" sh -ec '
    cd "$1"
    test -f payload && test ! -L payload
    test -d nested && test ! -L nested
    test -f nested/value && test ! -L nested/value
    printf "\000original payload\377\n" | cmp - payload
    printf "nested fixture bytes\n" | cmp - nested/value
    test -L nested-link && test "$(readlink nested-link)" = nested
' sh "$source"
layout=$(docker exec "$app" find "$source" -mindepth 1 -printf '%P\n' | LC_ALL=C sort)
[[ "$layout" == $'nested\nnested-link\nnested/value\npayload' ]]
primary_empty
after=$(tmp_entries)
[[ "$after" == "$before" ]]
[[ $(copies) == "$expected_copies" && $(copy_hashes) == "$hashes" ]]
[[ $(api GET "/backups/$backup" | local_copy) == "$key" ]]
