#!/usr/bin/env bash
# Validate service logs without publishing raw logs or opening a host port.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-logging.sh IMAGE}
container=
cleanup() {
    if [[ -n "$container" ]]; then docker rm -fv "$container" >/dev/null; fi
}
trap cleanup EXIT
failed=0
for variant in text:INFO json:INFO json:ERROR json:DEBUG json:WARNING json:CRITICAL; do
    format=${variant%:*}
    level=${variant#*:}
    container=$(docker run -d --pull=never --network none -e COOKIE_SECURE=false -e "LOG_FORMAT=$format" -e "LOG_LEVEL=$level" "$image")
    ready=false
    for ((attempt=0; attempt<30; attempt++)); do
        if docker exec "$container" curl -fs --max-time 2 http://127.0.0.1/health >/dev/null; then
            ready=true
            break
        fi
        sleep 1
    done
    [[ "$ready" == true ]]
    docker stop -t 30 "$container" >/dev/null
    [[ $(docker inspect -f '{{.State.ExitCode}}' "$container") == 0 ]]
    if docker logs "$container" 2>&1 | jq -e -Rn --arg format "$format" --arg level "$level" '
        [inputs] as $lines |
        [$lines[] | fromjson? | select(type == "object" and has("timestamp") and has("logger") and has("message") and has("level"))] as $records |
        [$records[].level] as $levels |
        if $format == "text" then
            ($records | length == 0) and any($lines[]; contains(" - INFO - "))
        elif $level == "INFO" then
            ($levels | index("INFO") != null and index("WARNING") != null and index("DEBUG") == null)
        elif $level == "DEBUG" then
            ($levels | index("DEBUG") != null and index("INFO") != null)
        elif $level == "WARNING" then
            ($levels | index("WARNING") != null and index("INFO") == null and index("DEBUG") == null)
        else
            ($levels | index("WARNING") == null and index("INFO") == null and index("DEBUG") == null)
        end' >/dev/null; then
        printf 'OK %s\n' "$variant"
    else
        printf 'FAIL %s\n' "$variant"
        failed=1
    fi
    cleanup
    container=
done
exit "$failed"
