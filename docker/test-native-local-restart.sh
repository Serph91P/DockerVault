#!/usr/bin/env bash
# Restore the primary, explicitly sync to LOCAL, then restore from that copy after restart.
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

phase=primary-backup
plan=$(jq -nc --arg path "$source" \
    '{name:"LOCAL restart",target_type:"path",host_path:$path,stop_container:false,compression_enabled:true,remote_storage_ids:[],delete_local_after_sync:false}' |
    api POST /targets --data-binary @- | jq -er '.id | numbers')
backup=$(api POST /backups --data "{\"target_id\":$plan,\"backup_type\":\"full\"}" | jq -er '.id | numbers')
status=
for ((attempt=0; attempt<30; attempt++)); do
    item=$(api GET "/backups/$backup" --max-time 5)
    status=$(jq -er '.status | strings' <<< "$item")
    [[ "$status" != failed && "$status" != cancelled ]] || exit 1
    if [[ "$status" == completed ]]; then break; fi
    sleep 1
done
[[ "$status" == completed ]]
primary=$(jq -er --argjson encrypted "$encrypted" '
    select(.encrypted == $encrypted and .local_available == true and .remote_synced == false and .remote_sync_details == []) |
    .file_path | strings | select(startswith("/backups/"))' <<< "$item")
catalog() { jq -cS '{id,target_id,target_name,backup_type,status,file_size,checksum,encrypted,created_at}'; }
original_catalog=$(catalog <<< "$item")
copies() { docker exec "$app" find "$store" -mindepth 1 ! -type d -print | LC_ALL=C sort; }
primary_entries() { docker exec "$app" find /backups -mindepth 1 ! -type d -print | LC_ALL=C sort; }
entries=$(copies)
[[ -z "$entries" ]]

phase=catalog-checksum
primary_artifacts=("$primary")
if [[ "$encrypted" == true ]]; then
    [[ "$primary" == *.tar.gz.enc ]]
    primary_artifacts+=("${primary%.enc}.key")
else
    [[ "$primary" == *.tar.gz ]]
fi
artifact_hashes() {
    docker exec -u 1000:1000 "$app" sh -ec '
        for path do test -f "$path" && test ! -L "$path" && test -s "$path" || exit 1; done
        sha256sum -- "$@"
    ' sh "$@"
}
primary_hashes=$(artifact_hashes "${primary_artifacts[@]}")
envelope_hash=${primary_hashes%% *}
catalog_checksum=$(jq -er '.checksum | strings | select(test("^[a-f0-9]{64}$"))' <<< "$item")
if [[ "$encrypted" == true ]]; then
    # The catalog hashes the archive before encryption, not the encrypted envelope.
    plaintext_hash=$(printf '%s\n' "$identity" | docker exec -i -u 1000:1000 "$app" sh -ec '
        scratch=$(mktemp -d)
        trap '\''rm -rf -- "$scratch"'\'' EXIT
        dockervault-recover --identity /dev/stdin --archive "$1" --key "$2" --output "$scratch/plain"
        sha256sum -- "$scratch/plain"
    ' sh "${primary_artifacts[@]}")
    [[ "$catalog_checksum" == "${plaintext_hash%% *}" && "$catalog_checksum" != "$envelope_hash" ]]
else
    [[ "$catalog_checksum" == "$envelope_hash" ]]
fi

damage_source() {
    docker exec -u 1000:1000 "$app" sh -ec '
        printf "mutated after backup\n" > "$1/payload"
        rm "$1/nested/value" "$1/nested-link"
    ' sh "$source"
}
tmp_entries() { docker exec "$app" find /tmp -mindepth 1 -print | LC_ALL=C sort; }
restore_and_verify() {
    local before after fingerprint payload layout
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
    after=$(tmp_entries)
    [[ "$after" == "$before" ]]
}

phase=restore-primary-before-sync
damage_source
restore_and_verify
[[ $(artifact_hashes "${primary_artifacts[@]}") == "$primary_hashes" ]]
entries=$(copies)
[[ -z "$entries" ]]
printf 'LOCAL %s primary restore before sync: exact fixture and format-specific catalog SHA-256 (%s, %s)\n' "$mode" "$arch" "$rev"

phase=explicit-sync
relative_key='strings | select(test("^[^/]+(/[^/]+)*$") and (split("/") | all(. != "." and . != "..")))'
key=$(api POST "/storage/$storage/sync/$backup" | jq -er ".remote_path | $relative_key")
local_copy() {
    jq -er --argjson storage "$storage" --argjson available "$1" --arg path "$2" '
        select(.local_available == $available and .remote_synced == true and (.file_path == (if $available then $path else null end))) |
        .remote_sync_details | select(length == 1) | .[0] |
        select(.storage_id == $storage and .status == "completed") | .remote_path | strings'
}
synced=$(api GET "/backups/$backup")
[[ $(local_copy true "$primary" <<< "$synced") == "$key" && $(catalog <<< "$synced") == "$original_catalog" ]]
sync_details=$(jq -cS '.remote_sync_details' <<< "$synced")
archive="$store/$key"
artifacts=("$archive")
if [[ "$encrypted" == true ]]; then
    [[ "$archive" == *.tar.gz.enc ]]
    artifacts+=("${archive%.enc}.key")
else
    [[ "$archive" == *.tar.gz ]]
fi
for i in "${!artifacts[@]}"; do
    docker exec -u 1000:1000 "$app" cmp -- "${primary_artifacts[i]}" "${artifacts[i]}"
done
[[ $(artifact_hashes "${primary_artifacts[@]}") == "$primary_hashes" ]]
hashes=$(artifact_hashes "${artifacts[@]}")
expected_copies=$(printf '%s\n' "${artifacts[@]}" | LC_ALL=C sort)
[[ $(copies) == "$expected_copies" ]]

phase=explicit-primary-removal
api DELETE "/backups/$backup?delete_local=true&delete_remote=false" |
    jq -e '.status == "completed" and .succeeded == 1 and .failed == 0 and .skipped == 0 and .retry_token == null' >/dev/null
replica_catalog() {
    local detail
    detail=$(api GET "/backups/$backup") || return 1
    [[ $(local_copy false '' <<< "$detail") == "$key" && $(catalog <<< "$detail") == "$original_catalog" &&
        $(jq -cS '.remote_sync_details' <<< "$detail") == "$sync_details" ]]
}
replica_catalog
[[ $(copies) == "$expected_copies" && $(artifact_hashes "${artifacts[@]}") == "$hashes" ]]
if [[ "$encrypted" == true ]]; then
    [[ $(primary_entries) == "${primary_artifacts[1]}" ]]
    docker exec -u 1000:1000 "$app" cmp -- "${primary_artifacts[1]}" "${artifacts[1]}"
    # The API retains the primary sidecar. Remove only this fixture to force replica-side key resolution.
    docker exec -u 1000:1000 "$app" rm -- "${primary_artifacts[1]}"
fi
entries=$(primary_entries)
[[ -z "$entries" ]]
printf 'LOCAL %s explicit sync and primary-only removal: byte-identical artifacts, retained replica and catalog (%s, %s)\n' "$mode" "$arch" "$rev"
damage_source

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
replica_catalog
entries=$(primary_entries)
[[ -z "$entries" && $(artifact_hashes "${artifacts[@]}") == "$hashes" ]]

phase=restore-replica-after-restart
restore_and_verify
unset identity
entries=$(primary_entries)
[[ -z "$entries" ]]
[[ $(copies) == "$expected_copies" && $(artifact_hashes "${artifacts[@]}") == "$hashes" ]]
replica_catalog
