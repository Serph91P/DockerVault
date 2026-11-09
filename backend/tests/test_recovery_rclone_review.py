"""Review regressions: public SSH custody and effective native source portability."""

import base64
import copy
import hashlib
import json

import pytest
from cryptography.hazmat.primitives import serialization as ssh
from cryptography.hazmat.primitives.asymmetric import ed25519

from app import rclone_recovery as recovery


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
