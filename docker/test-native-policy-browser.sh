#!/usr/bin/env bash
# Run the independent browser consumer against the packaged app over isolated loopback.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-policy-browser.sh APP_IMAGE BROWSER_IMAGE [policy | native [RECOVERY_IMAGE DEV_IMAGE] | python NATIVE_SEED_IMAGE]}
browser_image=${2:?Browser image required}
mode=${3:-policy}
offline_image=
capture=
case "$mode" in
    policy) script=native-policy-browser.mjs ;;
    native)
        script=recovery-download-browser.mjs
        offline_image=${4:-}
        if [[ -n "$offline_image" ]]; then capture=offline; fi
        ;;
    python) script=recovery-download-browser.mjs ;;
    *) exit 2 ;;
esac
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
[[ "$rev" =~ ^[a-f0-9]{40}$ ]]
id=$(docker image inspect -f '{{.Id}}' "$image")
app=
browser=
seed=
cleanup() {
    if [[ -n "$browser" ]]; then docker rm -fv "$browser" >/dev/null; fi
    if [[ -n "$app" ]]; then docker rm -fv "$app" >/dev/null; fi
    if [[ -n "$seed" ]]; then docker rm -fv "$seed" >/dev/null; fi
}
trap cleanup EXIT
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$1" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null; then return; fi
        if [[ $(docker inspect -f '{{.State.Running}}' "$1") != true ]]; then break; fi
        sleep 1
    done
    docker logs "$1" >&2
    return 1
}
volumes=()
if [[ "$mode" == python ]]; then
    seed_image=${4:?Native seed image required for fenced Python}
    test "$(docker image inspect -f '{{index .Config.Labels "io.dockervault.cutover.role"}}' "$id")" = fenced-python-rollback
    seed=$(docker create --pull=never --network none "$seed_image")
    docker start "$seed" >/dev/null
    ready "$seed"
    inode=$(docker exec "$seed" stat -c %d:%i /app/data/backup.db)
    docker stop --time 15 "$seed" >/dev/null
    test "$(docker inspect -f '{{.State.Running}}:{{.State.ExitCode}}' "$seed")" = false:0
    volumes=(--volumes-from "$seed")
fi
app=$(docker create --pull=never --network none -e TZ=America/New_York "${volumes[@]}" "$id")
docker start "$app" >/dev/null
ready "$app"
if [[ "$mode" == policy ]]; then
    orphan=/backups/orphan-browser-fixture.tar
    docker exec -u 1000:1000 "$app" sh -ec 'umask 077; printf "unregistered synthetic archive\n" > "$1"' sh "$orphan"
    orphan_state() { docker exec "$app" sh -ec 'sha256sum "$1"; stat -c %i:%s:%a "$1"' sh "$orphan"; }
    original_orphan=$(orphan_state)
fi
if [[ "$mode" == python ]]; then
    test "$(docker exec "$app" stat -c %d:%i /app/data/backup.db)" = "$inode"
fi
browser=$(docker create --pull=never --network "container:$app" --entrypoint node \
    "$browser_image" "/runner/$script" "$id" "$rev" "$mode" "$(docker image inspect -f '{{.Architecture}}' "$id")" "$capture")
docker cp "$(dirname "$0")/$script" "$browser:/runner/$script"
docker start -a "$browser"
test "$(docker inspect -f '{{.State.ExitCode}}' "$browser")" = 0
if [[ "$mode" == policy ]]; then
    test "$(orphan_state)" = "$original_orphan"
fi
if [[ "$mode" == python ]]; then
    test "$(docker exec "$app" stat -c %d:%i /app/data/backup.db)" = "$inode"
fi
docker stop --time 15 "$app" >/dev/null
if [[ -n "$offline_image" ]]; then
    test "$(docker inspect -f '{{.State.Running}}:{{.State.ExitCode}}' "$app")" = false:0
    bash "$(dirname "$0")/test-native-recovery-instructions.sh" "$browser" "$offline_image" "${5:?Rust development image required}" "$rev"
    if [[ -n ${DV_BROWSER_RECEIPT:-} ]]; then
        docker cp "$browser:/tmp/dockervault-recovery-evidence.json" - | tar -xO | \
            jq -e --arg config "$id" '{binary_sha256:.sha256, manifest_sha256, $config}' > "$DV_BROWSER_RECEIPT"
    fi
fi
