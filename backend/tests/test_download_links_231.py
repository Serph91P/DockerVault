"""Bounded single-file lookup retains native duplicate and tar-link semantics."""

import io
import tarfile

import pytest

from tests.test_download_performance import catalog


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "kind", ["symlink", "hardlink", "duplicate", "forward-hardlink"]
)
async def test_bounded_download_matches_native_tar_member_lookup(
    async_client, test_db, tmp_path, kind
):
    identifier, archive = await catalog(test_db, tmp_path)
    with tarfile.open(archive, "w") as tar:
        if kind != "forward-hardlink":
            original = tarfile.TarInfo("owned/value")
            original.size = 4
            tar.addfile(original, io.BytesIO(b"old!"))
        member = tarfile.TarInfo("owned/alias")
        if kind in ("hardlink", "forward-hardlink"):
            member.type, member.linkname = tarfile.LNKTYPE, "owned/value"
        elif kind == "symlink":
            member.type, member.linkname = tarfile.SYMTYPE, "value"
        else:
            member.size = 4
        tar.addfile(member, io.BytesIO(b"old!") if kind == "duplicate" else None)
        if kind == "duplicate":
            tar.addfile(member, io.BytesIO(b"new!"))
        if kind == "forward-hardlink":
            value = tarfile.TarInfo("owned/value")
            value.size = 4
            tar.addfile(value, io.BytesIO(b"late"))
    with tarfile.open(archive, "r:") as tar:
        if kind == "forward-hardlink":
            with pytest.raises(KeyError):
                tar.extractfile(tar.getmember("owned/alias"))
            expected = None
        else:
            with tar.extractfile(tar.getmember("owned/alias")) as stream:
                expected = stream.read()
    response = await async_client.get(f"/api/v1/backups/{identifier}/files/owned/alias")
    assert response.status_code == (404 if expected is None else 200)
    if expected is not None:
        assert response.content == expected
