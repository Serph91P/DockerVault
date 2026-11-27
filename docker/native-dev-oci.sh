#!/usr/bin/env bash
# Verify the native BuildKit export; never rebuild or write to a registry here.
set -euo pipefail
shopt -s inherit_errexit
fail() { printf '%s\n' "$*" >&2; exit 1; }
if [[ ${GITHUB_EVENT_NAME:-} == push && ${GITHUB_REF:-} == refs/heads/develop ]]; then
    source_revision=${GITHUB_SHA:-}
    build_env=development
    version=${DV_VERSION:-}
    [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+-dev$ ]] || fail 'invalid Develop OCI version'
elif [[ ${GITHUB_EVENT_NAME:-} == pull_request && ( ${1:-} == load || ${1:-} == receipt ) ]]; then
    source_revision=$(jq -er --arg ref "${GITHUB_REF:-}" '
        select((.number | type) == "number" and ("refs/pull/\(.number)/merge" == $ref)
            and .pull_request.base.ref == "develop")
        | .pull_request.head.sha | select(type == "string")
    ' "${GITHUB_EVENT_PATH:?Missing pull request event}") || fail 'invalid develop pull request event'
    [[ $(git rev-parse --verify HEAD) == "$source_revision" ]] || fail 'pull request checkout revision differs'
    build_env=production
    version=''
else
    fail 'requires a develop push'
fi
[[ $source_revision =~ ^[0-9a-f]{40}$ ]] || fail 'invalid source revision'
oci_index=application/vnd.oci.image.index.v1+json
oci_image=application/vnd.oci.image.manifest.v1+json
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
sha() { sha256sum "$1" | cut -d' ' -f1; }
read_blob() {
    local digest size
    digest=$(jq -er '.digest' <<< "$1")
    size=$(jq -er '.size | select(type == "number")' <<< "$1")
    [[ $digest =~ ^sha256:[a-f0-9]{64}$ ]] || fail 'invalid OCI digest'
    local file="$layout/blobs/sha256/${digest#sha256:}"
    [[ -f $file && ! -L $file && $(stat -c %s "$file") == "$size" && $(sha "$file") == "${digest#sha256:}" ]] || fail "OCI blob mismatch: $digest"
    printf '%s\n' "$file" >> "$layout/verified"
    printf '%s\n' "$file"
}
index_ref() {
    jq -nc --argjson descriptor "$2" --arg name "$3" \
        '{schemaVersion:2,manifests:[($descriptor | .annotations={"org.opencontainers.image.ref.name":$name})]}' > "$1/index.json"
}
inspect() {
    local input=$1 arch=$2 layout=$3 index image attestation manifest config layer entry spdx digest
    local archive="$input/application.oci.tar"
    [[ $arch == amd64 || $arch == arm64 ]] || fail 'unknown architecture'
    mkdir "$layout"
    # The PR caller checks its own local export. Promotion only reads Develop-push artifacts.
    tar -xf "$archive" -C "$layout" --no-same-owner --no-same-permissions
    index="$layout/index.json"
    if [[ $(jq -er '.manifests[0].mediaType' "$index") == "$oci_index" ]]; then
        index=$(read_blob "$(jq -ce '.manifests | select(length==1) | .[0]' "$index")")
    fi
    image=$(jq -ce --arg t "$oci_image" --arg a "$arch" \
        '.manifests | select(length == 2) | map(select(.mediaType==$t and .platform.os=="linux" and
        .platform.architecture==$a and ((.platform.variant // "")=="" or ($a=="arm64" and .platform.variant=="v8")))) |
        select(length==1) | .[0]' "$index")
    digest=$(jq -er '.digest' <<< "$image")
    attestation=$(jq -ce --arg t "$oci_image" --arg d "$digest" '
        [.manifests[] | select(.mediaType==$t and .platform.os=="unknown" and .platform.architecture=="unknown" and
         .annotations["vnd.docker.reference.type"]=="attestation-manifest" and .annotations["vnd.docker.reference.digest"]==$d)] |
        select(length==1) | .[0]' "$index")
    manifest=$(read_blob "$image")
    config=$(read_blob "$(jq -ce .config "$manifest")")
    jq -e --arg arch "$arch" --arg rev "$source_revision" \
        '.os=="linux" and .architecture==$arch and .config.Labels["org.opencontainers.image.revision"]==$rev' "$config" >/dev/null || fail 'OCI config/revision'
    jq -e --arg env "APP_ENV=$build_env" \
        '[.config.Env[] | select(startswith("APP_ENV="))] == [$env]' "$config" >/dev/null || fail 'OCI application build profile'
    if [[ $build_env == development ]]; then
        jq -e --arg version "$version" '.config.Labels["org.opencontainers.image.version"]==$version and
            .config.Labels["org.opencontainers.image.ref.name"]=="develop"' "$config" >/dev/null || fail 'Develop OCI version/ref label mismatch'
    fi
    entry=$(read_blob "$attestation")
    read_blob "$(jq -ce .config "$entry")" >/dev/null
    spdx=$(jq -re '[.layers[]|select(.annotations["in-toto.io/predicate-type"]=="https://spdx.dev/Document")]|select(length==1)|.[0].digest' "$entry")
    local -a attestations
    mapfile -t attestations < <(jq -c '.layers[]' "$entry")
    for layer in "${attestations[@]}"; do read_blob "$layer" >/dev/null; done
    # Reuse the existing SPDX/provenance subject and schema checks (including layer digests).
    bash "$(dirname "$0")/check-native-oci-attestations.sh" "$layout" "$arch" "$source_revision" >&2
    local binary=usr/local/bin/dockervault-recover http=usr/share/nginx/html/recovery-tools/manifest.json file hash raw
    # Walk newest to oldest, accepting the same regular-file paths as the in-app contract.
    # Whiteouts at download paths are unsupported. The native runner also compares
    # these bytes to the actual HTTP response from the selected config ID.
    local -a layers
    mapfile -t layers < <(jq -c '.layers | reverse[]' "$manifest")
    for layer in "${layers[@]}"; do read_blob "$layer" >/dev/null; done
    for layer in "${layers[@]}"; do
        digest=$(jq -er .digest <<< "$layer")
        file="$layout/blobs/sha256/${digest#sha256:}"
        tar -tf "$file" > "$scratch/members"
        for entry in "$binary" "$http"; do
            [[ ! -f $layout/${entry##*/} ]] || continue
            # No whiteout or replacement of an ancestor by a link/device.
            local ancestor=$entry
            while [[ $ancestor == */* ]]; do
                if grep -Fqx "${ancestor%/*}/.wh.${ancestor##*/}" "$scratch/members" ||
                   grep -Fqx "${ancestor%/*}/.wh..wh..opq" "$scratch/members"; then fail 'unsupported recovery whiteout'; fi
                ancestor=${ancestor%/*}
            done
            if grep -Fqx "$entry" "$scratch/members"; then
                [[ $(grep -Fxc "$entry" "$scratch/members") == 1 && $(tar -tvf "$file" "$entry") == -* ]] || fail 'nonregular or duplicate download file'
                tar -xOf "$file" "$entry" > "$layout/${entry##*/}"
            fi
        done
        [[ ! -s $layout/dockervault-recover || ! -s $layout/manifest.json ]] || break
    done
    hash=$(sha "$layout/dockervault-recover")
    raw=$(sha "$layout/manifest.json")
    jq -e --arg arch "$arch" --arg revision "$source_revision" --arg sha256 "$hash" \
        '.=={os:"linux",$arch,$revision,file:"dockervault-recover",$sha256}' "$layout/manifest.json" >/dev/null || fail 'in-app manifest binding'
    local machine=3e00
    [[ $arch != arm64 ]] || machine=b700
    [[ $(od -An -tx1 -N6 "$layout/dockervault-recover" | tr -d ' \n') == 7f454c460201 && \
       $(od -An -tx1 -j18 -N2 "$layout/dockervault-recover" | tr -d ' \n') == "$machine" ]] || fail 'ELF architecture'
    jq -nc --arg revision "$source_revision" --arg arch "$arch" --arg archive "$(sha "$archive")" \
        --argjson image "$image" --argjson attestation "$attestation" --arg config "$(jq -er .config.digest "$manifest")" \
        --arg binary_sha256 "$hash" --arg manifest_sha256 "$raw" --arg spdx "$spdx" \
        '{$revision,$arch,$archive,image:$image.digest,attestation:$attestation.digest,$config,$spdx,$binary_sha256,$manifest_sha256,descriptors:[$image,$attestation]}'
}
receipt() {
    jq -cS --arg event "$GITHUB_EVENT_NAME" --arg ref "$GITHUB_REF" --arg repository "$GITHUB_REPOSITORY" \
        --arg run_id "$2" --arg attempt "$3" --arg version "$version" \
        '. + {$event,$ref,$repository,$run_id,$attempt,$version} | del(.descriptors)' <<< "$1"
}
case ${1:-} in
    load)
        facts=$(inspect "$2" "$3" "$scratch/layout")
        index_ref "$scratch/layout" "$(jq -c '.descriptors[0]' <<< "$facts")" tested
        skopeo copy "oci:$scratch/layout:tested" "docker-daemon:$4:latest" >&2
        config=$(jq -er .config <<< "$facts")
        [[ $(docker image inspect -f '{{.Id}}' "$4") == "$config" ]] || fail 'loaded OCI config differs'
        printf 'Loaded OCI config %s linux/%s revision=%s BUILD_ENV=%s\n' "$config" "$3" "$source_revision" "$build_env"
        ;;
    receipt)
        facts=$(inspect "$2" "$3" "$scratch/layout")
        jq -e --argjson facts "$facts" '.config==$facts.config and .binary_sha256==$facts.binary_sha256 and .manifest_sha256==$facts.manifest_sha256' "$4" >/dev/null || fail 'browser bytes differ from OCI'
        receipt "$facts" "$GITHUB_RUN_ID" "$GITHUB_RUN_ATTEMPT"
        ;;
    promote)
        [[ $3 =~ ^[0-9]+$ && $4 =~ ^[0-9]+$ ]] || fail 'invalid native attempt'
        for arch in amd64 arm64; do
            file="$2/$arch/receipt.json"
            [[ -s $file ]] || fail "native receipt missing: $arch"
            facts=$(inspect "$2/$arch" "$arch" "$scratch/$arch")
            [[ $(jq -cS . "$file") == "$(receipt "$facts" "$3" "$4")" ]] || fail "native receipt mismatch: $arch"
            jq -c '.descriptors[]' <<< "$facts" >> "$scratch/descriptors"
        done
        mkdir "$5"
        mkdir -p "$5/blobs/sha256"
        cp "$scratch/amd64/oci-layout" "$5/oci-layout"
        while IFS= read -r file; do
            target="$5/blobs/sha256/${file##*/}"
            [[ -e $target ]] || cp --reflink=auto "$file" "$target"
        done < <(sort -u "$scratch"/{amd64,arm64}/verified)
        jq -cs --arg mediaType "$oci_index" '{schemaVersion:2,$mediaType,manifests:.}' "$scratch/descriptors" > "$scratch/index"
        digest=$(sha "$scratch/index")
        cp "$scratch/index" "$5/blobs/sha256/$digest"
        descriptor=$(jq -nc --arg mediaType "$oci_index" --arg digest "sha256:$digest" \
            --argjson size "$(stat -c %s "$scratch/index")" '{$mediaType,$digest,$size}')
        index_ref "$5" "$descriptor" bound
        printf 'sha256:%s\n' "$digest"
        ;;
    *) fail 'expected load, receipt or promote' ;;
esac
