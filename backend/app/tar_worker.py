#!/usr/bin/env python3
"""
Tar archive worker — runs with elevated privileges (via sudo) to read
Docker volume files that may be owned by various container-internal UIDs.

This script is intentionally self-contained: it does NOT import from the
main application to minimise the code surface that executes as root.

Usage:
    sudo python3 tar_worker.py '<json_config>'

JSON config schema:
    {
        "dest": "/backups/name/name_20260227.tar.gz",
        "compress": true,
        "sources": [["arcname", "/source/path"], ...],
        "include_paths": [],
        "exclude_paths": [],
        "per_volume_rules": {}
    }

Exit codes:
    0 — success (JSON result on stdout)
    1 — error   (JSON error on stdout)

Security:
    - Only callable via a sudoers rule restricted to this exact script path.
    - Validates all input paths to prevent path-traversal outside expected dirs.
    - Never writes outside the supplied ``dest`` path.
"""

import fnmatch
import json
import logging
import os
import sys
import tarfile
from typing import Dict, List

logging.basicConfig(level=logging.WARNING, format="%(levelname)s: %(message)s")
logger = logging.getLogger("tar_worker")

# Only allow reading from known Docker volume locations
_ALLOWED_SOURCE_PREFIXES = (
    "/var/lib/docker/volumes/",
    "/var/lib/docker/overlay2/",
)
# Only allow writing to the backup directory
_ALLOWED_DEST_PREFIX = "/backups/"


def _validate_paths(config: dict) -> None:
    """Reject configs that reference unexpected filesystem locations."""
    dest = os.path.realpath(config["dest"])
    if not dest.startswith(_ALLOWED_DEST_PREFIX):
        raise ValueError(
            f"Destination {dest!r} is outside the allowed backup directory."
        )

    for _arcname, source_path in config.get("sources", []):
        real = os.path.realpath(source_path)
        if not any(real.startswith(p) for p in _ALLOWED_SOURCE_PREFIXES):
            raise ValueError(
                f"Source path {real!r} is outside allowed Docker volume directories."
            )


# ── Path-filter helpers (duplicated from backup_engine to keep this
#    script self-contained and minimise the root-executed code surface) ──


def _should_include_path(
    path: str,
    include_paths: List[str],
    exclude_paths: List[str],
) -> bool:
    """Decide whether *path* passes the include/exclude filter."""
    path = path.lstrip("/")

    for pattern in exclude_paths:
        pattern = pattern.lstrip("/")
        if fnmatch.fnmatch(path, pattern) or path.startswith(
            pattern.rstrip("*").rstrip("/") + "/"
        ):
            return False

    if include_paths:
        for pattern in include_paths:
            pattern = pattern.lstrip("/")
            if fnmatch.fnmatch(path, pattern) or path.startswith(
                pattern.rstrip("*").rstrip("/") + "/"
            ):
                return True
        return False

    return True


def _is_parent_of_included(dir_path: str, include_paths: List[str]) -> bool:
    """Return *True* when *dir_path* leads towards an included path."""
    dir_path = dir_path.lstrip("/").rstrip("/") + "/"

    for pattern in include_paths:
        pattern = pattern.lstrip("/")
        literal_prefix = ""
        for char in pattern:
            if char in "*?[":
                break
            literal_prefix += char
        if literal_prefix.startswith(dir_path) or dir_path.startswith(literal_prefix):
            return True

    return False


# ── Tar creation ──


