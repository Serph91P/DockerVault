"""Run exactly the published recovery commands with real age/OpenSSL, no app decryption."""

import asyncio
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import textwrap

from app.encryption import (
    encrypt_dek,
    encrypt_file,
    generate_key_pair,
    get_recovery_instructions,
)


def commands(source, root, public_key):
    if source == "export":
        return "\n".join(
            textwrap.dedent(block)
            for block in re.findall(
                r"```bash\n(.*?)\n[ \t]*```",
                get_recovery_instructions(public_key),
                re.S,
            )[:5]
        )
    if source == "readme":
        text = (
            (root / "README.md")
            .read_text()
            .split("**Without DockerVault (Disaster Recovery):**", 1)[1]
            .split("## Security Considerations", 1)[0]
        )
        return re.findall(r"```bash\n(.*?)\n```", text, re.S)[0]
    text = (
        (root / "frontend/src/components/EncryptionSetup.tsx")
        .read_text()
        .split("{/* Steps */}", 1)[1]
        .split("{/* Security Notes */}", 1)[0]
    )
    return "\n".join(re.findall(r"\{`(.*?)`\}", text, re.S)).replace("\\\\", "\\")


async def run(root):
    pair = await generate_key_pair()
    payload = b"owned disaster recovery payload\n"
    for source in ("export", "readme", "frontend"):
        for dek in (b"A" * 32, bytes(range(32))):
            with tempfile.TemporaryDirectory(prefix="recovery-contract-") as directory:
                work = Path(directory)
                archive = work / "backup.tar.gz"
                member = tarfile.TarInfo("payload.txt")
                member.size = len(payload)
                with tarfile.open(archive, "w:gz") as tar:
                    tar.addfile(member, io.BytesIO(payload))
                expected = archive.read_bytes()
                await encrypt_file(archive, work / "backup.tar.gz.enc", dek)
                (work / "backup.tar.gz.key").write_bytes(
                    await encrypt_dek(dek, pair.public_key)
                )
                archive.unlink()
                script = """age() {
  test "$(stat -c %a "$recovery_dir/dek.bin")" = 600 || return 1
  command age "$@"
}
openssl() {
  test "$(stat -c %a "$recovery_dir/dek.hex")" = 600 || return 1
  test "$(wc -c < "$recovery_dir/dek.hex")" -eq 64 || return 1
  command openssl "$@"
}
""" + re.sub(
                    r"AGE-SECRET-KEY-1X[^\n]*",
                    pair.private_key,
                    commands(source, root, pair.public_key),
                )
                temporary = work / "temporary"
                temporary.mkdir()
                result = subprocess.run(
                    ["bash"],
                    input=script,
                    cwd=work,
                    env=os.environ | {"TMPDIR": str(temporary)},
                    text=True,
                    capture_output=True,
                    timeout=30,
                )
                assert result.returncode == 0, (
                    f"{source}: recovery command failed (exit {result.returncode}): {result.stderr}"
                )
                assert archive.read_bytes() == expected, source
                assert (work / "payload.txt").read_bytes() == payload, source
                assert not list(temporary.iterdir()), (
                    f"{source}: temporary secret directory leaked"
                )
                assert not list(work.glob("dek*")), f"{source}: DEK files leaked"
                private = work / "private_key.txt"
                if private.exists():
                    assert private.stat().st_mode & 0o777 == 0o600
                for damaged in (work / "backup.tar.gz.enc", work / "backup.tar.gz.key"):
                    original = damaged.read_bytes()
                    damaged.write_bytes(b"invalid")
                    failed = subprocess.run(
                        ["bash"],
                        input=script,
                        cwd=work,
                        env=os.environ | {"TMPDIR": str(temporary)},
                        text=True,
                        capture_output=True,
                        timeout=30,
                    )
                    assert failed.returncode != 0, f"{source}: corrupt input accepted"
                    assert not list(temporary.iterdir()), (
                        f"{source}: failure leaked DEK files"
                    )
                    damaged.write_bytes(original)
                print(
                    f"{source}: exact commands recovered binary DEK={dek != b'A' * 32}; bytes, mode0600 and success/error secret cleanup passed"
                )


if __name__ == "__main__":
    asyncio.run(run(Path(sys.argv[1])))
