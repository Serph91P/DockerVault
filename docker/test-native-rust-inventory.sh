#!/bin/sh
# Inspect the shipped ELF files without starting the application or mounting its data.
set -eu
app=${1:?Native app image required}
recovery=${2:?Standalone recovery image required}
work=$(mktemp -d)
app_container=
recovery_container=
cleanup() {
    if [ -n "$app_container" ]; then docker rm -v "$app_container" >/dev/null; fi
    if [ -n "$recovery_container" ]; then docker rm -v "$recovery_container" >/dev/null; fi
    rm -r -- "$work"
}
trap cleanup EXIT
app_container=$(docker create --pull=never --network none "$app")
recovery_container=$(docker create --pull=never --network none "$recovery")
verify() {
    docker cp "$1:$2" "$work/binary"
    if ! readelf -SW "$work/binary" | grep -Fq ' .dep-v0 '; then
        printf 'Missing Rust dependency inventory: %s\n' "$2" >&2
        exit 1
    fi
    printf 'Embedded Rust dependency inventory: %s\n' "$2"
}
for binary in /usr/local/bin/dockervault-service /usr/local/libexec/dockervault-archive-worker \
    /usr/local/libexec/dockervault-process-guard /usr/local/libexec/dockervault-nginx-realip \
    /usr/local/bin/dockervault-recover; do
    verify "$app_container" "$binary"
done
verify "$recovery_container" /usr/local/bin/dockervault-recover
