"""V1 interoperability, pre-KDF rejection and confidential failures (synthetic only)."""

import base64
import copy
import importlib
import json

import pytest
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id

PASSWORD = "synthetic export passphrase"
CANARY = "synthetic-recovery-secret-canary"


def codec():
    return importlib.import_module("app.recovery_bundle")


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def fixture_envelope(plaintext=None):
    salt = bytes(range(16))
    nonce = bytes(range(16, 28))
    header = {
        "format": "dockervault-recovery",
        "envelope_version": 1,
        "kdf": {
            "name": "argon2id",
            "memory_kib": 65536,
            "iterations": 3,
            "parallelism": 1,
            "salt_b64": base64.b64encode(salt).decode(),
        },
        "aead": {"name": "aes-256-gcm", "nonce_b64": base64.b64encode(nonce).decode()},
    }
    key = Argon2id(
        salt=salt, length=32, iterations=3, lanes=1, memory_cost=65536
    ).derive(PASSWORD.encode())
    ct = AESGCM(key).encrypt(
        nonce, plaintext or encoded({"secret": CANARY}), encoded(header)
    )
    header["aead"]["tag_b64"] = base64.b64encode(ct[-16:]).decode()
    header["ciphertext_b64"] = base64.b64encode(ct[:-16]).decode()
    return header


def test_v1_known_external_envelope_decrypts():
    assert codec().decode_bundle(encoded(fixture_envelope()), PASSWORD) == {
        "secret": CANARY
    }


def test_export_roundtrip_is_randomized_and_interoperable():
    module = codec()
    payload = {"secret": CANARY, "schema_version": 1, "unicode": "ä"}
    first = module.encode_bundle(payload, PASSWORD)
    second = module.encode_bundle(payload, PASSWORD)
    assert first != second
    assert CANARY.encode() not in first
    assert PASSWORD.encode() not in first
    envelope = json.loads(first)
    assert set(envelope) == {
        "format",
        "envelope_version",
        "kdf",
        "aead",
        "ciphertext_b64",
    }
    assert module.decode_bundle(first, PASSWORD) == payload
    kdf = envelope["kdf"]
    assert {k: v for k, v in kdf.items() if k != "salt_b64"} == {
        "name": "argon2id",
        "memory_kib": 65536,
        "iterations": 3,
        "parallelism": 1,
    }
    salt = base64.b64decode(kdf["salt_b64"], validate=True)
    nonce = base64.b64decode(envelope["aead"]["nonce_b64"], validate=True)
    tag = base64.b64decode(envelope["aead"].pop("tag_b64"), validate=True)
    ct = base64.b64decode(envelope.pop("ciphertext_b64"), validate=True)
    assert (len(salt), len(nonce), len(tag)) == (16, 12, 16)
    key = Argon2id(
        salt=salt, length=32, iterations=3, lanes=1, memory_cost=65536
    ).derive(PASSWORD.encode())
    assert (
        json.loads(AESGCM(key).decrypt(nonce, ct + tag, encoded(envelope))) == payload
    )


@pytest.fixture(scope="module")
def valid_envelope():
    return fixture_envelope()


@pytest.mark.parametrize(
    "path,value",
    [
        (("extra",), "forbidden"),
        (("envelope_version",), 2),
        (("envelope_version",), True),
        (("format",), "other"),
        (("kdf", "name"), "scrypt"),
        (("kdf", "memory_kib"), 2**40),
        (("kdf", "memory_kib"), 65536.0),
        (("kdf", "iterations"), 4),
        (("kdf", "parallelism"), 2),
        (("kdf", "extra"), 1),
        (("kdf", "salt_b64"), "AA=="),
        (("kdf", "salt_b64"), "!" * 24),
        (("kdf", "salt_b64"), "AAECAwQFBgcICQoLDA0ODx=="),
        (("aead", "name"), "aes-128-gcm"),
        (("aead", "extra"), 1),
        (("aead", "nonce_b64"), "AA=="),
        (("aead", "tag_b64"), "AA=="),
        (("ciphertext_b64",), ""),
        (("ciphertext_b64",), "YQ==\n"),
        (("ciphertext_b64",), None),
        (("kdf",), []),
        (("aead",), None),
    ],
)
def test_malformed_header_rejected_before_kdf(valid_envelope, monkeypatch, path, value):
    module = codec()
    envelope = copy.deepcopy(valid_envelope)
    target = envelope
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value

    def forbidden_kdf(*args, **kwargs):
        pytest.fail("malformed input reached expensive KDF")

    monkeypatch.setattr(module, "Argon2id", forbidden_kdf)
    with pytest.raises(module.RecoveryBundleError):
        module.decode_bundle(encoded(envelope), PASSWORD)


