#!/usr/bin/env bash
# Prove ordinary Embedded and Shared scheduler dispatch in the native package.
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
    for plan in active disabled removed shared-active shared-disabled; do
        mkdir "/app/data/scheduler-$plan"
        printf "\000scheduler-%s\377\n" "$plan" > "/app/data/scheduler-$plan/payload"
    done
'

phase=configure
# Leave at least 90 seconds for setup and the first restart. Recur hourly so
# the following legitimate dispatch cannot mask a duplicate of this deadline.
due=$(( ($(date +%s) + 149) / 60 * 60 ))
next=$((due + 3600))
shared_due=$((due + 60))
shared_next=$((shared_due + 3600))
cron="$(date -u -d "@$due" +%-M) * * * *"
shared_cron="$(date -u -d "@$shared_due" +%-M) * * * *"
create_plan() {
    jq -nc --arg name "scheduler-$1" --arg cron "$cron" --argjson schedule "${2:-null}" \
        '{name:$name,target_type:"path",host_path:("/app/data/"+$name),schedule_cron:$cron,enabled:true,compression_enabled:false}
         + (if $schedule == null then {} else {schedule_id:$schedule} end)' |
        api POST /targets --data-binary @- | jq -er '.id | numbers'
}
active=$(create_plan active)
disabled=$(create_plan disabled)
removed=$(create_plan removed)
api GET /schedules/jobs | jq -e --arg a "backup_target_$active" --arg d "backup_target_$disabled" --arg r "backup_target_$removed" \
    'map(.id) | sort == ([$a, $d, $r] | sort)' >/dev/null
api PUT "/targets/$disabled" --data '{"enabled":false}' | jq -e '.enabled == false' >/dev/null
api DELETE "/targets/$removed" | jq -e '.status == "removed"' >/dev/null
create_schedule() {
    jq -nc --arg name "$1" --arg cron "$shared_cron" --argjson enabled "$2" \
        '{name:$name,cron_expression:$cron,enabled:$enabled}' |
        api POST /schedules --data-binary @- | jq -er '.id | numbers'
}
shared_schedule=$(create_schedule 'Active shared schedule' true)
disabled_schedule=$(create_schedule 'Disabled shared schedule' false)
shared=$(create_plan shared-active "$shared_schedule")
suppressed=$(create_plan shared-disabled "$disabled_schedule")

expect_jobs() {
    local jobs name target expected term
    jobs=$(api GET /schedules/jobs)
    jq -e --arg a "backup_target_$active" --arg s "backup_target_$shared" \
        'map(.id) | sort == ([$a, $s] | sort)' <<< "$jobs" >/dev/null
    for name in active shared-active; do
        if [[ $name == active ]]; then target=$active; expected=$1; else target=$shared; expected=$2; fi
        term=$(jq -er --arg id "backup_target_$target" --arg name "Backup: scheduler-$name" \
            '.[] | select(.id == $id and .name == $name) | .next_run | strings' <<< "$jobs")
        [[ $(date -u -d "$term" +%s) == "$expected" ]] || {
            printf 'Expected %s deadline %s, got %s\n' "$name" "$expected" "$term" >&2
            return 1
        }
        printf '%s: %s registered job (target %s), next_run=%s\n' "$phase" "$name" "$target" "$term"
    done
}
expect_shared_legacy() {
    local target schedule rows term
    for target in "$shared" "$suppressed"; do
        if [[ $target == "$shared" ]]; then schedule=$shared_schedule; else schedule=$disabled_schedule; fi
        api GET "/targets/$target" | jq -e --argjson schedule "$schedule" --arg cron "$cron" \
            '.enabled == true and .schedule_id == $schedule and .schedule_cron == $cron' >/dev/null
    done
    rows=$(api GET /schedules/legacy/by-target)
    term=$(jq -er --argjson t "$shared" --arg cron "$shared_cron" \
        '[.[] | select(.target_id == $t)] | select(length == 1) | .[0] |
         select(.enabled == true and .cron_expression == $cron) | .next_run | strings' <<< "$rows")
    [[ $(date -u -d "$term" +%s) == "$shared_due" ]]
    jq -e --argjson t "$suppressed" --arg cron "$shared_cron" \
        '[.[] | select(.target_id == $t)] | length == 1 and
         (.[0] | .enabled == false and .cron_expression == $cron and has("next_run") and .next_run == null)' \
        <<< "$rows" >/dev/null
    printf '%s: legacy active Shared next_run=%s; disabled Shared target %s enabled=false next_run=null, no job\n' \
        "$phase" "$term" "$suppressed"
}
expect_empty_history() { api GET /backups | jq -e '. == []' >/dev/null; }
expect_jobs "$due" "$shared_due"
expect_empty_history

phase=restart-before-due
restart_backend
expect_jobs "$due" "$shared_due"
expect_shared_legacy
expect_empty_history
(( $(date +%s) < due ))

phase=automatic-backup
# Both linked targets' own cron fires at the Embedded control's deadline.
# Keep polling past both Shared deadlines even if the expected backup completes early.
while :; do
    observed=$(date +%s)
    rows=$(api GET /backups)
    jq -e --argjson a "$active" --argjson s "$shared" --argjson due "$due" --argjson shared_due "$shared_due" \
        --argjson now "$observed" '
        def epoch: sub("([.][0-9]+)?(Z|[+]00:00)$"; "Z") | fromdateiso8601;
        def timely($deadline):
            (.created_at | epoch) >= $deadline and
            if .status == "completed" then
                (.completed_at | epoch) >= $deadline and (.completed_at | epoch) < $deadline + 30
            else .status == "pending" or .status == "running" end;
        def ready($target; $deadline):
            $now < $deadline + 30 or any(.[]; .target_id == $target and .status == "completed");
        type == "array" and length == (unique_by(.target_id) | length) and
        all(.[]; (.target_id == $a and timely($due)) or (.target_id == $s and timely($shared_due))) and
        ready($a; $due) and ready($s; $shared_due)' <<< "$rows" >/dev/null || {
        printf 'Unexpected catalog at %s (Embedded/own due=%s, Shared due=%s): %s\n' \
            "$observed" "$due" "$shared_due" "$rows" >&2
        exit 1
    }
    if (( observed >= shared_due + 30 )); then break; fi
    sleep 1
