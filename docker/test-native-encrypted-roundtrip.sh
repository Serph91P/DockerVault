#!/usr/bin/env bash
# Exercise an authenticated native backup and independent, offline native recovery.
set -euo pipefail
app_image=${1:?Usage: bash docker/test-native-encrypted-roundtrip.sh APP_IMAGE RECOVERY_IMAGE}
recovery_image=${2:?Recovery image required}
app=
recovery=
cleanup() {
    if [[ -n "$recovery" ]]; then docker stop --time 1 "$recovery" >/dev/null || :; fi
    if [[ -n "$app" ]]; then docker rm -fv "$app" >/dev/null || :; fi
}
trap cleanup EXIT
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$app_image")
[[ "$rev" =~ ^[a-f0-9]{40}$ ]]
test "$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$recovery_image")" = "$rev"
test "$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$app_image")" = "$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$recovery_image")"
app=$(docker create --pull=never --network none "$app_image")
docker start "$app" >/dev/null
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null; then return; fi
        sleep 1
    done
    printf 'Native app did not become healthy\n' >&2
    return 1
}
ready

docker exec -u 1000:1000 "$app" sh -ec '
    umask 077
    mkdir /tmp/dv-encrypted-roundtrip
    printf "DockerVault encrypted candidate fixture\n" > /tmp/dv-encrypted-roundtrip/content
    password=$(tr -d "-" < /proc/sys/kernel/random/uuid)
    printf "{\"username\":\"roundtrip\",\"password\":\"%s\",\"confirm_password\":\"%s\"}" "$password" "$password" |
        curl -fsS --max-time 6 -c /tmp/dv-cookie -H "Content-Type: application/json" --data-binary @- http://127.0.0.1/api/v1/auth/setup >/dev/null
'
api() {
    local method=$1 path=$2
    shift 2
    docker exec -u 1000:1000 "$app" curl -fsS --max-time 8 -b /tmp/dv-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
recovery=$(docker run --detach --rm --pull=never --network none --read-only \
    --tmpfs /work:rw,uid=1000,gid=1000,mode=0700,size=16m \
    --user 1000:1000 --entrypoint /bin/sleep "$recovery_image" infinity)
# Only the fresh synthetic identity is passed over stdin; no response logging or host file.
api POST /encryption/setup --data '{}' | jq -er '.private_key | strings | select(startswith("AGE-SECRET-KEY-"))' | \
    docker exec -i "$recovery" /bin/sh -ec 'umask 077; cat > /work/identity; test -s /work/identity'
api POST /encryption/confirm-setup --data '{"confirmed":true}' >/dev/null
api GET /encryption/status | jq -e '.encryption_enabled and .setup_completed and (has("private_key") | not)' >/dev/null
plan=$(api POST /targets --data '{"name":"native-roundtrip","target_type":"path","host_path":"/tmp/dv-encrypted-roundtrip","compression_enabled":false}' | jq -er '.id | numbers')
backup=$(api POST /backups --data "{\"target_id\":$plan}" | jq -er '.id | numbers')
status=
for ((attempt=0; attempt<30; attempt++)); do
    status=$(api GET "/backups/$backup" | jq -er '.status')
    case "$status" in completed|failed) break;; esac
    sleep 1
done
test "$status" = completed || { printf 'Native backup did not complete: %s\n' "$status" >&2; exit 1; }

docker exec -u 1000:1000 "$app" sh -ec '
    printf "Source changed after backup\n" > /tmp/dv-encrypted-roundtrip/content
    test "$(cat /tmp/dv-encrypted-roundtrip/content)" = "Source changed after backup"
'
inode=$(docker exec "$app" stat -c %i /app/data/backup.db)
docker stop --time 30 "$app" >/dev/null
test "$(docker inspect -f '{{.State.ExitCode}}' "$app")" = 0
docker start "$app" >/dev/null
ready
test "$(docker exec "$app" stat -c %i /app/data/backup.db)" = "$inode"
api GET "/backups/$backup" | jq -e '.status == "completed" and .encrypted == true' >/dev/null

docker exec -u 1000:1000 "$app" sh -ec '
    cd /backups/native-roundtrip
    set -- *.tar.enc
    test "$#" = 1 && test -s "$1" && test -s "${1%.enc}.key"
    tar -cf - -- "$1" "${1%.enc}.key"
' | docker exec -i "$recovery" tar -xf - -C /work
# Stop the application before the recovery command. The recovery image has no data mount.
docker stop --time 30 "$app" >/dev/null
docker exec "$recovery" /bin/sh -ec '
    cd /work
    set -- *.tar.enc
    test "$#" = 1
    /usr/local/bin/dockervault-recover --identity identity --archive "$1" --key "${1%.enc}.key" --output restored.tar >/dev/null
    printf "DockerVault encrypted candidate fixture\n" > expected
    tar -xOf restored.tar dv-encrypted-roundtrip/content > actual
    cmp expected actual
'
printf 'Authenticated encrypted backup, database reopen and standalone offline recovery passed: %s\n' "$rev"
