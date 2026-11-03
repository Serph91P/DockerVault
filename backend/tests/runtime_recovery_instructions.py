"""Run exactly the published recovery commands with real age/OpenSSL, no app decryption."""

import asyncio
import io
import os
from pathlib import Path
import re
import shutil
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
        for dek, legacy in [
            (key, old) for key in (b"A" * 32, bytes(range(32))) for old in (False, True)
        ]:
            with tempfile.TemporaryDirectory(prefix="recovery-contract-") as directory:
                work = Path(directory)
                archive = work / "backup.tar.gz"
                member = tarfile.TarInfo("payload.txt")
                member.size = len(payload)
                with tarfile.open(archive, "w:gz") as tar:
                    tar.addfile(member, io.BytesIO(payload))
                expected = archive.read_bytes()
                if legacy:
                    password = work / "legacy.pass"
                    password.write_text(dek.hex())
                    result = subprocess.run(
                        [
                            "openssl",
                            "enc",
                            "-aes-256-cbc",
                            "-pbkdf2",
                            "-iter",
                            "100000",
                            "-in",
                            str(archive),
                            "-out",
                            str(work / "backup.tar.gz.enc"),
                            "-pass",
                            f"file:{password}",
                        ],
                        capture_output=True,
                    )
                    assert result.returncode == 0
                    password.unlink()
                    wrapped = subprocess.run(
                        ["age", "-a", "-r", pair.public_key],
                        input=dek,
                        capture_output=True,
                    )
                    assert wrapped.returncode == 0
                    (work / "backup.tar.gz.key").write_bytes(wrapped.stdout)
                else:
                    await encrypt_file(archive, work / "backup.tar.gz.enc", dek)
                    (work / "backup.tar.gz.key").write_bytes(
                        await encrypt_dek(dek, pair.public_key)
                    )
                shutil.copyfile(
                    root / "backend/app/recovery.py", work / "recover_backup.py"
                )
                archive.unlink()
                script = re.sub(
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
                assert not list(work.glob("dockervault-crypto-*")), (
                    f"{source}: codec temporary files leaked"
                )
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
                    assert not list(work.glob("dockervault-crypto-*")), (
                        f"{source}: failure leaked codec files"
                    )
                    damaged.write_bytes(original)
                if not legacy:
                    encrypted = work / "backup.tar.gz.enc"
                    original = encrypted.read_bytes()
                    flipped = bytearray(original)
                    flipped[32] ^= 1
                    for altered in (bytes(flipped), original[:-1], original[8:-32]):
                        encrypted.write_bytes(altered)
                        rejected = subprocess.run(
                            ["bash"],
                            input=script,
                            cwd=work,
                            env=os.environ | {"TMPDIR": str(temporary)},
                            text=True,
                            capture_output=True,
                            timeout=30,
                        )
                        assert rejected.returncode != 0, (
                            f"{source}: authentication/downgrade bypass"
                        )
                        assert archive.read_bytes() == expected, (
                            f"{source}: rejected ciphertext mutated destination"
                        )
                        assert not list(work.glob("dockervault-crypto-*"))
                    encrypted.write_bytes(original)
                print(
                    f"{source}: exact commands recovered legacy={legacy}, binary DEK={dek != b'A' * 32}; bytes, mode0600 and success/error cleanup passed"
                )


if __name__ == "__main__":
    asyncio.run(run(Path(sys.argv[1])))
