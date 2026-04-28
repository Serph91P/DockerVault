"""Tests for the SSH key management endpoints in app.api.storage."""

from __future__ import annotations

import os
import shutil
import stat
from pathlib import Path

import pytest

from app.api import storage as storage_api


@pytest.fixture(autouse=True)
def isolated_keys_dir(tmp_path, monkeypatch):
    keys_dir = tmp_path / "ssh_keys"
    keys_dir.mkdir()
    monkeypatch.setattr(storage_api, "_SSH_KEYS_DIR", keys_dir)
    yield keys_dir
    shutil.rmtree(keys_dir, ignore_errors=True)


def _ssh_keygen_available() -> bool:
    return shutil.which("ssh-keygen") is not None


@pytest.mark.skipif(not _ssh_keygen_available(), reason="ssh-keygen not installed")
async def test_generate_list_get_delete_roundtrip(async_client):
    # generate
    resp = await async_client.post(
        "/api/v1/storage/ssh-keys",
        json={"name": "unit_test", "comment": "ci"},
    )
    assert resp.status_code == 200, resp.text
    info = resp.json()
    assert info["name"] == "unit_test"
    assert info["public_key"].startswith("ssh-ed25519 ")
    private = Path(info["private_path"])
    assert private.is_file()
    # private key must be readable only by the owner
    assert stat.S_IMODE(private.stat().st_mode) & 0o077 == 0

    # list
    resp = await async_client.get("/api/v1/storage/ssh-keys")
    assert resp.status_code == 200
    names = [k["name"] for k in resp.json()]
    assert "unit_test" in names

    # get single
    resp = await async_client.get("/api/v1/storage/ssh-keys/unit_test")
    assert resp.status_code == 200
    assert resp.json()["public_key"] == info["public_key"]

    # download .pub
    resp = await async_client.get("/api/v1/storage/ssh-keys/unit_test/public")
    assert resp.status_code == 200
    assert resp.headers["content-type"].startswith("text/plain")

    # second create without overwrite must 409
    resp = await async_client.post(
        "/api/v1/storage/ssh-keys",
        json={"name": "unit_test"},
    )
    assert resp.status_code == 409

    # delete
    resp = await async_client.delete("/api/v1/storage/ssh-keys/unit_test")
    assert resp.status_code == 200
    assert not private.exists()


async def test_generate_rejects_bad_name(async_client):
    resp = await async_client.post(
        "/api/v1/storage/ssh-keys",
        json={"name": "../escape"},
    )
    assert resp.status_code == 400


async def test_get_unknown_returns_404(async_client):
    resp = await async_client.get("/api/v1/storage/ssh-keys/does_not_exist")
    assert resp.status_code == 404


async def test_install_unknown_method_rejected(async_client, isolated_keys_dir):
    # Pre-create a fake key pair so the route gets past the 404 check
    (isolated_keys_dir / "fake").write_text("private")
    (isolated_keys_dir / "fake.pub").write_text("ssh-ed25519 AAAA fake\n")
    os.chmod(isolated_keys_dir / "fake", 0o600)
    resp = await async_client.post(
        "/api/v1/storage/ssh-keys/fake/install",
        json={
            "host": "example.com",
            "port": 22,
            "username": "user",
            "password": "secret",
            "method": "bogus",
        },
    )
    assert resp.status_code == 400
    assert "Unknown install method" in resp.json()["detail"]
