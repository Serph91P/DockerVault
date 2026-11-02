"""Directory links are archive entries, never an invitation to follow a subtree."""

import os
from unittest.mock import patch

import pytest

from app.backup_engine import BackupEngine
from app import tar_worker


@pytest.fixture
def link_tree(tmp_path):
    source = tmp_path / "links"
    (source / "real").mkdir(parents=True)
    (source / "real/file.txt").write_bytes(b"original payload")
    (source / "current").symlink_to("real", target_is_directory=True)
    (source / "loop").symlink_to(".", target_is_directory=True)
    return source


def make_archive(implementation, multi, source, archive, include, exclude):
    prefix = "mount/data" if multi else source.name
    if implementation == "fallback":
        engine = BackupEngine()
        with patch.object(engine, "_sudo_available", return_value=False):
            if multi:
                engine._create_multi_source_tar(
                    [(prefix, str(source))], str(archive), True, include, exclude, {}
                )
            else:
                engine._create_tar(str(source), str(archive), True, include, exclude)
    else:
        config = {
            "dest": str(archive),
            "compress": True,
            "sources": [[prefix, str(source)]],
            "include_paths": include,
            "exclude_paths": exclude,
            "per_volume_rules": {},
            "allowed_sources": [str(source)],
        }
        # Exercise the worker archive implementation on owned temporary paths;
        # the privileged CLI's fixed /backups boundary is tested in the image.
        result = tar_worker.create_tar(config)
        assert result["status"] == "ok"
        assert result["skipped_files"] == []
    return prefix


@pytest.mark.parametrize("implementation", ["fallback", "worker"])
@pytest.mark.parametrize("multi", [False, True])
@pytest.mark.parametrize("filtering", ["all", "include", "exclude"])
def test_directory_links_roundtrip_once_without_traversal(
    link_tree, tmp_path, implementation, multi, filtering
):
    import tarfile

    include = ["real", "current"] if filtering == "include" else []
    exclude = ["current"] if filtering == "exclude" else []
    archive = tmp_path / "archive.tar.gz"
    prefix = make_archive(implementation, multi, link_tree, archive, include, exclude)
    expected_links = (
        {"current", "loop"}
        if filtering == "all"
        else ({"current"} if filtering == "include" else {"loop"})
    )
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        names = [member.name for member in members]
        assert {
            member.name.removeprefix(prefix + "/")
            for member in members
            if member.issym()
        } == expected_links
        assert names.count(prefix + "/real/file.txt") == 1
        for link in expected_links:
            assert names.count(prefix + "/" + link) == 1
            assert not any(name.startswith(prefix + "/" + link + "/") for name in names)
    destination = tmp_path / "restore"
    BackupEngine()._extract_tar(
        str(archive), str(destination), "r:gz", None if multi else ""
    )
    restored = destination / prefix if multi else destination
    assert (restored / "real/file.txt").read_bytes() == b"original payload"
    for link in expected_links:
        assert (restored / link).is_symlink()
        assert os.readlink(restored / link) == ("real" if link == "current" else ".")
    if "current" in expected_links:
        assert (restored / "current/file.txt").read_bytes() == b"original payload"


@pytest.mark.parametrize("implementation", ["fallback", "worker"])
def test_external_directory_link_is_not_followed_and_restore_rejects_it(
    link_tree, tmp_path, implementation
):
    import tarfile

    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "never-archive").write_bytes(b"outside synthetic data")
    (link_tree / "external").symlink_to("../outside", target_is_directory=True)
    archive = tmp_path / "outside.tar.gz"
    prefix = make_archive(implementation, False, link_tree, archive, [], [])
    with tarfile.open(archive, "r:gz") as tar:
        entries = tar.getmembers()
        assert any(
            member.name == prefix + "/external" and member.issym() for member in entries
        )
        assert not any(
            "never-archive" in member.name
            or member.name.startswith(prefix + "/external/")
            for member in entries
        )
    destination = tmp_path / "restore"
    with pytest.raises(ValueError, match="[Ss]ymlink escape"):
        BackupEngine()._extract_tar(str(archive), str(destination), "r:gz", "")
    assert not (destination / "real/file.txt").exists()
    assert (outside / "never-archive").read_bytes() == b"outside synthetic data"
