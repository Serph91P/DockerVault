import io
import tarfile

import pytest
from app.database import Backup, BackupStatus, BackupTarget


@pytest.mark.asyncio
async def test_archive_search_is_global_bounded_and_counts_matching_members(
    async_client, db_session, tmp_path
):
    path = tmp_path / "search.tar"
    with tarfile.open(path, "w") as archive:
        for i in range(2005):
            info = tarfile.TarInfo(f"root/file-{i:04}.txt")
            info.size = 1
            archive.addfile(info, io.BytesIO(b"x"))
    target = BackupTarget(
        name="Search source", target_type="path", host_path="/fixture"
    )
    db_session.add(target)
    await db_session.flush()
    backup = Backup(
        target_id=target.id, status=BackupStatus.COMPLETED, file_path=str(path)
    )
    db_session.add(backup)
    await db_session.commit()
    response = await async_client.get(
        f"/api/v1/backups/{backup.id}/files", params={"search": "file-200", "limit": 2}
    )
    assert response.status_code == 200
    assert response.headers["x-total-count"] == "5"
    assert response.headers["x-has-more"] == "true"
    assert [f["path"] for f in response.json()] == [
        "root/file-2000.txt",
        "root/file-2001.txt",
    ]
    second = await async_client.get(
        f"/api/v1/backups/{backup.id}/files",
        params={"search": "file-200", "limit": 2, "offset": 4},
    )
    assert [f["path"] for f in second.json()] == ["root/file-2004.txt"]
    assert second.headers["x-has-more"] == "false"
    whole = await async_client.get(f"/api/v1/backups/{backup.id}/download")
    assert whole.status_code == 200
    assert whole.content == path.read_bytes()
    assert "attachment" in whole.headers["content-disposition"]
