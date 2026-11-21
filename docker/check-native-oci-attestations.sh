#!/usr/bin/env bash
# Validate an unpublished single-platform BuildKit OCI export.
set -euo pipefail
archive=${1:?OCI archive required}
arch=${2:?Expected architecture required}
revision=${3:?Expected source revision required}
[[ "$revision" =~ ^[0-9a-f]{40}$ ]]
blob() {
    local digest=$1
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
    tar -xOf "$archive" "blobs/sha256/${digest#sha256:}"
}
verified_blob() {
    local sum
    # Hash the exact bytes first, without shell command substitution trimming a newline.
    sum=$(blob "$1" | sha256sum)
    [[ "sha256:${sum%% *}" == "$1" ]]
    blob "$1"
}
index=$(tar -xOf "$archive" index.json)
if [[ $(jq -r '.manifests[0].mediaType' <<< "$index") == application/vnd.oci.image.index.v1+json ]]; then
    nested=$(jq -er '.manifests | select(length==1) | .[0].digest' <<< "$index")
    index=$(verified_blob "$nested")
fi
image=$(jq -er --arg arch "$arch" '
    [.manifests[]|select(.platform.os=="linux" and .platform.architecture==$arch)] |
    select(length==1)|.[0].digest' <<< "$index")
attestation=$(jq -er --arg image "$image" '
    [.manifests[]|select(.annotations["vnd.docker.reference.type"]=="attestation-manifest" and .annotations["vnd.docker.reference.digest"]==$image)] |
    select(length==1)|.[0].digest' <<< "$index")
manifest=$(verified_blob "$image")
config=$(verified_blob "$(jq -er '.config.digest' <<< "$manifest")")
jq -e --arg arch "$arch" --arg rev "$revision" '
    .os=="linux" and .architecture==$arch and .config.Labels["org.opencontainers.image.revision"]==$rev' <<< "$config" >/dev/null
att=$(verified_blob "$attestation")
# OCI artifact form has its own subject. Older Docker-compatible manifests use the descriptor link.
jq -e --arg image "$image" 'if has("subject") then .subject.digest==$image else true end' <<< "$att" >/dev/null
for kind in spdx provenance; do
    descriptor=$(jq -er --arg kind "$kind" '
        [.layers[]|select(.mediaType=="application/vnd.in-toto+json") |
         select(.annotations["in-toto.io/predicate-type"] as $p |
           if $kind=="spdx" then $p=="https://spdx.dev/Document" else ($p|startswith("https://slsa.dev/provenance/")) end)] |
        select(length==1)|.[0]' <<< "$att")
    predicate=$(jq -er '.annotations["in-toto.io/predicate-type"]' <<< "$descriptor")
    verified_blob "$(jq -er '.digest' <<< "$descriptor")" | jq -e --arg kind "$kind" --arg type "$predicate" --arg image "${image#sha256:}" '
        (._type|startswith("https://in-toto.io/Statement/")) and .predicateType==$type and
        (.subject|type=="array") and
        ((.subject|length==0) or any(.subject[]; .digest.sha256==$image)) and
        (if $kind=="spdx" then (.predicate.spdxVersion|startswith("SPDX-")) and (.predicate.packages|length>0)
         else ((.predicate.buildDefinition.buildType // .predicate.buildType)|type=="string") end)' >/dev/null
done
printf 'Verified linked SPDX and provenance metadata for %s linux/%s %s\n' "$image" "$arch" "$revision"
