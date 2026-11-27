#!/usr/bin/env bash
# Exercise the unchanged browser oracle and independently check its persisted effects.
set -euo pipefail
image=${1:?Native application image required}
browser_image=${2:?Browser image required}
root=$(cd "$(dirname "$0")/.." && pwd)
app= browser= verifier=
cleanup() {
    for container in "$verifier" "$browser" "$app"; do
        if [[ -n "$container" ]]; then docker rm -fv "$container" >/dev/null; fi
    done
}
trap cleanup EXIT
app=$(docker create --pull never --network none -e COOKIE_SECURE=false "$image")
docker start "$app" >/dev/null
healthy=false
for ((attempt=0; attempt<90; attempt++)); do
    if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health >/dev/null 2>&1; then
        healthy=true
        break
    fi
    sleep .5
done
[[ "$healthy" == true ]] || { printf 'Isolated package did not become healthy\n' >&2; exit 1; }
docker exec -u 1000:1000 "$app" sh -ec '
    mkdir /app/data/ux183-source /app/data/ux183-remote
    printf "owned truthful states fixture" > /app/data/ux183-source/data.txt
    printf "owned disposable file" > /backups/notification-delete
'
browser=$(docker create --pull never --network "container:$app" --entrypoint sh "$browser_image" -ec '
    test -s /runner/node_modules/axe-core/axe.min.js
    ln -s /runner/node_modules/axe-core/axe.min.js /runner/axe.min.js
    exec node /runner/browser_truthful_states.mjs
')
docker cp "$root/backend/tests/browser_truthful_states.mjs" "$browser:/runner/browser_truthful_states.mjs"
timeout 900 docker start -a "$browser"
test "$(docker inspect -f '{{.State.ExitCode}}' "$browser")" = 0
docker stop --time 15 "$app" >/dev/null
test "$(docker inspect -f '{{.State.ExitCode}}' "$app")" = 0
verifier=$(docker create --pull never --network none --volumes-from "$app:ro" --user 1000:1000 --entrypoint node "$browser_image" -e '
    const assert = require("node:assert/strict");
    const { readFileSync, statSync } = require("node:fs");
    const { DatabaseSync } = require("node:sqlite");
    const db = new DatabaseSync("/app/data/backup.db", { readOnly: true });
    assert.equal(db.prepare("PRAGMA foreign_key_check").all().length, 0);
    assert.equal(db.prepare("SELECT count(*) AS n FROM backups WHERE status = ?").get("COMPLETED").n, 61);
    assert.ok(db.prepare("SELECT count(*) AS n FROM storage_operations WHERE state = ?").get("completed").n >= 1);
    db.close();
    for (const name of ["truthful_key", "truthful_key.pub"]) {
        assert.ok(statSync(`/app/data/ssh_keys/${name}`).isFile());
    }
    assert.throws(() => statSync("/backups/notification-delete"), { code: "ENOENT" });
    assert.deepEqual(readFileSync("/app/data/ux183-source/data.txt"), Buffer.from("owned truthful states fixture"));
    console.log("Truthful states persisted effects: SQLite FK, 61 backups, operation, SSH files, deletion and bytes verified.");
')
docker start -a "$verifier"
test "$(docker inspect -f '{{.State.ExitCode}}' "$verifier")" = 0
