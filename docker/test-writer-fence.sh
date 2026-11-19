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

# The fence must keep legacy startup recovery from finalizing a native backup
# before Rust has settled it. Seed only an isolated SQLite fixture.
fixture_db() {
    docker run --rm --pull=never --network none --volumes-from "$first" \
        --user 1000:1000 --entrypoint /opt/venv/bin/python "$rollback" -c '
import sqlite3, sys
with sqlite3.connect("/app/data/backup.db") as db:
    if len(sys.argv) > 1:
        db.execute(sys.argv[1])
    else:
        print(db.execute("SELECT status, file_path, backup_metadata FROM backups WHERE id=248").fetchone())
' "$@"
}
fixture_db "INSERT INTO backup_targets (id,name,target_type,enabled) VALUES (248,'Fence fixture','path',0)"
fixture_db "INSERT INTO backups (id,target_id,backup_type,status,backup_metadata) VALUES (248,248,'FULL','RUNNING',json_object('native_ephemeral_archive',json('true'),'active_artifacts',json_array('/backups/absent-fixture.tar.gz')))"
before_row=$(fixture_db)
refused() {
    start "$rollback"
    for attempt in $(seq 1 10); do
        [ "$(docker inspect -f '{{.State.Status}}' "$c")" = exited ] && break
        if docker exec "$c" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null 2>&1; then
            echo "Python became ready despite $1" >&2
            exit 1
        fi
        sleep 1
    done
    test "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$c")" = 'exited 74'
    docker logs "$c" 2>&1 | grep -F -- "$1"
}
refused 'backup work or archive publication is unsettled'
test "$(fixture_db)" = "$before_row"
start "$native"; healthy; stop "$c"
test "$(fixture_db)" = "('FAILED', None, '{}')"
start "$rollback"; healthy; stop "$c"

fixture_db "INSERT INTO workload_operations (kind,phase,state,resources,created_at) VALUES ('backup','restart','recovery_required','[]',CURRENT_TIMESTAMP)"
refused 'workload restart or recovery is unsettled'
fixture_db "UPDATE workload_operations SET state='finished'"
fixture_db 'INSERT INTO schema_migrations(version) VALUES (8)'
refused 'incompatible migration ledger'
printf 'Writer fence: exclusion, SIGKILL, journal refusal, and settled handoff passed\n'
