#!/usr/bin/env bash
# Execute only the four recognized fixture arguments, never evaluate captured prose as shell code.
set -euo pipefail
browser=${1:?Stopped browser container required}
image=${2:?Recovery image required}
dev=${3:?Rust development image required}
revision=${4:?Revision required}
root=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d -t dv-native-instructions.XXXXXX)
container=
cleanup() {
    if [[ -n "$container" ]]; then docker rm -fv "$container" >/dev/null; fi
    rm -f "$scratch/dockervault-recover" "$scratch/evidence.json" "$scratch/commands"
    rmdir "$scratch"
}
trap cleanup EXIT
[[ $(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image") == "$revision" ]]
docker cp "$browser:/tmp/dockervault-recover" "$scratch/dockervault-recover"
docker cp "$browser:/tmp/dockervault-recovery-evidence.json" "$scratch/evidence.json"
hash=$(jq -er '.sha256 | select(test("^[0-9a-f]{64}$"))' "$scratch/evidence.json")
[[ $(sha256sum "$scratch/dockervault-recover" | cut -d' ' -f1) == "$hash" ]]
node - "$root" "$scratch/evidence.json" > "$scratch/commands" <<'NODE'
const assert = require('node:assert/strict')
const { readFileSync } = require('node:fs')
const [, , root, evidencePath] = process.argv
const readme = readFileSync(`${root}/README.md`, 'utf8')
assert.ok(readme.includes('(docs/RUST_OFFLINE_RECOVERY.md#recover-an-existing-backup)'))
const guide = readFileSync(`${root}/docs/RUST_OFFLINE_RECOVERY.md`, 'utf8')
const matches = [...guide.matchAll(/^```sh\n(\.\/dockervault-recover[\s\S]*?)\n```/gm)]
assert.equal(matches.length, 1)
const evidence = JSON.parse(readFileSync(evidencePath, 'utf8'))
const accepted = {
  '--archive': ['backup.tar.gz.enc'], '--output': ['backup.tar.gz'],
  '--identity': ['private_key.txt', 'my-private-identity.txt'], '--key': ['backup.tar.gz.key'],
}
for (const [surface, text] of Object.entries({ readme: matches[0][1], export: evidence.export, frontend: evidence.frontend })) {
  const [exe, ...args] = text.replace(/\\\n/g, ' ').trim().split(/\s+/)
  assert.equal(exe, './dockervault-recover', `wrong executable: ${surface}`)
  assert.equal(args.length, 8, `wrong argument count: ${surface}`)
  const seen = new Set()
  for (let i = 0; i < args.length; i += 2) {
    assert.ok(!seen.has(args[i]) && Object.hasOwn(accepted, args[i]) && accepted[args[i]].includes(args[i + 1]), `unexpected argument: ${surface}`)
    seen.add(args[i])
  }
  console.log(`${surface}\t${args.join('\t')}`)
}
NODE
container=$(docker run -d --pull=never --network none --read-only \
    --tmpfs /work:rw,exec,nosuid,nodev,uid=1000,gid=1000,mode=0700,size=32m \
    --user 1000:1000 --entrypoint /bin/sleep "$image" infinity)
docker exec -i "$container" sh -ec 'umask 077; mkdir /work/fixtures; cat > /work/dockervault-recover; chmod 700 /work/dockervault-recover' < "$scratch/dockervault-recover"
[[ $(docker exec "$container" sha256sum /work/dockervault-recover | cut -d' ' -f1) == "$hash" ]]
a=pax-links.tar.gz
tar -C "$root/crates/dockervault-core/tests/fixtures" -cf - "$a" "$a.enc" "$a.legacy.enc" "$a.key.TEST_ONLY" "$a.legacy.key.TEST_ONLY" |
    docker exec -i "$container" tar -xf - -C /work/fixtures
docker run --rm --pull=never --network none --cpus 2 --memory 6g -e CARGO_BUILD_JOBS=2 "$dev" \
    cargo run --quiet --example public_fixture_identity -p dockervault-tools --locked --offline -- gzip |
    docker exec -i "$container" sh -ec 'umask 077; cat > /work/private_key.txt; cp /work/private_key.txt /work/my-private-identity.txt'
while IFS=$'\t' read -r -a command; do
    for variant in '' .legacy; do
        docker exec -w /work "$container" sh -ec 'cp "fixtures/pax-links.tar.gz$1.enc" backup.tar.gz.enc; cp "fixtures/pax-links.tar.gz$1.key.TEST_ONLY" backup.tar.gz.key' sh "$variant"
        docker exec -w /work "$container" ./dockervault-recover "${command[@]:1}"
        docker exec -w /work "$container" sh -ec 'cmp fixtures/pax-links.tar.gz backup.tar.gz; tar -tzf backup.tar.gz >/dev/null; rm backup.tar.gz'
        printf 'Offline recovery matched exact bytes and readable TAR: %s %s\n' "${command[0]}" "${variant:-v1}"
    done
done < "$scratch/commands"
