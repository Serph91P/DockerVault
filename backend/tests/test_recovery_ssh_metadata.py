"""Public reconnect references survive subsequent system exports without private files."""

import json

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ed25519

from app import database
from app.api import storage
from app.recovery_service import export_state, validate_state


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
