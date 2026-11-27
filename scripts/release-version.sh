#!/usr/bin/env bash
# Shared release version selection from the checked-out commit and Git tags.
set -e
REF=${1:?Release source ref required}
SOURCE_SHA=${2:?Release source SHA required}
test "$(git rev-parse HEAD)" = "$SOURCE_SHA"

if [[ "$REF" == refs/tags/v* ]]; then
  # Manual tag push - use the tag version directly
  VERSION=${REF#refs/tags/v}
  if [[ "$VERSION" == *-dev* ]] || [[ "$VERSION" == dev.* ]]; then
    IS_PRERELEASE=true
    BUILD_ENV=development
  else
    IS_PRERELEASE=false
    BUILD_ENV=production
  fi
else
  # Auto-version: find highest version across ALL tags (dev + stable)
  HIGHEST_TAG=""
  HIGHEST_MAJOR=0
  HIGHEST_MINOR=0
  HIGHEST_PATCH=0

  for tag in $(git tag -l 'v*'); do
    ver="$tag"
    ver="${ver#v}"
    ver="${ver#dev.}"
    ver="${ver%%-dev*}"

    M=$(echo "$ver" | cut -d. -f1)
    m=$(echo "$ver" | cut -d. -f2)
    P=$(echo "$ver" | cut -d. -f3)

    [[ "$M" =~ ^[0-9]+$ ]] || continue
    [[ "$m" =~ ^[0-9]+$ ]] || continue
    [[ "$P" =~ ^[0-9]+$ ]] || continue

    if (( M > HIGHEST_MAJOR )) || \
       (( M == HIGHEST_MAJOR && m > HIGHEST_MINOR )) || \
       (( M == HIGHEST_MAJOR && m == HIGHEST_MINOR && P > HIGHEST_PATCH )); then
      HIGHEST_MAJOR=$M
      HIGHEST_MINOR=$m
      HIGHEST_PATCH=$P
      HIGHEST_TAG="$tag"
    fi
  done

  echo "Highest existing version: ${HIGHEST_MAJOR}.${HIGHEST_MINOR}.${HIGHEST_PATCH} (tag: ${HIGHEST_TAG:-none})" >&2

  # Analyze commits since last tag for conventional commit bump type
  BUMP="patch"
  if [ -n "$HIGHEST_TAG" ]; then
    COMMITS=$(git log "${HIGHEST_TAG}..HEAD" --pretty=format:"%s%n%b" 2>/dev/null || echo "")
  else
    COMMITS=$(git log --pretty=format:"%s%n%b" 2>/dev/null || echo "")
  fi

  if echo "$COMMITS" | grep -qiE '(^[a-z]+!:|BREAKING CHANGE)'; then
    BUMP="major"
  elif echo "$COMMITS" | grep -qiE '^feat(\(|:)'; then
    BUMP="minor"
  fi

  echo "Bump type: $BUMP" >&2

  case "$BUMP" in
    major)
      NEXT_MAJOR=$((HIGHEST_MAJOR + 1))
      NEXT_MINOR=0
      NEXT_PATCH=0
      ;;
    minor)
      NEXT_MAJOR=$HIGHEST_MAJOR
      NEXT_MINOR=$((HIGHEST_MINOR + 1))
      NEXT_PATCH=0
      ;;
    patch)
      NEXT_MAJOR=$HIGHEST_MAJOR
      NEXT_MINOR=$HIGHEST_MINOR
      NEXT_PATCH=$((HIGHEST_PATCH + 1))
      ;;
  esac

  VERSION="${NEXT_MAJOR}.${NEXT_MINOR}.${NEXT_PATCH}"

  if [[ "$REF" == refs/heads/main ]]; then
    IS_PRERELEASE=false
    BUILD_ENV=production
  else
    VERSION="${VERSION}-dev"
    IS_PRERELEASE=true
    BUILD_ENV=development
  fi
fi

printf 'version=%s\nis_prerelease=%s\nbuild_env=%s\n' "$VERSION" "$IS_PRERELEASE" "$BUILD_ENV"
