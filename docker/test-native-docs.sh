#!/usr/bin/env bash
# Exercise real packaged docs through loopback only. No external assets fetched.
set -euo pipefail
image=${1:?image required}
container=
cleanup() {
    if [[ -n "$container" ]]; then docker rm -fv "$container" >/dev/null; fi
}
trap cleanup EXIT
failed=0
expect() {
    if [[ "$2" != "$3" ]]; then
        printf 'FAIL %s expected %s got %s\n' "$1" "$3" "$2"
        failed=1
    else
        printf 'OK %s %s\n' "$1" "$2"
    fi
}
status() {
    docker exec "$container" curl -sS --max-time 5 -o /dev/null -w '%{http_code}' "$@"
}
for setting in default true; do
    env_args=()
    if [[ "$setting" == true ]]; then env_args=(-e ENABLE_DOCS=true); fi
    container=$(docker run -d --pull=never --network none -e COOKIE_SECURE=false "${env_args[@]}" "$image")
    upstream=http://127.0.0.1:8000
    proxy=http://127.0.0.1
    for ((attempt=0; attempt<30; attempt++)); do
        if [[ $(status "$proxy/health" 2>/dev/null) == 200 ]]; then break; fi
        sleep 1
    done
    expect "$setting upstream health" "$(status "$upstream/health")" 200
    if [[ "$setting" == true ]]; then
        for route in /openapi.json /docs /redoc; do
            expect "before setup $route" "$(status "$upstream$route")" 403
        done
    fi
    docker exec -u 1000:1000 "$container" sh -ec '
        umask 077
        password=$(tr -d "-" < /proc/sys/kernel/random/uuid)
        printf "{\"username\":\"docs-fixture\",\"password\":\"%s\",\"confirm_password\":\"%s\"}" "$password" "$password" |
            curl -fsS --max-time 8 -c /tmp/dv-docs-cookie -H "Content-Type: application/json" --data-binary @- http://127.0.0.1/api/v1/auth/setup >/dev/null
    '
    cookie=(-b /tmp/dv-docs-cookie)
    expect "$setting authenticated control" "$(status "${cookie[@]}" "$upstream/api/v1/auth/me")" 200
    wanted=404
    if [[ "$setting" == true ]]; then wanted=200; fi
    for route in /openapi.json /docs /redoc; do
        if [[ "$setting" == true ]]; then
            expect "anonymous $route" "$(status "$upstream$route")" 401
        fi
        actual=$(status "${cookie[@]}" "$upstream$route")
        expect "$setting authenticated $route" "$actual" "$wanted"
        if [[ "$wanted" == 200 && "$actual" == 200 ]]; then
            case "$route" in
                /openapi.json)
                    expect 'OpenAPI title, version, routes, request/response schema' "$(docker exec "$container" curl -fsS --max-time 5 "${cookie[@]}" "$upstream$route" | jq -r '(.info.title == "Docker Volume Backup Manager") and (.openapi | startswith("3.")) and (.paths | length > 80) and (.paths["/api/v1/auth/setup"].post | has("requestBody") and has("responses"))')" true;;
                /docs|/redoc)
                    body=$(docker exec "$container" curl -fsS --max-time 5 "${cookie[@]}" "$upstream$route")
                    expect "$route references local spec" "$(if [[ "$body" == *'/openapi.json'* ]]; then printf yes; else printf no; fi)" yes;;
            esac
        fi
        public=$(docker exec "$container" curl -fsS --max-time 5 "${cookie[@]}" "$proxy$route")
        index=$(docker exec "$container" curl -fsS --max-time 5 "${cookie[@]}" "$proxy/")
        expect "$setting public $route stays SPA" "$(if [[ "$public" == "$index" ]]; then printf yes; else printf no; fi)" yes
    done
    cleanup
    container=
done
exit "$failed"