@pytest.mark.parametrize(
    "data",
    [
        b"{}",
        b"[]",
        b"null",
        b"\xff",
        b"{",
        b'{"format":1,"format":2}',
        b'{"kdf":{"name":1,"name":2}}',
        b'{"format":NaN}',
    ],
)
def test_invalid_json_rejected_without_kdf(monkeypatch, data):
    module = codec()
    monkeypatch.setattr(module, "Argon2id", lambda **kwargs: pytest.fail("KDF invoked"))
    with pytest.raises(module.RecoveryBundleError):
        module.decode_bundle(data, PASSWORD)


@pytest.mark.parametrize(
    "change", ["password", "ciphertext", "salt", "nonce", "tag", "truncate"]
)
def test_authentication_failures_are_secret_free(valid_envelope, change):
    module = codec()
    envelope = copy.deepcopy(valid_envelope)
    password = PASSWORD
    if change == "password":
        password += "wrong"
    elif change != "truncate":
        target, key = {
            "ciphertext": (envelope, "ciphertext_b64"),
            "salt": (envelope["kdf"], "salt_b64"),
            "nonce": (envelope["aead"], "nonce_b64"),
            "tag": (envelope["aead"], "tag_b64"),
        }[change]
        value = bytearray(base64.b64decode(target[key]))
        value[0] ^= 1
        target[key] = base64.b64encode(value).decode()
    data = encoded(envelope)
    if change == "truncate":
        data = data[:-10]
    with pytest.raises(module.RecoveryBundleError) as error:
        module.decode_bundle(data, password)
    assert CANARY not in str(error.value)
    assert PASSWORD not in str(error.value)
    assert error.value.__cause__ is None


def test_authenticated_nonobject_or_duplicate_payload_is_rejected():
    module = codec()
    for plaintext in [b"[]", b'{"secret":1,"secret":2}', b'{"secret":Infinity}']:
        with pytest.raises(module.RecoveryBundleError):
            module.decode_bundle(encoded(fixture_envelope(plaintext)), PASSWORD)


def test_concurrent_crypto_is_bounded_without_waiting_or_leaking():
    from concurrent.futures import ThreadPoolExecutor
    from threading import Barrier

    module = codec()
    start = Barrier(8)

    def export():
        start.wait(timeout=5)
        try:
            return module.encode_bundle({"secret": CANARY}, PASSWORD)
        except module.RecoveryBundleError as error:
            assert CANARY not in str(error) and PASSWORD not in str(error)
            return None

    with ThreadPoolExecutor(max_workers=8) as workers:
        outcomes = list(workers.map(lambda _: export(), range(8)))
    assert sum(value is None for value in outcomes) >= 1
    assert any(value is not None for value in outcomes)
    for value in outcomes:
        if value is not None:
            assert module.decode_bundle(value, PASSWORD) == {"secret": CANARY}


def test_size_and_password_bounds_precede_kdf(monkeypatch):
    module = codec()
    monkeypatch.setattr(module, "Argon2id", lambda **kwargs: pytest.fail("KDF invoked"))
    for password in ["", "x" * 1025]:
        with pytest.raises(module.RecoveryBundleError):
            module.encode_bundle({"a": 1}, password)
    with pytest.raises(module.RecoveryBundleError):
        module.decode_bundle(b"x" * (module.MAX_BUNDLE_BYTES + 1), PASSWORD)
    with pytest.raises(module.RecoveryBundleError):
        module.encode_bundle({"a": "x" * module.MAX_PAYLOAD_BYTES}, PASSWORD)
