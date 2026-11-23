#!/usr/bin/env bash
# Prove ordinary scheduler startup projection and one real deadline in the native package.
set -euo pipefail
image=${1:?Native application image required}
[[ ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || {
    printf 'Run this container test only on the GitHub-hosted CI matrix\n' >&2
    exit 2
}
rev=$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
arch=$(docker image inspect -f '{{.Os}}/{{.Architecture}}' "$image")
[[ "$rev" =~ ^[a-f0-9]{40}$ && ( "$arch" == linux/amd64 || "$arch" == linux/arm64 ) ]]
app=$(docker run --detach --rm --pull=never --network none --cpus 2 --memory 6g -e TZ=UTC "$image")
phase=startup
cleanup() {
    local status=$? remaining attempt
    if ((status != 0)); then
        printf 'Scheduler restart failed at %s (%s, %s)\n' "$phase" "$arch" "$rev" >&2
    fi
    docker stop --time 30 "$app" >/dev/null || return 1
    for ((attempt=0; attempt<30; attempt++)); do
        remaining=$(docker ps --all --quiet --no-trunc --filter "id=$app") || return 1
        if [[ -z "$remaining" ]]; then return "$status"; fi
        sleep 1
    done
    printf 'Owned test container was not removed: %s\n' "$app" >&2
    return 1
}
trap cleanup EXIT

api() {
    local method=$1 path=$2
    shift 2
    docker exec -i -u 1000:1000 "$app" curl -fsS --max-time 8 -b /tmp/scheduler-cookie \
        -H 'Content-Type: application/json' -X "$method" "$@" "http://127.0.0.1/api/v1$path"
}
ready() {
    local attempt
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$app" curl -fsS --max-time 2 http://127.0.0.1/health 2>/dev/null |
            jq -e '.status == "healthy"' >/dev/null; then return; fi
        sleep 1
    done
    return 1
}
credentials=$(jq -nc --arg password "$(tr -d '-' </proc/sys/kernel/random/uuid)" \
    '{username:"scheduler-restart",password:$password,confirm_password:$password}')
authenticate() {
    printf '%s' "$credentials" | api POST "/auth/$1" -c /tmp/scheduler-cookie --data-binary @- >/dev/null
}
restart_backend() {
    local service=/run/dockervault/services/backend old_pid new_pid
    old_pid=$(docker exec "$app" s6-svstat -o pid "$service")
    [[ "$old_pid" =~ ^[0-9]+$ && "$old_pid" -gt 1 ]]
    docker exec "$app" s6-svc -wD -T 30000 -d "$service"
    [[ $(docker exec "$app" s6-svstat -o up,exitcode "$service") == 'false 0' ]]
    docker exec "$app" test ! -e "/proc/$old_pid"
    docker exec "$app" s6-svc -wu -T 30000 -u "$service"
    ready
    new_pid=$(docker exec "$app" s6-svstat -o pid "$service")
    [[ "$new_pid" =~ ^[0-9]+$ && "$new_pid" -gt 1 && "$new_pid" != "$old_pid" ]]
    docker exec -u 1000:1000 "$app" rm /tmp/scheduler-cookie
    authenticate login
    printf '%s: exit=0, backend PID %s -> %s, container=%s\n' "$phase" "$old_pid" "$new_pid" "$app"
}

ready
authenticate setup
docker exec -u 1000:1000 "$app" sh -ec '
    umask 077
    for plan in active disabled removed; do
        mkdir "/app/data/scheduler-$plan"
        printf "\000scheduler-%s\377\n" "$plan" > "/app/data/scheduler-$plan/payload"
    done
'

phase=configure
# Leave at least 90 seconds for setup and the first restart. Recur hourly so
# the following legitimate dispatch cannot mask a duplicate of this deadline.
due=$(( ($(date +%s) + 149) / 60 * 60 ))
next=$((due + 3600))
cron="$(date -u -d "@$due" +%-M) * * * *"
create_plan() {
    jq -nc --arg name "scheduler-$1" --arg cron "$cron" \
        '{name:$name,target_type:"path",host_path:("/app/data/"+$name),schedule_cron:$cron,enabled:true,compression_enabled:false}' |
        api POST /targets --data-binary @- | jq -er '.id | numbers'
}
active=$(create_plan active)
disabled=$(create_plan disabled)
removed=$(create_plan removed)
api GET /schedules/jobs | jq -e --arg a "backup_target_$active" --arg d "backup_target_$disabled" --arg r "backup_target_$removed" \
    'map(.id) | sort == ([$a, $d, $r] | sort)' >/dev/null
api PUT "/targets/$disabled" --data '{"enabled":false}' | jq -e '.enabled == false' >/dev/null
api DELETE "/targets/$removed" | jq -e '.status == "removed"' >/dev/null

expect_job() {
    local expected=$1 jobs term
    jobs=$(api GET /schedules/jobs)
    term=$(jq -er --arg id "backup_target_$active" \
        'select(length == 1) | .[0] | select(.id == $id and .name == "Backup: scheduler-active") | .next_run | strings' <<< "$jobs") || {
        printf 'Expected only the active registered job, got: %s\n' "$jobs" >&2
        return 1
    }
    [[ $(date -u -d "$term" +%s) == "$expected" ]] || {
        printf 'Expected registered deadline %s, got %s\n' "$expected" "$term" >&2
        return 1
    }
    printf '%s: active plan %s is the only registered job, next_run=%s\n' "$phase" "$active" "$term"
}
expect_empty_history() { api GET /backups | jq -e '. == []' >/dev/null; }
expect_job "$due"
expect_empty_history

phase=restart-before-due
restart_backend
expect_job "$due"
expect_empty_history
(( $(date +%s) < due ))

phase=automatic-backup
# No trigger or scheduler write after restart: observe the unfiltered catalog.
backup=
while (( $(date +%s) < due + 30 )); do
    rows=$(api GET /backups)
    if [[ "$rows" != '[]' ]]; then
        (( $(date +%s) >= due ))
        jq -e --argjson target "$active" 'length == 1 and .[0].target_id == $target and
            (.[0].status == "pending" or .[0].status == "running" or .[0].status == "completed")' <<< "$rows" >/dev/null
        if backup=$(jq -er '.[0] | select(.status == "completed") | .id | numbers' <<< "$rows"); then break; fi
    fi
    sleep 1
done
[[ -n "$backup" ]] || { printf 'No completed backup by deadline %s + 30s\n' "$due" >&2; exit 1; }
expect_backup() {
    api GET /backups | jq -er --argjson target "$active" --argjson backup "$backup" --argjson due "$due" '
        # The API emits UTC with Z or +00:00. The deadline has no fractional seconds.
        def epoch: sub("([.][0-9]+)?(Z|[+]00:00)$"; "Z") | fromdateiso8601;
        select(length == 1) | .[0] | select(.id == $backup and .target_id == $target and
          .backup_type == "full" and .status == "completed" and .completed_at != null and
          .local_available == true and .encrypted == false and .file_size > 0) |
        if (.created_at | epoch) < $due or (.completed_at | epoch) < $due then
          error("Backup \(.id) predates deadline \($due | todateiso8601): created_at=\(.created_at), completed_at=\(.completed_at)")
        else .file_path | strings | select(startswith("/backups/")) end'
}
archive=$(expect_backup)
verify_archive() {
    docker exec -u 1000:1000 "$app" sh -ec '
        test -f "$1"
        test ! -L "$1"
        tar -xOf "$1" scheduler-active/payload > /tmp/scheduler-backup-payload
        printf "\000scheduler-active\377\n" | cmp - /tmp/scheduler-backup-payload
        sha256sum "$1"
    ' sh "$archive"
}
digest=$(verify_archive)
printf 'Automatic backup %s: completed, fixture bytes verified, archive SHA256 %s\n' "$backup" "${digest%% *}"
expect_job "$next"

phase=restart-after-completion
restart_backend
unset credentials
expect_job "$next"
# Observe multiple scheduler ticks, still well before the next legitimate run.
for ((attempt=0; attempt<12; attempt++)); do
    [[ $(expect_backup) == "$archive" ]]
    sleep 1
done
[[ $(expect_backup) == "$archive" && $(verify_archive) == "$digest" ]]
expect_job "$next"
(( $(date +%s) < next ))
printf 'Scheduler restart verified: one completed backup %s, no disabled/removed backup or replay (%s, %s)\n' "$backup" "$arch" "$rev"
