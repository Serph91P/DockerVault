"""Standalone DockerVault archive recovery (Python standard library + age/OpenSSL).

Download this file as recover_backup.py; it has no DockerVault/package dependency.
"""

import argparse
import hashlib
import hmac
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

ARCHIVE_MAGIC = b"DVENC\x00\x01\n"
KEY_MAGIC = b"DVKEY\x00\x01\n"
CHUNK_SIZE = 1024 * 1024
TAG_SIZE = 32


def parse_key(material):
    authenticated = material.startswith(KEY_MAGIC)
    raw = material[len(KEY_MAGIC) :] if authenticated else material
    if len(raw) != 32:
        raise ValueError("Invalid or unsupported key envelope version")
    return raw, authenticated


def archive_mac(dek):
    key = hmac.digest(dek, b"DockerVault archive authentication v1", "sha256")
    return hmac.new(key, digestmod=hashlib.sha256)


def crypt_file(source, destination, material, *, decrypt):
    """Authenticate before invoking OpenSSL; publish only fully successful output."""
    source, destination = Path(source), Path(destination)
    dek, require_authentication = parse_key(material)
    with tempfile.TemporaryDirectory(
        prefix="dockervault-crypto-", dir=destination.parent
    ) as directory:
        work = Path(directory)
        password, ciphertext, plaintext = work / "pass", work / "cipher", work / "plain"
        password.write_bytes(dek.hex().encode("ascii"))
        password.chmod(0o600)
        if decrypt:
            with source.open("rb") as stream:
                header = stream.read(len(ARCHIVE_MAGIC))
                if header == ARCHIVE_MAGIC:
                    length = source.stat().st_size - len(ARCHIVE_MAGIC) - TAG_SIZE
                    if length < 32:
                        raise ValueError("Truncated authenticated archive")
                    mac = archive_mac(dek)
                    mac.update(header)
                    with ciphertext.open("wb") as output:
                        while length:
                            block = stream.read(min(CHUNK_SIZE, length))
                            if not block:
                                raise ValueError("Truncated authenticated archive")
                            mac.update(block)
                            output.write(block)
                            length -= len(block)
                    tag = stream.read(TAG_SIZE)
                    if not hmac.compare_digest(tag, mac.digest()) or stream.read(1):
                        raise ValueError("Archive authentication failed")
                elif header == b"Salted__" and not require_authentication:
                    # Explicit weaker legacy support: the age sidecar contains only 32 raw bytes.
                    ciphertext = source
                else:
                    raise ValueError(
                        "Missing or unsupported authenticated archive version"
                    )
            command = [
                "openssl",
                "enc",
                "-d",
                "-aes-256-cbc",
                "-pbkdf2",
                "-iter",
                "100000",
                "-in",
                str(ciphertext),
                "-out",
                str(plaintext),
                "-pass",
                f"file:{password}",
            ]
            result = subprocess.run(command, capture_output=True)
            if result.returncode:
                raise ValueError("Archive decryption failed")
            plaintext.chmod(0o600)
            os.replace(plaintext, destination)
        else:
            command = [
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
            ]
            result = subprocess.run(command, capture_output=True)
            if result.returncode:
                raise ValueError("Archive encryption failed")
            mac = archive_mac(dek)
            mac.update(ARCHIVE_MAGIC)
            with plaintext.open("wb") as output, ciphertext.open("rb") as stream:
                output.write(ARCHIVE_MAGIC)
                while block := stream.read(CHUNK_SIZE):
                    mac.update(block)
                    output.write(block)
                output.write(mac.digest())
                output.flush()
                os.fsync(output.fileno())
            plaintext.chmod(0o600)
            os.replace(plaintext, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--identity", type=Path)
    parser.add_argument("--key", type=Path, help="Age-wrapped .key sidecar")
    parser.add_argument("--key-material-file", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--encrypt", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.key_material_file:
        material = args.key_material_file.read_bytes()
    else:
        if not args.identity or not args.key or args.encrypt:
            parser.error("Recovery requires --identity and --key")
        result = subprocess.run(
            ["age", "-d", "-i", str(args.identity), str(args.key)], capture_output=True
        )
        if result.returncode:
            raise ValueError("Key envelope decryption failed")
        material = result.stdout
    crypt_file(args.archive, args.output, material, decrypt=not args.encrypt)


if __name__ == "__main__":

    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        main()
    except Exception as exc:
        print(f"Recovery failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
