#!/usr/bin/env bash
# Run the historical Chromium oracle against the real package and a synthetic TLS edge.
set -euo pipefail
image=${1:?Usage: bash docker/test-native-websocket-browser.sh APP_IMAGE BROWSER_IMAGE}
browser_image=${2:?Browser image required}
root=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d -t dv-r11-ws.XXXXXX)
app=
browser=
cleanup() {
    if [[ -n "$browser" ]]; then docker rm -fv "$browser" >/dev/null; fi
    if [[ -n "$app" ]]; then docker rm -fv "$app" >/dev/null; fi
    rm -f "$scratch/tls.key" "$scratch/tls.crt"
    rmdir "$scratch"
}
trap cleanup EXIT
openssl req -new -newkey rsa:2048 -nodes -x509 -days 1 -subj /CN=127.0.0.1 -keyout "$scratch/tls.key" -out "$scratch/tls.crt" >/dev/null 2>&1
app=$(docker run -d --pull=never --network none -e COOKIE_SECURE=true -e CORS_ORIGINS=https://127.0.0.1:8443 "$image")
docker exec "$app" mkdir -m 700 /tmp/dv-ws
for f in tls.crt tls.key; do docker cp "$scratch/$f" "$app:/tmp/dv-ws/$f"; done
docker exec -i "$app" sh -ec 'umask 077; cat > /tmp/dv-ws/nginx.conf; nginx -c /tmp/dv-ws/nginx.conf' <<'NGINX'
worker_processes 1;
pid /tmp/dv-ws/nginx.pid;
error_log stderr warn;
events { worker_connections 128; }
http {
  map $uri $log_ws { default 0; /ws/updates 1; }
  map $cookie_session_token $has_session { default present; '' absent; }
  log_format ws '$http_origin $has_session $status';
  access_log /tmp/dv-ws/handshakes.log ws if=$log_ws;
  ssl_certificate /tmp/dv-ws/tls.crt;
  ssl_certificate_key /tmp/dv-ws/tls.key;
  map $http_upgrade $connection_upgrade { default upgrade; '' close; }
  server {
    listen 127.0.0.1:8443 ssl;
    location / {
      proxy_pass http://127.0.0.1:80;
      proxy_http_version 1.1;
      proxy_set_header Host $http_host;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection $connection_upgrade;
    }
  }
  server {
    listen 127.0.0.1:8444 ssl;
    location / { default_type text/plain; return 200 'Synthetic foreign origin'; }
  }
}
NGINX
browser=$(docker create --pull=never --network "container:$app" --entrypoint node "$browser_image" /runner/browser_websocket.mjs)
docker cp "$root/backend/tests/browser_websocket.mjs" "$browser:/runner/browser_websocket.mjs"
timeout 100 docker start -a "$browser"
test "$(docker inspect -f '{{.State.ExitCode}}' "$browser")" = 0
docker exec "$app" sh -ec '
    grep -Fqx "https://127.0.0.1:8443 absent 403" /tmp/dv-ws/handshakes.log
    grep -Fqx "https://127.0.0.1:8444 present 403" /tmp/dv-ws/handshakes.log
'
printf 'TLS edge confirms the rejected foreign-origin handshake actually sent its session cookie.\n'
docker stop -t 15 "$app" >/dev/null
test "$(docker inspect -f '{{.State.ExitCode}}' "$app")" = 0
