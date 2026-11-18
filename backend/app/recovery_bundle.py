"""Password-authenticated, bounded V1 DockerVault recovery envelope.

Only the documented cryptographic header is public. Payload interpretation belongs
in recovery_service; no plaintext files or application state are touched here.
"""

import base64
import binascii
import json
import os
import threading
from typing import NoReturn

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id

MAX_BUNDLE_BYTES = 32 * 1024 * 1024
MAX_PAYLOAD_BYTES = 24 * 1024 * 1024 - 1024
_KDF_SLOT = threading.BoundedSemaphore(1)


class RecoveryBundleError(ValueError):
    """Safe public failure; never include input or nested crypto/parser errors."""


def _reject() -> NoReturn:
    raise RecoveryBundleError("Invalid recovery bundle or password") from None


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            _reject()
        result[key] = value
    return result


def _json(data):
    try:
        value = json.loads(
            data.decode("utf-8"),
            object_pairs_hook=_object,
            parse_constant=lambda _: _reject(),
        )
    except (ValueError, UnicodeError, RecursionError):
        _reject()
    if type(value) is not dict:
        _reject()
    return value


def _canonical(value):
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=True,
            allow_nan=False,
        ).encode("utf-8")
    except (ValueError, TypeError, RecursionError):
        _reject()


def _fields(value, names):
    if type(value) is not dict or set(value) != set(names):
        _reject()


def _b64(value, length=None):
    if type(value) is not str:
        _reject()
    try:
        raw = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        _reject()
    if not raw or base64.b64encode(raw).decode("ascii") != value:
        _reject()
    if length is not None and len(raw) != length:
        _reject()
    return raw


def _password(password):
    if type(password) is not str:
        _reject()
    try:
        raw = password.encode("utf-8")
    except UnicodeError:
        _reject()
    if not 1 <= len(raw) <= 1024:
        _reject()
    return raw


def _key(password, salt):
    # A busy request is rejected, not queued with another copy of its secrets.
    if not _KDF_SLOT.acquire(blocking=False):
        raise RecoveryBundleError("Recovery cryptography is busy; try again") from None
    try:
        return Argon2id(
            salt=salt, length=32, iterations=3, lanes=1, memory_cost=65536
        ).derive(password)
    finally:
        _KDF_SLOT.release()


def encode_bundle(payload: dict, password: str) -> bytes:
    """Encrypt a JSON object using a fresh direct Argon2id key and GCM nonce."""
    raw_password = _password(password)
    if type(payload) is not dict:
        _reject()
    plaintext = _canonical(payload)
    if len(plaintext) > MAX_PAYLOAD_BYTES:
        _reject()
    salt, nonce = os.urandom(16), os.urandom(12)
    header = {
        "format": "dockervault-recovery",
        "envelope_version": 1,
        "kdf": {
            "name": "argon2id",
            "memory_kib": 65536,
            "iterations": 3,
            "parallelism": 1,
            "salt_b64": base64.b64encode(salt).decode("ascii"),
        },
        "aead": {
            "name": "aes-256-gcm",
            "nonce_b64": base64.b64encode(nonce).decode("ascii"),
        },
    }
    ciphertext = AESGCM(_key(raw_password, salt)).encrypt(
        nonce, plaintext, _canonical(header)
    )
    header["aead"]["tag_b64"] = base64.b64encode(ciphertext[-16:]).decode("ascii")
    header["ciphertext_b64"] = base64.b64encode(ciphertext[:-16]).decode("ascii")
    return _canonical(header)


def decode_bundle(data: bytes, password: str) -> dict:
    """Validate all public fields before bounded KDF work; authenticate once."""
    raw_password = _password(password)
    if type(data) is not bytes or not 1 <= len(data) <= MAX_BUNDLE_BYTES:
        _reject()
    envelope = _json(data)
    _fields(envelope, ("format", "envelope_version", "kdf", "aead", "ciphertext_b64"))
    if envelope["format"] != "dockervault-recovery":
        _reject()
    if (
        type(envelope["envelope_version"]) is not int
        or envelope["envelope_version"] != 1
    ):
        _reject()
    kdf, aead = envelope["kdf"], envelope["aead"]
    _fields(kdf, ("name", "memory_kib", "iterations", "parallelism", "salt_b64"))
    _fields(aead, ("name", "nonce_b64", "tag_b64"))
    if kdf["name"] != "argon2id" or aead["name"] != "aes-256-gcm":
        _reject()
    for field, required in (
        ("memory_kib", 65536),
        ("iterations", 3),
        ("parallelism", 1),
    ):
        if type(kdf[field]) is not int or kdf[field] != required:
            _reject()
    salt = _b64(kdf["salt_b64"], 16)
    nonce = _b64(aead["nonce_b64"], 12)
    tag = _b64(aead["tag_b64"], 16)
    ciphertext = _b64(envelope.pop("ciphertext_b64"))
    if len(ciphertext) > MAX_PAYLOAD_BYTES:
        _reject()
    del aead["tag_b64"]
    try:
        plaintext = AESGCM(_key(raw_password, salt)).decrypt(
            nonce, ciphertext + tag, _canonical(envelope)
        )
    except InvalidTag:
        _reject()
    return _json(plaintext)
