#!/usr/bin/env bash
# Deliberately separate the pre-write and post-write Python handoff checks.
set -euo pipefail
shopt -s inherit_errexit
native=${1:?native application}
python=${2:?fenced Python application}
mode=${3:-plain}
case "$mode" in plain|encrypted) ;; *) printf 'Expected plain or encrypted\n' >&2; exit 2;; esac
set +x # The synthetic identity must not appear in traces.
label=dockervault.fixture=run796-rollback-success
containers=()
cleanup() { for c in "${containers[@]}"; do docker rm -fv "$c" >/dev/null; done; }
trap cleanup EXIT
create() {
    app=$(docker create --pull=never --network none --label "$label" "$@")
    containers=("$app" "${containers[@]}")
}
start() {
    docker start "$app" >/dev/null
    for ((i=0;i<50;i++)); do
        if docker exec "$app" curl -fsS --max-time 1 http://127.0.0.1/health >/dev/null 2>&1; then return; fi
        sleep .2
    done
    return 1
}
stop() { docker stop -t 30 "$app" >/dev/null; [[ $(docker inspect -f '{{.State.ExitCode}}' "$app") == 0 ]]; }
identity() { docker exec "$app" stat -c '%d:%i' /app/data/backup.db; }
api() {
    local method=$1 route=$2; shift 2
    docker exec -u 1000:1000 "$app" curl --fail-with-body -sS --max-time 45 -b /tmp/rollback-cookie -c /tmp/rollback-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$route"
}
login() {
    api POST /auth/login --data '{"username":"handoff-fixture","password":"synthetic-rollback-fixture-only"}' >/dev/null
    api GET /auth/me | jq -e '.username == "handoff-fixture"' >/dev/null
}
prepare_source() {
    docker exec -u 1000:1000 "$app" sh -ec '
        mkdir -p "$1"; printf "%s" "$2" > "$1/payload";
        chmod 640 "$1/payload"; touch -m -d @1700000000 "$1/payload"
    ' sh "/app/data/$1" "$2"
}
make_backup() {
    local plan id row
    docker exec -u 1000:1000 "$app" test -f "/app/data/$1/payload"
    row=$(api POST /targets --data "{\"name\":\"$1\",\"target_type\":\"path\",\"host_path\":\"/app/data/$1\",\"stop_container\":false,\"compression_enabled\":true}") || { jq -c '{detail}' <<< "$row" >&2; return 1; }
    plan=$(jq -er '.id | numbers' <<< "$row")
    id=$(api POST /backups --data "{\"target_id\":$plan}" | jq -er '.id | numbers')
    for ((i=0;i<150;i++)); do
        row=$(api GET "/backups/$id")
        jq -e '.status != "failed" and .status != "cancelled"' <<< "$row" >/dev/null
        if jq -e '.status == "completed"' <<< "$row" >/dev/null; then printf '%s\n' "$row"; return; fi
        sleep .2
    done
    return 1
}
restore() {
    local row=$1 source=$2 value=$3 id path
    id=$(jq -er '.id' <<< "$row"); path="/app/data/$source/payload"
    docker exec -u 1000:1000 "$app" sh -ec 'printf changed > "$1"' sh "$path"
    if [[ "$mode" == encrypted ]]; then
        printf '{"private_key":"%s"}' "$identity_key" |
            docker exec -i -u 1000:1000 "$app" curl -fsS --max-time 45 -b /tmp/rollback-cookie \
                -H 'Content-Type: application/json' --data-binary @- "http://127.0.0.1/api/v1/backups/$id/restore" |
            jq -e '.status == "restored"' >/dev/null
    else
        api POST "/backups/$id/restore" --data '{}' | jq -e '.status == "restored"' >/dev/null
    fi
    [[ $(docker exec "$app" sh -ec 'cat "$1"; printf "\n"; stat -c "%a %Y" "$1"' sh "$path") == "$(printf '%s\n640 1700000000' "$value")" ]]
}
read_catalog() {
    local id file
    id=$(jq -er '.id' <<< "$1"); file=$(jq -er '.file_path' <<< "$1")
    if [[ "$mode" == encrypted ]]; then
        [[ $(api GET "/backups/$id" | jq -er 'select(.status == "completed" and .encrypted == true) | .file_path') == "$file" ]]
        [[ "$file" == *.enc ]]
        docker exec "$app" sha256sum "$file" "${file%.enc}.key"
    else
        [[ $(api GET "/backups/$id" | jq -er 'select(.status == "completed") | .file_path') == "$file" ]]
        docker exec "$app" sha256sum "$file"
    fi
}
restore_journal_snapshot() {
    # Inspect the stopped fixture with the legacy runtime's SQLite, not its app startup.
    docker run --rm --interactive --pull=never --network none --read-only \
        --volumes-from "$app:ro" --user 1000:1000 --entrypoint /opt/venv/bin/python "$python" -IB - "$1" <<'PY'
import json
import sqlite3
import sys

with sqlite3.connect("file:/app/data/backup.db?mode=ro", uri=True) as db:
    ledger = db.execute("SELECT version, applied_at FROM schema_migrations ORDER BY version").fetchall()
    assert [version for version, _ in ledger] == list(range(1, 8)), "Unexpected migration ledger"
    rows = db.execute(
        "SELECT id, state, resources FROM workload_operations WHERE kind = 'restore' AND backup_id = ? LIMIT 2",
        (int(sys.argv[1]),),
    ).fetchall()
    assert len(rows) == 1, "Expected one journal record for the completed Rust restore"
    operation, state, resources = rows[0]
    assert state == "finished", "Completed Rust restore must be settled before handoff"
    assert json.loads(resources) == {}, "A file restore must not introduce legacy workload entries"
    print(json.dumps([operation, ledger]))
PY
}
create "$native"; seed=$app; start; stop
create --volumes-from "$seed" "$python"; start
api POST /auth/setup --data '{"username":"handoff-fixture","password":"synthetic-rollback-fixture-only","confirm_password":"synthetic-rollback-fixture-only"}' >/dev/null
login; original_inode=$(identity)
if [[ "$mode" == encrypted ]]; then
    identity_key=$(api POST /encryption/setup --data '{}' | jq -er '.private_key | strings | select(test("^AGE-SECRET-KEY-[A-Z0-9]+$"))')
    api POST /encryption/confirm-setup --data '{"confirmed":true}' >/dev/null
    api GET /encryption/status | jq -e '.encryption_enabled and .setup_completed and (has("private_key") | not)' >/dev/null
