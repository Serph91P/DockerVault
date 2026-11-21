#!/usr/bin/env bash
# Test an S3-only restore through the packaged HTTP service, with no host mounts.
set -euo pipefail
app_image=${1:?Usage: bash docker/test-native-s3-restart.sh APP_IMAGE SEAWEEDFS_IMAGE}
provider_image=${2:?Pinned SeaweedFS image required}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$app_image")
[[ "$rev" =~ ^[a-f0-9]{40}$ ]]
app=
provider=
cleanup() {
    if [[ -n "$app" ]]; then docker rm -fv "$app" >/dev/null; fi
    if [[ -n "$provider" ]]; then docker rm -fv "$provider" >/dev/null; fi
}
trap cleanup EXIT
provider=$(docker create --pull=never --network none -e AWS_ACCESS_KEY_ID=fixture-only \
    -e AWS_SECRET_ACCESS_KEY=public-test-fixture-only "$provider_image" \
    mini -dir=/data -bucket=dv-restart-fixture -master.telemetry=false)
docker start "$provider" >/dev/null
# Only the provider's isolated loopback namespace is shared; no egress or ports.
app=$(docker create --pull=never --network "container:$provider" "$app_image")
docker start "$app" >/dev/null
ready() {
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null 2>&1; then return; fi
        sleep 1
    done
    printf 'Native app did not become healthy\n' >&2
    return 1
}
api() {
    local method=$1 path=$2
    shift 2
    docker exec -i -u 1000:1000 "$app" curl -sS --max-time 10 -b /tmp/dv-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
s3() {
    local path
    path=$(jq -nr --arg path "$1" '$path | split("/") | map(@uri) | join("/")')
    docker exec "$app" curl -fsS --max-time 10 --aws-sigv4 aws:amz:us-east-1:s3 \
        --user fixture-only:public-test-fixture-only "http://127.0.0.1:8333/dv-restart-fixture$path"
}
ready
for ((attempt=0; attempt<30; attempt++)); do
    if s3 '' >/dev/null 2>&1; then break; fi
    sleep 1
done
s3 '' >/dev/null

docker exec -u 1000:1000 "$app" sh -ec '
    umask 077
    password=$(tr -d "-" < /proc/sys/kernel/random/uuid)
    printf "{\"username\":\"s3-roundtrip\",\"password\":\"%s\",\"confirm_password\":\"%s\"}" "$password" "$password" |
        curl -fsS --max-time 6 -c /tmp/dv-cookie -H "Content-Type: application/json" --data-binary @- http://127.0.0.1/api/v1/auth/setup >/dev/null
'
storage=$(api POST /storage --fail --data '{"name":"S3 fixture","storage_type":"s3","s3_bucket":"dv-restart-fixture","s3_region":"us-east-1","s3_endpoint_url":"http://127.0.0.1:8333","s3_access_key":"fixture-only","s3_secret_key":"public-test-fixture-only","base_path":"fixture-prefix","enabled":true}' | jq -er '.id | numbers')

for mode in plain encrypted; do
    source="/tmp/dv-s3-$mode"
    docker exec -u 1000:1000 "$app" sh -ec '
        umask 077; mkdir "$1"; printf "S3-only fixture\n" > "$1/content"
        chmod 640 "$1/content"; touch -d @1700000000 "$1/content"
    ' sh "$source"
    if [[ "$mode" == encrypted ]]; then
        # The synthetic identity stays in memory and goes to restore over stdin only.
        identity=$(api POST /encryption/setup --fail --data '{}' | jq -er '.private_key | strings | select(startswith("AGE-SECRET-KEY-"))')
        api POST /encryption/confirm-setup --fail --data '{"confirmed":true}' >/dev/null
    fi
    plan=$(jq -nc --arg name "s3-$mode" --arg path "$source" --argjson id "$storage" \
        '{name:$name,target_type:"path",host_path:$path,compression_enabled:false,remote_storage_ids:[$id],delete_local_after_sync:true}' |
        api POST /targets --fail --data-binary @- | jq -er '.id | numbers')
    backup=$(api POST /backups --fail --data "{\"target_id\":$plan}" | jq -er '.id | numbers')
    entry() {
        api GET "/backups/$backup" --fail | jq -er --argjson storage "$storage" \
            --argjson encrypted "$([[ "$mode" == encrypted ]] && printf true || printf false)" '
                select(.status == "completed" and .encrypted == $encrypted and .local_available == false and .remote_synced) |
                .remote_sync_details | select(length == 1) | .[0] |
                select(.storage_id == $storage and .status == "completed") | .remote_path | strings | select(length > 0)'
    }
    for ((attempt=0; attempt<30; attempt++)); do
        if key=$(entry); then break; fi
        sleep 1
    done
    test -n "$key"

    object="/fixture-prefix/$key"
    archive_sum=$(s3 "$object" | sha256sum)
    if [[ "$mode" == encrypted ]]; then
        sidecar_sum=$(s3 "${object%.enc}.key" | sha256sum)
    fi
    docker exec "$app" sh -ec 'test -z "$(find /backups -type f -print -quit)"'
    docker exec -u 1000:1000 "$app" sh -ec 'printf "mutated\n" > "$1/content"' sh "$source"
    inode=$(docker exec "$app" stat -c '%d:%i' /app/data/backup.db)
    docker stop --time 30 "$app" >/dev/null
    test "$(docker inspect -f '{{.State.ExitCode}}' "$app")" = 0
    docker start "$app" >/dev/null
    ready
    test "$(docker exec "$app" stat -c '%d:%i' /app/data/backup.db)" = "$inode"
    test "$(entry)" = "$key"
    fingerprint=$(api POST "/backups/$backup/restore-review" --fail --data '{"destination_type":"original"}' | jq -er '.fingerprint | strings')
    payload=$(jq -nc --arg fingerprint "$fingerprint" '{destination_type:"original",review_fingerprint:$fingerprint}')
    if [[ "$mode" == encrypted ]]; then
        test "$(printf '%s' "$payload" | api POST "/backups/$backup/restore" --data-binary @- -o /dev/null -w '%{http_code}')" = 400
        test "$(docker exec "$app" cat "$source/content")" = mutated
        payload=$(printf '%s\n%s\n' "$payload" "$identity" | jq -Rsc 'split("\n") | (.[0] | fromjson) + {private_key:.[1]}')
    fi
    printf '%s' "$payload" | api POST "/backups/$backup/restore" --fail --data-binary @- >/dev/null
    unset identity payload
    docker exec -u 1000:1000 "$app" sh -ec '
        printf "S3-only fixture\n" | cmp - "$1/content"
        test "$(stat -c "%a:%Y" "$1/content")" = 640:1700000000
        test -z "$(find /backups -type f -print -quit)"
    ' sh "$source"
    test "$(s3 "$object" | sha256sum)" = "$archive_sum"
    if [[ "$mode" == encrypted ]]; then
        test "$(s3 "${object%.enc}.key" | sha256sum)" = "$sidecar_sum"
    fi
    printf 'S3-only %s restore after restart: bytes, mode, mtime, unchanged provider object(s) (%s)\n' "$mode" "$rev"
done