done
printf 'Observation through %s: no Shared fallback at own deadline %s, no disabled Shared backup at either %s or %s\n' \
    "$observed" "$due" "$due" "$shared_due"
expect_backup() {
    jq -er --argjson target "$1" '
        select(length == 2) | [.[] | select(.target_id == $target)] | select(length == 1) | .[0] |
        select((.id | type) == "number" and
          .backup_type == "full" and .status == "completed" and .completed_at != null and
          .local_available == true and .encrypted == false and .file_size > 0 and
          (.file_path | strings | startswith("/backups/"))) |
        [.id, .file_path, .created_at, .completed_at] | @tsv'
}
verify_archive() {
    docker exec -u 1000:1000 "$app" sh -ec '
        test -f "$1"
        test ! -L "$1"
        tar -xOf "$1" "scheduler-$2/payload" > /tmp/scheduler-backup-payload
        printf "\000scheduler-%s\377\n" "$2" | cmp - /tmp/scheduler-backup-payload
        sha256sum "$1"
    ' sh "$1" "$2"
}
embedded_record=$(expect_backup "$active" <<< "$rows")
shared_record=$(expect_backup "$shared" <<< "$rows")
record_archive() {
    local id path created completed digest
    IFS=$'\t' read -r id path created completed <<< "$1"
    digest=$(verify_archive "$path" "$2") || return 1
    printf '%s automatic backup %s: completed, created_at=%s completed_at=%s, fixture bytes verified, archive SHA256 %s\n' \
        "$2" "$id" "$created" "$completed" "${digest%% *}" >&2
    printf '%s\n' "$digest"
}
embedded_digest=$(record_archive "$embedded_record" active)
shared_digest=$(record_archive "$shared_record" shared-active)
expect_jobs "$next" "$shared_next"

phase=restart-after-completion
restart_backend
unset credentials
expect_jobs "$next" "$shared_next"
expect_unchanged_history() {
    rows=$(api GET /backups)
    [[ $(expect_backup "$active" <<< "$rows") == "$embedded_record" &&
       $(expect_backup "$shared" <<< "$rows") == "$shared_record" ]]
}
# Observe multiple scheduler ticks, still well before the next legitimate run.
for ((attempt=0; attempt<12; attempt++)); do
    expect_unchanged_history
    sleep 1
done
expect_unchanged_history
[[ $(record_archive "$embedded_record" active) == "$embedded_digest" &&
   $(record_archive "$shared_record" shared-active) == "$shared_digest" ]]
expect_jobs "$next" "$shared_next"
(( $(date +%s) < next ))
printf 'Scheduler restart verified: one Embedded and one Shared backup, no disabled/removed backup or replay (%s, %s)\n' "$arch" "$rev"
