#!/usr/bin/env bash
# Run the independent browser consumer against the packaged app over isolated loopback.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-policy-browser.sh APP_IMAGE BROWSER_IMAGE}
browser_image=${2:?Browser image required}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
[[ "$rev" =~ ^[a-f0-9]{40}$ ]]
id=$(docker image inspect -f '{{.Id}}' "$image")
app=
browser=
cleanup() {
    if [[ -n "$browser" ]]; then docker rm -fv "$browser" >/dev/null; fi
    if [[ -n "$app" ]]; then docker stop --time 15 "$app" >/dev/null; fi
}
trap cleanup EXIT
app=$(docker run --rm --detach --pull=never --network none -e TZ=America/New_York "$id")
for ((attempt=0; attempt<30; attempt++)); do
    if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null; then break; fi
    sleep 1
done
test "$attempt" -lt 30
browser=$(docker create --pull=never --network "container:$app" --entrypoint node \
    "$browser_image" /runner/native-policy-browser.mjs "$id" "$rev")
docker cp "$(dirname "$0")/native-policy-browser.mjs" "$browser":/runner/native-policy-browser.mjs
docker start -a "$browser"
test "$(docker inspect -f '{{.State.ExitCode}}' "$browser")" = 0
