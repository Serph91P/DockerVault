"""Real archive roundtrips must restore bytes at the promised destination root."""

import io
import os
from pathlib import Path
import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from app.backup_engine import BackupEngine
from app.database import Backup, BackupStatus, BackupTarget


@pytest.mark.asyncio
@pytest.mark.parametrize("target_type", ["path", "volume"])
@pytest.mark.parametrize("custom", [False, True])
@pytest.mark.parametrize("compression", [True, False])
async def test_real_backup_modify_restore_maps_single_source_root(
    test_db, tmp_path, target_type, custom, compression
):
    source = tmp_path / ("source" if target_type == "path" else "_data")
    source.mkdir()
    (source / "payload").write_bytes(b"original bytes")
    os.link(source / "payload", source / "hardlink")
    async with test_db() as session:
        target = BackupTarget(
            name="restore-roundtrip",
            target_type=target_type,
            host_path=str(source),
            volume_name="owned-volume" if target_type == "volume" else None,
            compression_enabled=compression,
            stop_container=False,
            dependencies=[],
        )
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    engine = BackupEngine()
    destination = tmp_path / "custom" if custom else source
    with (
        patch("app.backup_engine.async_session", test_db),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch.object(engine, "_sudo_available", return_value=False),
        patch(
            "app.backup_engine.docker_client.list_volumes",
            new=AsyncMock(
                return_value=[
                    SimpleNamespace(name="owned-volume", mountpoint=str(source))
                ]
            ),
        ),
    ):
        assert await engine.run_backup(backup_id, skip_validation=True) is True
        async with test_db() as session:
            saved = await session.get(Backup, backup_id)
            assert saved.status == BackupStatus.COMPLETED
            assert saved.file_path.endswith(".tar.gz" if compression else ".tar")
            assert tarfile.is_tarfile(saved.file_path)
        (source / "payload").write_bytes(b"modified bytes")
        assert (
            await engine.restore_backup(
                backup_id, target_path=str(destination) if custom else None
            )
            is True
        )
    assert (destination / "payload").read_bytes() == b"original bytes"
    assert (destination / "hardlink").read_bytes() == b"original bytes"
    assert (destination / "payload").stat().st_ino == (
        destination / "hardlink"
    ).stat().st_ino
    assert not (destination / source.name).exists()
    if custom:
        assert (source / "payload").read_bytes() == b"modified bytes"


async def register_archive(test_db, path, target_type, destination, metadata=None):
    async with test_db() as session:
        target = BackupTarget(
            name="legacy",
            target_type=target_type,
            host_path=str(destination),
            stop_container=False,
            dependencies=[],
        )
        session.add(target)
        await session.flush()
        backup = Backup(
            target_id=target.id,
            status=BackupStatus.COMPLETED,
            file_path=str(path),
            backup_metadata=metadata or {},
        )
        session.add(backup)
        await session.commit()
        return backup.id


@pytest.mark.asyncio
async def test_legacy_single_source_archive_maps_old_prefix_after_path_change(
    test_db, tmp_path
):
    archive = tmp_path / "legacy.tar"
    with tarfile.open(archive, "w") as tar:
        member = tarfile.TarInfo("old_source/payload")
        member.size = 3
        tar.addfile(member, io.BytesIO(b"old"))
    destination = tmp_path / "renamed_source"
    backup_id = await register_archive(test_db, archive, "path", destination)
    with patch("app.backup_engine.async_session", test_db):
        assert await BackupEngine().restore_backup(backup_id) is True
    assert (destination / "payload").read_bytes() == b"old"
    assert not (destination / "old_source").exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("roots", [("data",), ("data", "logs")])
async def test_legacy_container_custom_restore_keeps_mount_roots(
    test_db, tmp_path, roots
):
    archive = tmp_path / "container.tar"
    with tarfile.open(archive, "w") as tar:
        for root in roots:
            member = tarfile.TarInfo(f"{root}/payload")
            member.size = 3
            tar.addfile(member, io.BytesIO(b"old"))
    destination = tmp_path / "custom"
    backup_id = await register_archive(test_db, archive, "container", destination)
    with patch("app.backup_engine.async_session", test_db):
        assert (
            await BackupEngine().restore_backup(backup_id, target_path=str(destination))
            is True
        )
    for root in roots:
        assert (destination / root / "payload").read_bytes() == b"old"
    assert not (destination / "payload").exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("malicious", ["traversal", "symlink", "hardlink"])
async def test_root_mapping_does_not_weaken_pre_extraction_validation(
    test_db, tmp_path, malicious
):
    archive = tmp_path / "hostile.tar"
    with tarfile.open(archive, "w") as tar:
        safe = tarfile.TarInfo("root/payload")
        safe.size = 4
        tar.addfile(safe, io.BytesIO(b"safe"))
        member = tarfile.TarInfo(
            "root/../../escape" if malicious == "traversal" else "root/link"
        )
        if malicious != "traversal":
            member.type = tarfile.SYMTYPE if malicious == "symlink" else tarfile.LNKTYPE
            member.linkname = "../../escape"
        tar.addfile(member)
    destination = tmp_path / "restore"
    backup_id = await register_archive(test_db, archive, "path", destination)
    with patch("app.backup_engine.async_session", test_db):
        assert await BackupEngine().restore_backup(backup_id) is False
    assert not (tmp_path / "escape").exists()
    assert not (destination / "payload").exists()
    assert not (destination / "root/payload").exists()
