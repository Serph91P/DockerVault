"""Public reconnect references survive subsequent system exports without private files."""

import json

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ed25519
from sqlalchemy import select

from app import database
from app.api import storage
from app.recovery_service import export_state, validate_state
from tests.test_recovery_activation import file_database as file_database


@pytest.fixture(params=["ed25519", "rsa", "ecdsa", "dsa", "certificate"])
def ssh_identity(request):
    from cryptography.hazmat.primitives.asymmetric import dsa, ec, rsa

    key = {
        "ed25519": ed25519.Ed25519PrivateKey.generate,
        "rsa": lambda: rsa.generate_private_key(public_exponent=65537, key_size=2048),
        "ecdsa": lambda: ec.generate_private_key(ec.SECP256R1()),
        "dsa": lambda: dsa.generate_private_key(key_size=1024),
        "certificate": ed25519.Ed25519PrivateKey.generate,
    }[request.param]()
    if request.param == "certificate":
        public = (
            serialization.SSHCertificateBuilder()
            .public_key(key.public_key())
            .type(serialization.SSHCertificateType.USER)
            .valid_after(0)
            .valid_before(2**64 - 1)
            .valid_for_all_principals()
            .sign(ed25519.Ed25519PrivateKey.generate())
            .public_bytes()
        )
    else:
        public = key.public_key().public_bytes(
            serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH
        )
    private = key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.OpenSSH,
        serialization.NoEncryption(),
    )
    return public.decode(), private.decode()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "restored", [False, True], ids=["managed-pub", "restored-reference"]
)
async def test_public_metadata_export_serializes_only_public_identity(
    db_session, tmp_path, monkeypatch, ssh_identity, restored
):
    from app.recovery_bundle import decode_bundle, encode_bundle

    monkeypatch.setattr(storage, "_SSH_KEYS_DIR", tmp_path)
    public, private = ssh_identity
    combined = public + " synthetic public comment\n" + private
    source = tmp_path / "combined.pub"
    if restored:
        db_session.add(
            database.AppSettings(
                key="recovery_ssh_public_keys",
                value=json.dumps([{"name": "combined", "public_key": combined}]),
            )
        )
        await db_session.commit()
    else:
        source.write_text(combined)
    payload = await export_state(db_session)
    decoded = decode_bundle(
        encode_bundle(payload, "synthetic-password"), "synthetic-password"
    )
    leaked = private.strip() in json.dumps(decoded).replace("\\n", "\n")
    assert not leaked, "V1 decrypted public metadata contains a private key"
    public_only = decoded["ssh_public_keys"] == [
        {"name": "combined", "public_key": public}
    ]
    assert public_only, "Public identity/name was not canonically preserved"
    await validate_state(decoded)
    if restored:
        value = await db_session.scalar(
            select(database.AppSettings.value).where(
                database.AppSettings.key == "recovery_ssh_public_keys"
            )
        )
        unchanged = json.loads(value)[0]["public_key"] == combined
    else:
        unchanged = source.read_text() == combined
    assert unchanged, "Export changed its source"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "combined", [False, True], ids=["public-control", "private-suffix"]
)
async def test_authenticated_public_metadata_import_never_persists_private_suffix(
    file_database, db_session, tmp_path, monkeypatch, ssh_identity, combined, request
):
    from httpx import ASGITransport, AsyncClient

    from app import main
    from app.recovery_activation import generation_directory
    from app.recovery_bundle import decode_bundle, encode_bundle
    from app.recovery_service import state_manifest
    from tests.test_api_recovery import bind

    keys = tmp_path / "keys"
    keys.mkdir()
    monkeypatch.setattr(storage, "_SSH_KEYS_DIR", keys)
    public, private = ssh_identity
    payload = await export_state(db_session)
    payload["ssh_public_keys"] = [
        {
            "name": "original",
            "public_key": public
            + " synthetic comment\n"
            + (private if combined else ""),
        }
    ]
    payload["manifest"] = state_manifest(payload)
    bundle = encode_bundle(payload, "synthetic-password").decode()
    bind(monkeypatch, file_database)
    async with AsyncClient(
        transport=ASGITransport(app=main.app, client=(request.node.name, 123)),
        base_url="https://test",
    ) as client:
        preview = await client.post(
            "/api/v1/recovery/preview",
            json={"bundle": bundle, "password": "synthetic-password"},
        )
        assert preview.status_code == 200
        restored = await client.post(
            "/api/v1/recovery/restore",
            json={
                "bundle": bundle,
                "password": "synthetic-password",
                "username": "new-admin",
                "admin_password": "synthetic-admin-password",
                "confirm_admin_password": "synthetic-admin-password",
            },
        )
        assert restored.status_code == 200
        status = await client.get("/api/v1/recovery/status")
        assert status.status_code == 200
        public_only = status.json()["ssh_public_keys"] == [
            {"name": "original", "public_key": public}
        ]
        assert public_only, "Status must expose only the public identity"
        leaked = private.strip() in restored.text or private.strip() in status.text
        assert not leaked
        async with file_database() as session:
            values = (await session.scalars(select(database.AppSettings.value))).all()
            private_in_sqlite = any(
                private.strip() in value.replace("\\n", "\n") for value in values
            )
            assert not private_in_sqlite, "Private SSH bytes persisted in SQLite"
            activation = json.loads(
                await session.scalar(
                    select(database.AppSettings.value).where(
                        database.AppSettings.key == "recovery_activation"
                    )
                )
            )
        directory = generation_directory(file_database, activation["generation"])
        before = {
            path.relative_to(directory): path.read_bytes()
            for path in directory.rglob("*")
            if path.is_file()
        }
        private_in_generation = any(
            private.encode().strip() in data for data in before.values()
        )
        assert not private_in_generation, "Private SSH bytes persisted in generation"
        exported = await client.post(
            "/api/v1/recovery/export",
            json={
                "password": "synthetic-password",
                "confirm_password": "synthetic-password",
            },
        )
        assert exported.status_code == 200
        reexported = decode_bundle(exported.content, "synthetic-password")
        public_only = reexported["ssh_public_keys"] == [
            {"name": "original", "public_key": public}
        ]
        assert public_only, "Restored public metadata must survive a real reexport"
        after = {
            path.relative_to(directory): path.read_bytes()
            for path in directory.rglob("*")
            if path.is_file()
        }
        assert before == after


