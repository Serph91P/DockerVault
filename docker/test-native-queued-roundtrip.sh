#!/usr/bin/env bash
# Exercise four independent accepted jobs without exceeding physical I/O capacity.
set -euo pipefail
image=${1:?Native application image required}
app=$(docker create --pull never --network none --cpus 2 --memory 6g "$image")
trap 'docker rm -fv "$app" >/dev/null' EXIT
docker start "$app" >/dev/null
for attempt in {1..30}; do
    if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null; then break; fi
    sleep 1
done
docker exec -u 1000:1000 "$app" sh -ec '
    umask 077
    password=$(tr -d "-" </proc/sys/kernel/random/uuid)
    printf "{\"username\":\"queued\",\"password\":\"%s\",\"confirm_password\":\"%s\"}" "$password" "$password" |
        curl -fsS --max-time 8 -c /tmp/queue-cookie -H "Content-Type: application/json" --data-binary @- http://127.0.0.1/api/v1/auth/setup >/dev/null
    mkdir /app/data/queue-copies
    for i in 1 2 3 4; do
        mkdir "/app/data/queue-$i"
        dd if=/dev/urandom of="/app/data/queue-$i/payload" bs=1M count=8 status=none
        chmod 640 "/app/data/queue-$i/payload"
        touch -m -d @1700000000 "/app/data/queue-$i/payload"
    done
'
api() {
    local method=$1 path=$2
    shift 2
    docker exec -u 1000:1000 "$app" curl -fsS --max-time 90 -b /tmp/queue-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
store=$(api POST /storage --data '{"name":"queue-copies","storage_type":"local","base_path":"/app/data/queue-copies"}' | jq -er '.id | numbers')
plans=() backups=() hashes=()
for i in 1 2 3 4; do
    plans+=("$(api POST /targets --data "{\"name\":\"queue-$i\",\"target_type\":\"path\",\"host_path\":\"/app/data/queue-$i\",\"compression_enabled\":true,\"remote_storage_ids\":[$store]}" | jq -er '.id | numbers')")
    receipt=$(docker exec "$app" sha256sum "/app/data/queue-$i/payload")
    hashes+=("${receipt%% *}")
done
for plan in "${plans[@]}"; do backups+=("$(api POST /backups --data "{\"target_id\":$plan}" | jq -er '.id | numbers')"); done
for id in "${backups[@]}"; do
    ready=false
    for attempt in {1..120}; do
        item=$(api GET "/backups/$id")
        if ! jq -e '.status != "failed" and .status != "cancelled"' <<< "$item" >/dev/null; then
            printf 'Accepted backup %s reached an unsuccessful terminal state\n' "$id" >&2
            exit 1
        fi
        if jq -e '.status == "completed" and .remote_synced == true' <<< "$item" >/dev/null; then ready=true; break; fi
        sleep .2
    done
    if [[ $ready != true ]]; then
        printf 'Accepted backup %s did not settle its LOCAL copy\n' "$id" >&2
        exit 1
    fi
done
docker exec -u 1000:1000 "$app" sh -ec 'for i in 1 2 3 4; do printf changed > "/app/data/queue-$i/payload"; done'
pids=()
for id in "${backups[@]}"; do
    (api POST "/backups/$id/restore" --data '{}' | jq -e '.status == "restored"' >/dev/null) &
    pids+=("$!")
done
status=0
for pid in "${pids[@]}"; do wait "$pid" || status=1; done
[[ $status == 0 ]]
for i in 1 2 3 4; do
    path=/app/data/queue-$i/payload
    receipt=$(docker exec "$app" sha256sum "$path")
    [[ ${receipt%% *} == "${hashes[i-1]}" ]]
    [[ $(docker exec "$app" stat -c '%a %Y' "$path") == '640 1700000000' ]]
done
printf 'Four accepted backups synced and four concurrent restores matched bytes, mode and mtime\n'
