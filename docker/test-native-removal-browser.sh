#!/usr/bin/env bash
# One encrypted archive, one replica, one failed-only Chromium retry.
set -euo pipefail
image=${1:?Native application image required}
browser_image=${2:?Browser image required}
[[ ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || {
    printf 'Run this test only on the GitHub-hosted CI matrix\n' >&2
    exit 2
}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
arch=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$image")
[[ "$rev" == "$(git rev-parse HEAD)" && ( "$arch" == linux/amd64 || "$arch" == linux/arm64 ) ]]
[[ $(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$browser_image") == "$arch" ]]
app= runner=
cleanup() {
    if [[ -n "$runner" ]]; then docker rm -fv "$runner" >/dev/null; fi
    if [[ -n "$app" ]]; then docker rm -fv "$app" >/dev/null; fi
}
trap cleanup EXIT
app=$(docker create --pull never --network none -e COOKIE_SECURE=false "$image")
docker start "$app" >/dev/null
service=/run/dockervault/services/backend
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health 2>/dev/null | \
            jq -e '.status == "healthy"' >/dev/null; then return; fi
        sleep 1
    done
    return 1
}
export DV_FIXTURE_PASSWORD
DV_FIXTURE_PASSWORD=$(tr -d '-' </proc/sys/kernel/random/uuid)
run_phase() {
    runner=$(docker create --pull never --network "container:$app" --volumes-from "$app" \
        --user 1000:1000 --env HOME=/tmp --env DV_FIXTURE_PASSWORD --entrypoint node \
        "$browser_image" /runner/native-removal-browser.mjs "$1")
    docker cp "$(dirname "$0")/native-removal-browser.mjs" "$runner:/runner/native-removal-browser.mjs"
    timeout 120 docker start -a "$runner"
    [[ $(docker inspect -f '{{.State.ExitCode}}' "$runner") == 0 ]]
    docker rm -v "$runner" >/dev/null
    runner=
}
ready
run_phase seed
old_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$old_pid" =~ ^[0-9]+$ && "$old_pid" -gt 1 ]]
docker exec "$app" s6-svc -wD -T 30000 -d "$service"
[[ $(docker exec "$app" s6-svstat -o up,exitcode "$service") == 'false 0' ]]
docker exec "$app" test ! -e "/proc/$old_pid"
docker exec "$app" s6-svc -wu -T 30000 -u "$service"
ready
new_pid=$(docker exec "$app" s6-svstat -o pid "$service")
[[ "$new_pid" =~ ^[0-9]+$ && "$new_pid" -gt 1 && "$new_pid" != "$old_pid" ]]
printf 'Copy removal clean backend restart: %s -> %s, old exit=0 (%s, %s)\n' "$old_pid" "$new_pid" "$arch" "$rev"
run_phase browser
unset DV_FIXTURE_PASSWORD
docker stop --time 30 "$app" >/dev/null
[[ $(docker inspect -f '{{.State.ExitCode}}' "$app") == 0 ]]