fi
prepare_source python-seed 'original Python bytes'
legacy_row=$(make_backup python-seed); legacy_sha=$(read_catalog "$legacy_row")
stop
create --volumes-from "$seed" "$native"; start; login
[[ $(identity) == "$original_inode" && $(read_catalog "$legacy_row") == "$legacy_sha" ]]
stop
create --volumes-from "$seed" "$python"; start; login
[[ $(identity) == "$original_inode" && $(read_catalog "$legacy_row") == "$legacy_sha" ]]
printf '%s before Rust backup or restore: Python -> Rust catalog read -> Python on the same SQLite inode and archive digest(s)\n' "$mode"
stop
create --volumes-from "$seed" "$native"; start; login
[[ $(identity) == "$original_inode" ]]
restore "$legacy_row" python-seed 'original Python bytes'
prepare_source rust-written 'original Rust bytes'
rust_row=$(make_backup rust-written); rust_sha=$(read_catalog "$rust_row")
stop
legacy_id=$(jq -er '.id | numbers' <<< "$legacy_row")
rust_journal=$(restore_journal_snapshot "$legacy_id")
create --volumes-from "$seed" "$python"; start; login
[[ $(identity) == "$original_inode" && $(read_catalog "$legacy_row") == "$legacy_sha" && $(read_catalog "$rust_row") == "$rust_sha" ]]
restore "$rust_row" rust-written 'original Rust bytes'
stop; start; login
[[ $(identity) == "$original_inode" && $(read_catalog "$rust_row") == "$rust_sha" ]]
restore "$rust_row" rust-written 'original Rust bytes'
stop
[[ $(restore_journal_snapshot "$legacy_id") == "$rust_journal" ]]
printf '%s post-write: legacy SQLite read the same finished Rust restore and unchanged migration ledger after handoff\n' "$mode"
printf '%s post-write: Python -> Rust restore and committed backup -> Python restore and restart/re-restore: same SQLite inode and archive digest(s), bytes/mode/mtime match\n' "$mode"
