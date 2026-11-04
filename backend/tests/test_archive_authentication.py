"""Encrypted plaintext is released only after authenticated content verification."""

import hashlib
from pathlib import Path
import subprocess

import pytest

from app.encryption import DecryptionError, decrypt_file, encrypt_file

KEY_MAGIC = b"DVKEY\x00\x01\n"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change",
    ["flip", "truncate", "reorder", "wrong_key", "strip_version", "unknown_version"],
)
async def test_authenticated_archive_rejects_mutation_without_destination_write(
    tmp_path, change
):
    source = tmp_path / "source.tar"
    source.write_bytes(bytes(range(256)) * 1024)
    encrypted, output = tmp_path / "archive.enc", tmp_path / "restored.tar"
    dek = bytes(range(32))
    await encrypt_file(source, encrypted, dek)
    original = encrypted.read_bytes()
    assert original.startswith(b"DVENC\x00\x01\n"), (
        "New archive must explicitly version authenticated data"
    )
    altered = bytearray(original)
    key_material = KEY_MAGIC + dek
    if change == "flip":
        altered[128] ^= 1
    elif change == "truncate":
        altered = altered[:-40]
    elif change == "reorder":
        altered[128:192], altered[192:256] = altered[192:256], altered[128:192]
    elif change == "wrong_key":
        key_material = KEY_MAGIC + b"wrong".ljust(32, b"x")
    elif change == "strip_version":
        # A new authenticated sidecar must prevent downgrade to legacy CBC.
        altered = altered[8:-32]
    else:
        altered[6] = 2
    encrypted.write_bytes(altered)
    output.write_bytes(b"existing destination sentinel")
    with pytest.raises(DecryptionError):
        await decrypt_file(encrypted, output, key_material)
    assert output.read_bytes() == b"existing destination sentinel"
    assert sorted(p.name for p in tmp_path.iterdir()) == [
        "archive.enc",
        "restored.tar",
        "source.tar",
    ]


@pytest.mark.asyncio
async def test_authenticated_archive_valid_roundtrip(tmp_path):
    source = tmp_path / "source.tar"
    source.write_bytes(b"owned payload" * 10000)
    encrypted, output = tmp_path / "archive.enc", tmp_path / "restored"
    dek = bytes(range(32))
    await encrypt_file(source, encrypted, dek)
    await decrypt_file(encrypted, output, KEY_MAGIC + dek)
    assert (
        hashlib.sha256(output.read_bytes()).digest()
        == hashlib.sha256(source.read_bytes()).digest()
    )
    assert output.stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
async def test_legacy_cbc_read_requires_legacy_key_material(tmp_path):
    source, ciphertext, password, restored = [
        tmp_path / n for n in ("plain", "old.enc", "pass", "restored")
    ]
    dek = bytes(range(32))
    source.write_bytes(b"legacy owned archive")
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
            str(source),
            "-out",
            str(ciphertext),
            "-pass",
            f"file:{password}",
        ],
        capture_output=True,
    )
    assert result.returncode == 0
    await decrypt_file(ciphertext, restored, dek)
    assert restored.read_bytes() == source.read_bytes()
    restored.write_bytes(b"sentinel")
    with pytest.raises(DecryptionError):
        await decrypt_file(ciphertext, restored, KEY_MAGIC + dek)
    assert restored.read_bytes() == b"sentinel"


@pytest.mark.asyncio
async def test_codec_cancellation_joins_children_and_removes_secrets(
    tmp_path, monkeypatch
):
    import asyncio
    import os
    import shutil
    import tempfile

    binary = shutil.which("openssl")
    wrapper = tmp_path / "bin"
    wrapper.mkdir()
    entered = tmp_path / "entered"
    command = wrapper / "openssl"
    command.write_text(
        f'#!/bin/sh\nprintf ready > "$OWNED_CRYPTO_ENTERED"\nsleep 10\nexec "{binary}" "$@"\n'
    )
    command.chmod(0o700)
    monkeypatch.setenv("PATH", str(wrapper) + os.pathsep + os.environ["PATH"])
    monkeypatch.setenv("OWNED_CRYPTO_ENTERED", str(entered))
    monkeypatch.setenv("TMPDIR", str(tmp_path))
    monkeypatch.setattr(tempfile, "tempdir", str(tmp_path))
    source, destination = tmp_path / "source", tmp_path / "output"
    source.write_bytes(b"owned payload")
    destination.write_bytes(b"existing destination")
    task = asyncio.create_task(encrypt_file(source, destination, bytes(range(32))))
    try:
        async with asyncio.timeout(5):
            while not entered.exists():
                assert not task.done()
                await asyncio.sleep(0.01)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 5)
        assert destination.read_bytes() == b"existing destination"
        assert not list(tmp_path.glob("dockervault-crypto-*"))
        assert not list(tmp_path.glob("*.key-material"))
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
