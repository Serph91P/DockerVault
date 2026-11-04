"""Verify downloaded identity via actual age and authenticated offline recovery."""

import json
from pathlib import Path
import sqlite3
import subprocess
import tarfile

root = Path("/app/data/ux181")
result = json.loads((root / "browser-result.json").read_text())
with sqlite3.connect(root / "backup.db") as db:
    archive_name, encrypted = db.execute(
        "SELECT file_path, encrypted FROM backups WHERE id=?", (result["backupId"],)
    ).fetchone()
    public_key = db.execute(
        "SELECT public_key FROM encryption_config LIMIT 1"
    ).fetchone()[0]
assert encrypted
identity = root / "exported.key"
probe = subprocess.run(
    ["age-keygen", "-y", str(identity)], capture_output=True, text=True
)
assert probe.returncode == 0, "Export is not an age identity file"
assert probe.stdout.strip() == public_key, (
    "Export does not match configured public identity"
)
archive = Path(archive_name)
output = root / "offline.tar.gz"
recovered = subprocess.run(
    [
        "python",
        "/app/app/recovery.py",
        "--identity",
        str(identity),
        "--key",
        str(archive.with_suffix(".key")),
        "--archive",
        str(archive),
        "--output",
        str(output),
    ],
    capture_output=True,
)
assert recovered.returncode == 0, (
    "Exported identity could not decrypt real authenticated archive"
)
expected = b"DockerVault synthetic recovery bytes\n"
with tarfile.open(output) as tar:
    matches = [
        member
        for member in tar.getmembers()
        if member.name.endswith("nested/payload.txt")
    ]
    assert len(matches) == 1
    assert tar.extractfile(matches[0]).read() == expected
restored = list(Path("/backups/ux181-restore").rglob("payload.txt"))
assert len(restored) == 1 and restored[0].read_bytes() == expected
identity.unlink()
output.unlink()
print(
    "PASS exported real age identity matches public identity, decrypts actual authenticated backup, and restored bytes match"
)