def public_key():
    return (
        ed25519.Ed25519PrivateKey.generate()
        .public_key()
        .public_bytes(
            serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH
        )
        .decode()
    )


@pytest.mark.asyncio
async def test_restored_public_references_reexport_without_any_private_counterpart(
    db_session, tmp_path, monkeypatch
):
    monkeypatch.setattr(storage, "_SSH_KEYS_DIR", tmp_path)
    restored = {"name": "original", "public_key": public_key()}
    current = {"name": "current", "public_key": public_key()}
    (tmp_path / "current.pub").write_text(current["public_key"])
    private = tmp_path / "current"
    private.write_text("never-export-private-ssh-canary")
    db_session.add(
        database.AppSettings(
            key="recovery_ssh_public_keys", value=json.dumps([restored])
        )
    )
    await db_session.commit()
    payload = await export_state(db_session)
    assert payload["ssh_public_keys"] == [current, restored]
    assert "never-export-private-ssh-canary" not in json.dumps(payload)
    assert private.read_text() == "never-export-private-ssh-canary"
    await validate_state(payload)


@pytest.mark.asyncio
async def test_recovery_status_exposes_public_references_not_private_key_availability(
    async_client, db_session
):
    restored = {"name": "original", "public_key": public_key()}
    result = {
        "storages": [],
        "catalog": [],
        "komodo": {"state": "disabled"},
        "counts": {},
        "archives_modified": False,
    }
    db_session.add_all(
        [
            database.AppSettings(
                key="recovery_ssh_public_keys", value=json.dumps([restored])
            ),
            database.AppSettings(key="recovery_result", value=json.dumps(result)),
        ]
    )
    await db_session.commit()
    from app.auth import create_session

    user = database.User(
        username="operator", password_hash="unused-in-session-lookup", is_admin=True
    )
    db_session.add(user)
    await db_session.commit()
    token = await create_session(user.id, db_session)
    async_client.cookies.set("session_token", token)
    response = await async_client.get("/api/v1/recovery/status")
    assert response.status_code == 200
    assert response.json()["ssh_public_keys"] == [restored]
    assert "private_path" not in response.text
