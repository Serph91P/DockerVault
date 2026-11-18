#!/bin/sh
# Real entrypoints, one disposable local data mount, no published ports or socket.
set -eu
native=${1:?Usage: sh docker/test-writer-fence.sh NATIVE_IMAGE PYTHON_ROLLBACK_IMAGE}
rollback=${2:?Expected Python rollback image}
first=$(docker create --pull=never --network none "$native")
containers=$first
cleanup() {
    for c in $containers; do docker rm -fv "$c" >/dev/null; done
}
trap cleanup EXIT
start() {
    c=$(docker create --pull=never --network none --volumes-from "$first" "$1")
    containers="$c $containers"
    docker start "$c" >/dev/null
}
healthy() {
    for attempt in $(seq 1 30); do
        if docker exec "$c" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null 2>&1; then
            return
        fi
        sleep 1
    done
    docker logs "$c" >&2
    return 1
}
conflict() {
    start "$1"
    for attempt in $(seq 1 10); do
        if [ "$(docker inspect -f '{{.State.Status}}' "$c")" = exited ]; then
            test "$(docker inspect -f '{{.State.ExitCode}}' "$c")" = 73
            return
        fi
        if docker exec "$c" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null 2>&1; then
            echo 'Concurrent writer became ready' >&2
            return 1
        fi
        sleep 1
    done
    docker logs "$c" >&2
    echo 'Conflicting writer did not exit' >&2
    return 1
}
identity() { docker exec "$1" stat -c '%d:%i' /app/data /app/data/backup.db; }
stop() {
    docker stop -t 30 "$1" >/dev/null
    test "$(docker inspect -f '{{.State.ExitCode}}' "$1")" = 0
}

c=$first
docker start "$first" >/dev/null
healthy
before=$(identity "$first")
conflict "$rollback"
conflict "$native"
stop "$first"
start "$rollback"; healthy; second=$c
conflict "$native"
conflict "$rollback"
test "$(identity "$second")" = "$before"
stop "$second"
start "$native"; healthy; third=$c
docker kill --signal KILL "$third" >/dev/null
test "$(docker wait "$third")" = 137
start "$rollback"; healthy
holder=$c
test "$(identity "$holder")" = "$before"
conflict "$native"
stop "$holder"
printf 'Writer fence: conflicts, clean stop and container SIGKILL passed; data and DB identities unchanged\n'
