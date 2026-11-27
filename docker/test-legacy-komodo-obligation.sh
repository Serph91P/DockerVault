#!/bin/sh
# Synthetic compatibility case, not a released migration or a provider test.
set -eu
native=${1:?Usage: sh docker/test-legacy-komodo-obligation.sh NATIVE_IMAGE PYTHON_ROLLBACK_IMAGE}
rollback=${2:?Expected fenced Python rollback image}
containers=
cleanup() {
    for id in $containers; do docker rm -fv "$id" >/dev/null; done
}
trap cleanup EXIT
start() {
    c=$(docker create --pull=never --network none "$@")
    containers="$c $containers"
    docker start "$c" >/dev/null
}
logs_contain() { docker logs "$1" 2>&1 | grep -F -- "$2"; }
await_log() {
    for attempt in $(seq 1 30); do
        if logs_contain "$1" "$2"; then return; fi
        sleep 1
    done
    docker logs "$1" >&2
    return 1
}

start "$native"
seed=$c
await_log "$seed" 'DockerVault backend ready'
docker stop -t 30 "$seed" >/dev/null
test "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$seed")" = 'exited 0'

# The format comes from the unpublished 1110565 prototype. Extend the known
# ledger-7 catalog synthetically: ledger 8 would mask the journal check.
# The local Unix listener only records a connection and cannot perform actions.
start --volumes-from "$seed" --user 1000:1000 --entrypoint /opt/venv/bin/python "$rollback" -u -c '
import json, pathlib, socket, sqlite3

with sqlite3.connect("/app/data/backup.db") as db:
    assert [v for v, in db.execute("SELECT version FROM schema_migrations ORDER BY version")] == list(range(1, 8))
    db.executescript("""
        ALTER TABLE workload_operations ADD COLUMN authority VARCHAR(16) NOT NULL DEFAULT \x27docker\x27;
        ALTER TABLE workload_operations ADD COLUMN owner_id INTEGER;
        ALTER TABLE workload_operations ADD COLUMN request_id VARCHAR(36);
        CREATE UNIQUE INDEX fixture_request_id ON workload_operations(request_id);
        ALTER TABLE workload_operations ADD COLUMN context JSON NOT NULL DEFAULT \x27{}\x27;
        ALTER TABLE workload_operations ADD COLUMN cancel_requested BOOLEAN NOT NULL DEFAULT 0;
        ALTER TABLE backup_targets ADD COLUMN lifecycle_authority VARCHAR(16);
        ALTER TABLE backup_targets ADD COLUMN komodo_server_id VARCHAR(255);
        ALTER TABLE backup_targets ADD COLUMN komodo_stack_id VARCHAR(255);
    """)
    db.executemany("INSERT INTO app_settings(key,value) VALUES (?,?)", [
        ("komodo_enabled", "true"), ("komodo_api_url", "http://fixture.invalid"),
        ("komodo_api_key", "synthetic"), ("komodo_api_secret", "synthetic"),
    ])
    db.execute("""INSERT INTO backup_targets
        (id,name,target_type,enabled,lifecycle_authority,komodo_server_id,komodo_stack_id)
        VALUES (248,?,?,0,?,?,?)""",
        ("Archived binding", "path", "komodo", "fixture-server", "fixture-stack"))
    db.execute("INSERT INTO backups (id,target_id,backup_type,status,file_path) VALUES (248,248,?,?,?)",
        ("FULL", "COMPLETED", "/backups/retained.fixture"))
    context = dict(server_id="fixture-server", stack_id="fixture-stack",
                   containers={"fixture-service": "a" * 64}, action=None, revision=None,
                   commands=[dict(action="stop", services=["fixture-service"], update_id="fixture-update", settled=True)])
    resources = {"fixture-service": dict(original="running", requested=True, stopped=True, restored=False)}
    db.execute("""INSERT INTO workload_operations
        (id,backup_id,kind,authority,phase,state,resources,context,created_at)
        VALUES (248,248,\x27backup\x27,\x27komodo\x27,\x27stop_confirmed\x27,\x27active\x27,?,?,CURRENT_TIMESTAMP)""",
        (json.dumps(resources), json.dumps(context)))
pathlib.Path("/backups/retained.fixture").write_bytes(b"synthetic retained archive bytes\n")
with socket.socket(socket.AF_UNIX) as observer:
    observer.bind("/app/data/docker-observer.sock")
    observer.listen()
    print("Synthetic Docker observer ready")
    while True:
        conn, _ = observer.accept()
        print("Unexpected Docker connection")
        conn.close()
'
observer=$c
await_log "$observer" 'Synthetic Docker observer ready'
snapshot() {
    docker exec "$observer" /opt/venv/bin/python -c '
import hashlib, pathlib, sqlite3
with sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True) as db:
    db.execute("BEGIN")
    assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
    assert not db.execute("PRAGMA foreign_key_check").fetchall()
    print(hashlib.sha256("\n".join(db.iterdump()).encode()).hexdigest())
print(hashlib.sha256(pathlib.Path("/backups/retained.fixture").read_bytes()).hexdigest())
'
}
before=$(snapshot)

start --volumes-from "$seed" --env DOCKER_SOCKET=/app/data/docker-observer.sock "$native"
rust=$c
# s6 can retry a failed backend, so wait for the startup decision, then stop it.
await_log "$rust" 'native startup or shutdown failed'
if docker exec "$rust" curl -fsS --max-time 2 http://127.0.0.1/health; then
    echo 'Native backend became healthy with an unresolved obligation' >&2
    exit 1
fi
docker stop -t 30 "$rust" >/dev/null
test "$(docker inspect -f '{{.State.Status}}' "$rust")" = exited
if logs_contain "$observer" 'Unexpected Docker connection'; then
    echo 'Native recovery treated the legacy journal as Docker work' >&2
    exit 1
fi
logs_contain "$rust" 'unsettled legacy workload authority'
test "$(snapshot)" = "$before"

start --volumes-from "$seed" "$rollback"
for attempt in $(seq 1 15); do
    test "$(docker inspect -f '{{.State.Status}}' "$c")" != exited || break
    sleep 1
done
test "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$c")" = 'exited 74'
logs_contain "$c" 'workload restart or recovery is unsettled'
test "$(snapshot)" = "$before"

printf 'Legacy Komodo obligation retained; no Docker connection; fenced rollback refused (74)\n'
docker image inspect -f '{{.Os}}/{{.Architecture}} {{.Id}} {{index .Config.Labels "org.opencontainers.image.revision"}}' "$native" "$rollback"
printf 'Unchanged SQLite dump and retained archive SHA-256 (in order):\n%s\n' "$before"
