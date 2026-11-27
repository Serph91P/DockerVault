#!/bin/sh
# An isolated image smoke: no published ports, host mounts, or Docker socket.
set -eu
image=${1:?Usage: sh docker/test-native-candidate.sh IMAGE BROWSER_IMAGE}
browser_image=${2:?Browser acceptance image with Node SQLite required}
container=$(docker create --network none "$image")
trap 'docker rm -fv "$container" >/dev/null' EXIT

docker start "$container" >/dev/null
ready() {
    for attempt in $(seq 1 30); do
        if docker exec "$container" curl -fsS --max-time 2 http://127.0.0.1/health; then
            printf '\n'
            return
        fi
        sleep 1
    done
    docker logs "$container" >&2
    return 1
}
ready
docker exec "$container" sh -eu -c '
    test "$(cat /proc/1/comm)" = s6-svscan || { echo "Expected s6 PID 1" >&2; exit 1; }
    pid=$(s6-svstat -o pid /run/dockervault/services/backend)
    test "$(awk "/^Uid:/ { print \$2, \$3 }" /proc/$pid/status)" = "1000 1000"
    test "$(stat -c %u /app/data/backup.db)" = 1000
    test "$(stat -c %u:%a /app/data/ssh_known_hosts)" = 1000:600
    test "$(stat -c %u:%a /app/data/ssh_keys)" = 1000:700
    curl -fsS http://127.0.0.1/ >/dev/null
    test "$(rclone version | sed -n "1p")" = "rclone v1.72.1"
    rclone --config /dev/null --ask-password=false config providers >/dev/null
    for cmd in python python3 uvicorn supervisord; do
        if command -v "$cmd"; then exit 1; fi
    done
    test "$(curl -sS -o /dev/null -w "%{http_code}" http://127.0.0.1/recover_backup.py)" = 404
    for file in /usr/local/sbin/dockervault-init /etc/dockervault/services/*/run \
        /usr/local/libexec/dockervault-process-guard /usr/local/libexec/dockervault-archive-worker /usr/local/bin/dockervault-recover; do
        test "$(stat -c %u:%g:%a "$file")" = 0:0:755
    done
'
# Fetch the advertised native executable through nginx, not directly from the image.
arch=$(docker image inspect --format '{{.Architecture}}' "$image")
revision=$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
docker exec -u dockervault "$container" sh -eu -c '
    cd /tmp
    curl -fsS http://127.0.0.1/recovery-tools/manifest.json -o recovery-manifest.json
    curl -fsS http://127.0.0.1/recovery-tools/dockervault-recover -o downloaded-recovery
    cmp downloaded-recovery /usr/local/bin/dockervault-recover
    hash=$(sha256sum downloaded-recovery); hash=${hash%% *}
    expected=$(printf "{\"os\":\"linux\",\"arch\":\"%s\",\"revision\":\"%s\",\"file\":\"dockervault-recover\",\"sha256\":\"%s\"}" "$1" "$2" "$hash")
    test "$(cat recovery-manifest.json)" = "$expected"
    test "$(curl -sS -o /dev/null -w "%{http_code}" http://127.0.0.1/recovery-tools/missing)" = 404
    chmod 700 downloaded-recovery
    env -i ./downloaded-recovery --help >/dev/null
' sh "$arch" "$revision"
# Reading /proc/PID/exe needs the service UID under Docker's default capabilities.
pid=$(docker exec "$container" s6-svstat -o pid /run/dockervault/services/backend)
test "$(docker exec -u 1000:1000 "$container" readlink "/proc/$pid/exe")" = /usr/local/bin/dockervault-service
docker exec -u dockervault "$container" sh -eu -c '
    /usr/local/bin/dockervault-recover --help >/dev/null
    /usr/local/libexec/dockervault-process-guard "$$" /usr/bin/id -u > /tmp/guard-uid &
    wait "$!"
    test "$(cat /tmp/guard-uid)" = 1000
'
docker exec "$container" sh -eu -c '
    umask 077
    mkdir /tmp/archive-smoke
    printf "candidate fixture\n" >/tmp/archive-smoke/content
    request="{\"version\":1,\"sources\":[[\"fixture\",\"/tmp/archive-smoke\"]],\"dest\":\"/backups/candidate-smoke.tar\",\"compress\":false,\"allowed_sources\":[\"/tmp/archive-smoke\"]}"
    if printf "%s" "$request" | s6-setuidgid dockervault /usr/local/libexec/dockervault-archive-worker; then
        exit 1
    fi
    test ! -e /backups/candidate-smoke.tar
    printf "%s" "$request" | /usr/local/libexec/dockervault-archive-worker
    test "$(tar -xOf /backups/candidate-smoke.tar fixture/content)" = "candidate fixture"
    stat -c %i /app/data/backup.db >/app/data/smoke-db-inode
'
sqlite_ledger() {
    docker run --rm --pull never --network none --read-only \
        --volumes-from "$container:ro" --user 1000:1000 --entrypoint node "$browser_image" -e '
        const assert = require("node:assert/strict");
        const { DatabaseSync } = require("node:sqlite");
        const db = new DatabaseSync("/app/data/backup.db", { readOnly: true });
        assert.equal(db.prepare("SELECT 1 AS value").get().value, 1);
        const ledger = db.prepare("SELECT version, applied_at FROM schema_migrations ORDER BY version").all();
        assert.deepEqual(ledger.map(({ version }) => version), [1, 2, 3, 4, 5, 6, 7]);
        assert.ok(ledger.every(({ applied_at }) => typeof applied_at === "string" && applied_at.length > 0));
        db.close();
        console.log(JSON.stringify(ledger));
    '
}
before=$(sqlite_ledger)
printf 'SQLite SELECT 1 and ledger 1–7 passed before restart\n'
docker stop -t 30 "$container" >/dev/null
test "$(docker inspect -f '{{.State.ExitCode}}' "$container")" = 0
docker start "$container" >/dev/null
ready
docker exec "$container" sh -ec 'test "$(stat -c %i /app/data/backup.db)" = "$(cat /app/data/smoke-db-inode)"'
after=$(sqlite_ledger)
if [ "$before" != "$after" ]; then
    printf 'SQLite migration ledger changed across restart\n' >&2
    exit 1
fi
printf 'SQLite SELECT 1 and unchanged ledger rows passed after clean restart on the same DB inode\n'
bash "$(dirname "$0")/test-native-docs.sh" "$image"
bash "$(dirname "$0")/test-native-logging.sh" "$image"
printf 'Native candidate smoke passed: %s\n' "$(docker inspect -f '{{.Image}}' "$container")"
