"""Review regressions: public SSH custody and effective native source portability."""

import base64
import copy
import hashlib
import json

import pytest
from cryptography.hazmat.primitives import serialization as ssh
from cryptography.hazmat.primitives.asymmetric import ed25519

from app import rclone_recovery as recovery
from tests.test_recovery_activation import file_database as file_database


@pytest.fixture
def ssh_peer(tmp_path, ssh_material):
    import socket
    import subprocess
    import time

    _, public, private = ssh_material
    home = tmp_path / "home"
    (home / ".ssh").mkdir(parents=True)
    identity = home / ".ssh" / "id_ed25519"
    identity.write_bytes(private)
    identity.chmod(0o600)
    host = ed25519.Ed25519PrivateKey.generate()
    host_path = tmp_path / "host"
    host_path.write_bytes(
        host.private_bytes(
            ssh.Encoding.PEM, ssh.PrivateFormat.OpenSSH, ssh.NoEncryption()
        )
    )
    host_path.chmod(0o600)
    authorized = tmp_path / "authorized"
    authorized.write_bytes(public)
    depot = tmp_path / "depot"
    depot.mkdir()
    (depot / "unchanged.archive").write_bytes(b"synthetic-original-archive-bytes")
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    known = tmp_path / "known_hosts"
    known.write_bytes(
        f"[127.0.0.1]:{port} ".encode()
        + host.public_key().public_bytes(ssh.Encoding.OpenSSH, ssh.PublicFormat.OpenSSH)
        + b"\n"
    )
    server = subprocess.Popen(
        [
            recovery._binary(),
            "--config",
            "/dev/null",
            "--log-level",
            "ERROR",
            "serve",
            "sftp",
            str(depot),
            "--addr",
            f"127.0.0.1:{port}",
            "--authorized-keys",
            str(authorized),
            "--key",
            str(host_path),
        ],
        env=recovery._environment(),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 10
        while True:
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                    break
            except OSError:
                assert server.poll() is None and time.monotonic() < deadline
                time.sleep(0.02)
        yield {
            "identity": identity,
            "home": home,
            "known": known,
            "port": port,
            "depot": depot,
        }
    finally:
        server.terminate()
        server.communicate(timeout=10)
        identity.unlink(missing_ok=True)
        host_path.unlink(missing_ok=True)


def storage(name="vault", identifier=1):
    return {"id": identifier, "storage_type": "rclone", "rclone_remote": name}


@pytest.fixture
def ssh_material():
    key = ed25519.Ed25519PrivateKey.generate()
    ca = ed25519.Ed25519PrivateKey.generate()
    certificate = (
        ssh.SSHCertificateBuilder()
        .public_key(key.public_key())
        .serial(1)
        .type(ssh.SSHCertificateType.USER)
        .key_id(b"synthetic-recovery-review")
        .valid_principals([b"fixture"])
        .valid_after(0)
        .valid_before(4102444800)
        .sign(ca)
        .public_bytes()
    )
    public = key.public_key().public_bytes(
        ssh.Encoding.OpenSSH, ssh.PublicFormat.OpenSSH
    )
    private = key.private_bytes(
        ssh.Encoding.PEM, ssh.PrivateFormat.OpenSSH, ssh.NoEncryption()
    )
    return certificate, public, private


@pytest.mark.parametrize("field", ["pubkey_file", "pubkey"])
@pytest.mark.parametrize("combined", [False, True])
def test_public_certificate_export_never_embeds_combined_private_identity(
    field, combined, ssh_material, tmp_path
):
    from app.recovery_bundle import encode_bundle, decode_bundle

    certificate, _, private = ssh_material
    material = certificate + b"\n" + (private if combined else b"")
    path = tmp_path / "cert-and-key"
    path.write_bytes(material)
    remote = {"type": "sftp", "key_file": str(path)}
    remote[field] = str(path) if field.endswith("_file") else material.decode()
    payload = recovery.package_rclone({"vault": remote}, [storage()])
    decrypted = decode_bundle(
        encode_bundle(payload, "synthetic-review-password"), "synthetic-review-password"
    )
    assert not any(
        private in base64.b64decode(record["content_b64"])
        for record in decrypted["files"].values()
    )
    assert private.decode() not in json.dumps(decrypted)
    assert decrypted["roots"][0]["reason"] == "ssh_private_key_required"
    exported = decrypted["remotes"]["vault"][field]
    data = (
        base64.b64decode(decrypted["files"][exported["file_ref"]]["content_b64"])
        if field.endswith("_file")
        else exported.encode()
    )
    assert data.strip() == certificate
    recovery.validate_rclone(decrypted, [storage()])


@pytest.mark.parametrize("field", ["pubkey_file", "pubkey", "known_hosts_file"])
def test_import_rejects_private_identity_hidden_in_public_ssh_material(
    field, ssh_material, tmp_path
):
    certificate, public, private = ssh_material
    clean = (
        b"example.invalid " + public + b"\n"
        if field == "known_hosts_file"
        else certificate + b"\n"
    )
    path = tmp_path / "public"
    path.write_bytes(clean)
    remote = {
        "type": "sftp",
        field: str(path) if field.endswith("_file") else clean.decode(),
    }
    payload = recovery.package_rclone({"vault": remote}, [storage()])
    recovery.validate_rclone(payload, [storage()])
    dirty = clean + private
    if field.endswith("_file"):
        old = payload["remotes"]["vault"][field]["file_ref"]
        identifier = hashlib.sha256(dirty).hexdigest()
        del payload["files"][old]
        payload["files"][identifier] = {
            "kind": "provider_credential",
            "content_b64": base64.b64encode(dirty).decode(),
        }
        payload["remotes"]["vault"][field] = {"file_ref": identifier}
    else:
        payload["remotes"]["vault"][field] = dirty.decode()
    with pytest.raises(recovery.RcloneRecoveryError, match="Invalid or unavailable"):
        recovery.validate_rclone(payload, [storage()])


@pytest.mark.parametrize("field", ["pubkey_file", "pubkey", "known_hosts_file"])
def test_export_rejects_private_only_or_invalid_public_ssh_material(
    field, ssh_material, tmp_path
):
    _, _, private = ssh_material
    path = tmp_path / "not-public"
    path.write_bytes(private)
    remote = {
        "type": "sftp",
        field: str(path) if field.endswith("_file") else private.decode(),
    }
    payload = recovery.package_rclone({"vault": remote}, [storage()])
    assert payload["roots"][0]["state"] == "credentials_required"
    assert payload["remotes"] == {} and payload["files"] == {}
    recovery.validate_rclone(payload, [storage()])


@pytest.mark.parametrize("reference", ["absolute", "relative", "tilde", "environment"])
def test_ssh_reconnect_metadata_resolves_source_paths_without_reading_private_key(
    reference, tmp_path, monkeypatch
):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("DV_SYNTHETIC_KEY_HOME", str(tmp_path / "home"))
    expected = str(tmp_path / "home" / ".ssh" / "id_ed25519")
    values = {
        "absolute": expected,
        "relative": "home/.ssh/id_ed25519",
        "tilde": "~/.ssh/id_ed25519",
        "environment": "$DV_SYNTHETIC_KEY_HOME/.ssh/id_ed25519",
    }
    payload = recovery.package_rclone(
        {"vault": {"type": "sftp", "key_file": values[reference]}}, [storage()]
    )
    assert payload["roots"][0]["ssh_requirements"] == [
        {"remote": "vault", "key_path": expected, "agent": False}
    ]
    assert payload["roots"][0]["reason"] == "ssh_private_key_required"
    assert payload["files"] == {}
    recovery.validate_rclone(payload, [storage()])


@pytest.mark.asyncio
@pytest.mark.parametrize("reference", ["absolute", "relative", "tilde", "environment"])
async def test_native_ssh_source_references_preview_and_restore_with_missing_key(
    reference, ssh_peer, db_session, file_database, tmp_path, monkeypatch
):
    import subprocess
    from cryptography.fernet import Fernet
    from httpx import ASGITransport, AsyncClient
    from app import database, credential_encryption, main
    from app.config import settings
    from app.recovery_bundle import encode_bundle, decode_bundle
    from app.recovery_service import export_state, validate_state
    from tests.test_api_recovery import bind

    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("HOME", str(ssh_peer["home"]))
    monkeypatch.setenv("DV_SYNTHETIC_KEY_HOME", str(ssh_peer["home"]))
    values = {
        "absolute": str(ssh_peer["identity"]),
        "relative": "home/.ssh/id_ed25519",
        "tilde": "~/.ssh/id_ed25519",
        "environment": "$DV_SYNTHETIC_KEY_HOME/.ssh/id_ed25519",
    }
    config = tmp_path / "external.conf"
    config.write_text(
        f"[vault]\ntype=sftp\nhost=127.0.0.1\nport={ssh_peer['port']}\nuser=fixture\nkey_file={values[reference]}\nknown_hosts_file={ssh_peer['known']}\n"
    )
    native = subprocess.run(
        [
            recovery._binary(),
            "--config",
            str(config),
            "lsjson",
            "vault:",
            "--retries",
            "1",
            "--low-level-retries",
            "1",
        ],
        capture_output=True,
        timeout=10,
    )
    assert native.returncode == 0
    assert [item["Name"] for item in json.loads(native.stdout)] == ["unchanged.archive"]
    monkeypatch.setattr(settings, "RCLONE_CONFIG", str(config))
    db_session.add(
        database.RemoteStorage(
            id=1,
            name="SSH source",
            storage_type="rclone",
            rclone_remote="vault",
            base_path="/",
        )
    )
    await db_session.commit()
    payload = await export_state(db_session)
    bundle = encode_bundle(payload, "synthetic-review-password")
    decrypted = decode_bundle(bundle, "synthetic-review-password")
    preview = await validate_state(decrypted)
    assert preview["storages"][0]["state"] == "credentials_required"
    ssh_peer["identity"].unlink()
    ssh_peer["known"].unlink()
    config.unlink()
    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    bind(monkeypatch, file_database)
    async with AsyncClient(
        transport=ASGITransport(app=main.app), base_url="https://test"
    ) as client:
        request = {"bundle": bundle.decode(), "password": "synthetic-review-password"}
        response = await client.post("/api/v1/recovery/preview", json=request)
        assert response.status_code == 200
        assert response.json()["storages"][0]["state"] == "credentials_required"
        restored = await client.post(
            "/api/v1/recovery/restore",
            json={
                **request,
                "username": "restored-admin",
                "admin_password": "synthetic-account-password",
                "confirm_admin_password": "synthetic-account-password",
            },
        )
        assert restored.status_code == 200
        assert restored.json()["result"]["storages"] == [
            {"id": 1, "type": "rclone", "state": "credentials_required"}
        ]
        assert (await client.get("/api/v1/auth/me")).json()[
            "username"
        ] == "restored-admin"
    async with file_database() as session:
        state = await recovery.managed_state(session)
        code, output, error = await recovery.run_managed(
            state, "vault", ["lsjson", "vault:"], timeout=10
        )
        assert code != 0 and output == "" and error == "Storage credentials required"
        assert (await session.get(database.RemoteStorage, 1)).rclone_remote == "vault"
    assert (
        ssh_peer["depot"] / "unchanged.archive"
    ).read_bytes() == b"synthetic-original-archive-bytes"