def _add_source_to_tar(
    tar: tarfile.TarFile,
    source_path: str,
    arcname_base: str,
    include_paths: List[str],
    exclude_paths: List[str],
) -> List[str]:
    """Add a single source directory to the tar with filtering.

    Returns a list of skipped file paths (permission/OS errors).
    """
    skipped: List[str] = []
    has_filters = bool(include_paths or exclude_paths)

    for dirpath, dirnames, filenames in os.walk(source_path):
        rel_dir = os.path.relpath(dirpath, source_path)
        if rel_dir == ".":
            rel_dir = ""

        arc_dir = os.path.join(arcname_base, rel_dir) if rel_dir else arcname_base

        # Prune excluded directories
        if exclude_paths:
            dirnames[:] = [
                d
                for d in dirnames
                if _should_include_path(
                    os.path.join(rel_dir, d) if rel_dir else d, [], exclude_paths
                )
            ]

        # Skip dirs that can't contain included files
        if include_paths and rel_dir:
            if not _should_include_path(
                rel_dir, include_paths, []
            ) and not _is_parent_of_included(rel_dir, include_paths):
                dirnames.clear()
                continue

        # Add directory entry
        try:
            tarinfo = tar.gettarinfo(dirpath, arcname=arc_dir)
            tar.addfile(tarinfo)
        except (PermissionError, OSError) as exc:
            logger.warning("Cannot read directory %s: %s", dirpath, exc)
            skipped.append(dirpath)
            dirnames.clear()
            continue

        # Add files
        for filename in filenames:
            filepath = os.path.join(dirpath, filename)
            file_rel = os.path.join(rel_dir, filename) if rel_dir else filename
            arcpath = os.path.join(arc_dir, filename)

            if has_filters and not _should_include_path(
                file_rel, include_paths, exclude_paths
            ):
                continue

            try:
                tarinfo = tar.gettarinfo(filepath, arcname=arcpath)
                if tarinfo.isreg():
                    with open(filepath, "rb") as fobj:
                        tar.addfile(tarinfo, fobj)
                else:
                    tar.addfile(tarinfo)
            except (PermissionError, OSError) as exc:
                logger.warning("Cannot read file %s: %s", filepath, exc)
                skipped.append(filepath)

    return skipped


def create_tar(config: dict) -> dict:
    """Create a tar archive according to *config*."""
    dest: str = config["dest"]
    compress: bool = config.get("compress", True)
    sources: List[list] = config["sources"]
    include_paths: List[str] = config.get("include_paths", [])
    exclude_paths: List[str] = config.get("exclude_paths", [])
    per_volume_rules: Dict[str, dict] = config.get("per_volume_rules", {})

    mode = "w:gz" if compress else "w"
    open_kwargs: dict = {"name": dest, "mode": mode}
    if compress:
        open_kwargs["compresslevel"] = 6

    all_skipped: List[str] = []

    with tarfile.open(**open_kwargs) as tar:
        for archive_name, source_path in sources:
            if not os.path.exists(source_path):
                continue

            volume_key = archive_name.lstrip("/")
            volume_basename = os.path.basename(volume_key)
            vol_rules = per_volume_rules.get(volume_key) or per_volume_rules.get(
                volume_basename
            )

            if vol_rules:
                vol_include = vol_rules.get("include_paths", [])
                vol_exclude = vol_rules.get("exclude_paths", [])
            else:
                vol_include = include_paths
                vol_exclude = exclude_paths

            skipped = _add_source_to_tar(
                tar, source_path, volume_key, vol_include, vol_exclude
            )
            all_skipped.extend(skipped)

    return {"status": "ok", "skipped_files": all_skipped}


# ── Entry-point ──


def main() -> None:
    if len(sys.argv) != 2:
        json.dump(
            {"status": "error", "message": "Usage: tar_worker.py '<json_config>'"},
            sys.stdout,
        )
        sys.exit(1)

    try:
        config = json.loads(sys.argv[1])
    except json.JSONDecodeError as exc:
        json.dump({"status": "error", "message": f"Invalid JSON: {exc}"}, sys.stdout)
        sys.exit(1)

    try:
        _validate_paths(config)
        result = create_tar(config)
        json.dump(result, sys.stdout)
        sys.exit(0)
    except Exception as exc:
        json.dump({"status": "error", "message": str(exc)}, sys.stdout)
        sys.exit(1)


if __name__ == "__main__":
    main()
