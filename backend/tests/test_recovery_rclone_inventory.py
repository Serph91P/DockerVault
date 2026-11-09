"""Pinned native option inventory: every typed scalar credential, not a transport claim."""

import base64
import json
import re
import subprocess

import pytest
from cryptography.fernet import Fernet

from app import rclone_recovery as recovery


def inventory():
    result = subprocess.run(
        [recovery._binary(), "--config", "/dev/null", "config", "providers"],
        env=recovery._environment(),
        capture_output=True,
        check=True,
        timeout=15,
    )
    providers = json.loads(result.stdout)
    cases = []
    for provider in providers:
        name = provider["Name"]
        for option in provider["Options"]:
            field = option["Name"]
            if field in recovery._FILE_OPTIONS.get(name, ()) or (
                name == "sftp" and field in recovery._SSH_PRIVATE
            ):
                continue
            if field.endswith("_command"):
                continue  # Executable credential sources are tested as external dependencies below.
            if type(option.get("Default")) is not str:
                continue
            if option.get("IsPassword") or re.search(
                r"pass|token|secret|(^|_)(key|credentials?|private)(_|$)|client_id|^account$|^user(name)?$",
                field,
            ):
                value = "synthetic-inventory-" + name + "-" + field
                if field == "token":
                    value = json.dumps(
                        {
                            "access_token": value,
                            "refresh_token": "synthetic-inventory-refresh",
                            "expiry": "2100-01-01T00:00:00Z",
                        }
                    )
                cases.append(pytest.param(name, field, value, id=name + "/" + field))
    return cases


@pytest.mark.asyncio
@pytest.mark.parametrize("provider,field,value", inventory())
async def test_every_native_scalar_credential_survives_encrypted_generation_and_reexport(
    provider, field, value, tmp_path, monkeypatch
):
    from app.config import settings
    from app import credential_encryption

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    config = {
        "vault": {"type": provider, field: value},
        "dependency": {"type": "memory"},
    }
    if provider in recovery._WRAPPERS:
        config["vault"]["remote"] = "dependency:fixture"
    if provider in {"union", "combine"}:
        config["vault"]["upstreams"] = (
            "dependency:fixture" if provider == "union" else "part=dependency:fixture"
        )
    storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
    payload = recovery.package_rclone(config, [storage])
    assert payload["remotes"]["vault"][field] == value
    recovery.validate_rclone(payload, [storage])
    state = await recovery.create_managed_config(payload, tmp_path / "generation")
    assert await recovery.export_managed_config(state, [storage]) == payload
    assert all(
        value.encode() not in path.read_bytes()
        for path in tmp_path.rglob("*")
        if path.is_file()
    )


def test_default_oci_config_and_nested_key_are_materialized_as_typed_dependencies(
    tmp_path, monkeypatch
):
    home = tmp_path / "synthetic-home"
    (home / ".oci").mkdir(parents=True)
    signing = home / ".oci" / "signing.pem"
    signing.write_bytes(b"synthetic-implicit-nonssh-signing-key")
    (home / ".oci" / "config").write_text(
        "[DEFAULT]\nuser=synthetic-user\ntenancy=synthetic-tenancy\nkey_file="
        + str(signing)
        + "\n"
    )
    monkeypatch.setenv("HOME", str(home))
    config = {
        "vault": {
            "type": "oracleobjectstorage",
            "provider": "user_principal_auth",
            "namespace": "synthetic",
            "region": "us-ashburn-1",
        }
    }
    storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
    payload = recovery.package_rclone(config, [storage])
    assert payload["roots"][0]["state"] == "portable"
    reference = payload["remotes"]["vault"]["config_file"]["file_ref"]
    assert payload["files"][reference]["dependencies"]
    assert any(
        base64.b64decode(record["content_b64"]) == signing.read_bytes()
        for record in payload["files"].values()
    )
    monkeypatch.setattr(
        recovery,
        "_read_material",
        lambda *_: pytest.fail("import read host credential material"),
    )
    recovery.validate_rclone(payload, [storage])


@pytest.mark.asyncio
async def test_external_credential_command_is_not_executed_or_exported_but_other_secrets_survive(
    tmp_path, monkeypatch
):
    from app.config import settings
    from app import credential_encryption

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    script = "never-execute-sensitive-external-credential-command"
    config = {
        "vault": {
            "type": "webdav",
            "url": "https://example.invalid",
            "bearer_token_command": script,
            "bearer_token": "synthetic-retained-bearer",
            "pass": "synthetic-retained-password",
        }
    }
    storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
    payload = recovery.package_rclone(config, [storage])
    assert payload["roots"][0]["state"] == "credentials_required"
    assert payload["remotes"]["vault"] == {
        key: value
        for key, value in config["vault"].items()
        if key != "bearer_token_command"
    }
    assert script not in json.dumps(payload)
    assert payload["roots"][0]["external_commands"] == [
        {"remote": "vault", "field": "bearer_token_command"}
    ]
    recovery.validate_rclone(payload, [storage])
    state = await recovery.create_managed_config(payload, tmp_path / "generation")
    code, output, error = await recovery.run_managed(
        state, "vault", ["lsjson", "vault:"], timeout=5
    )
    assert code != 0 and not output and error == "Storage credentials required"
    assert await recovery.export_managed_config(state, [storage]) == payload


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "reference",
    [
        ":memory:",
        ":memory,description='inline,both''quotes':",
        'shared,description="inline,both""quotes":',
        "local",
        "union-writeback",
    ],
)
async def test_native_connection_string_and_local_alias_dependencies_roundtrip_without_archive_content(
    reference, tmp_path, monkeypatch
):
    from app.config import settings
    from app import credential_encryption

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    folder = tmp_path / "existing-source"
    folder.mkdir()
    archive = folder / "unchanged.archive"
    archive.write_bytes(b"never-bundle-synthetic-archive-content")
    remote = str(folder) if reference == "local" else reference
    config = {
        "vault": {"type": "alias", "remote": remote},
        "shared": {"type": "memory"},
    }
    cache = tmp_path / "empty-cache"
    cache.mkdir()
    if reference == "union-writeback":
        config["vault"] = {
            "type": "union",
            "upstreams": f"{cache}:writeback {folder}:ro",
        }
    storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
    payload = recovery.package_rclone(config, [storage])
    assert payload["roots"][0]["state"] == "portable"
    assert archive.read_text() not in json.dumps(payload)
    recovery.validate_rclone(payload, [storage])
    state = await recovery.create_managed_config(payload, tmp_path / "generation")
    code, output, error = await recovery.run_managed(
        state, "vault", ["lsjson", "vault:"], timeout=10
    )
    assert code == 0 and error == ""
    assert [item["Name"] for item in json.loads(output)] == (
        ["unchanged.archive"] if reference in {"local", "union-writeback"} else []
    )
    assert list(cache.iterdir()) == []
    assert archive.read_bytes() == b"never-bundle-synthetic-archive-content"
    assert await recovery.export_managed_config(state, [storage]) == payload


def test_unportable_ambient_identity_does_not_discard_other_typed_provider_secrets():
    config = {
        "vault": {
            "type": "azureblob",
            "env_auth": "true",
            "client_secret": "synthetic-retain-configured-client-secret",
            "account": "synthetic-account",
        }
    }
    storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
    payload = recovery.package_rclone(config, [storage])
    assert payload["roots"][0]["state"] == "credentials_required"
    assert payload["remotes"]["vault"] == config["vault"]
    recovery.validate_rclone(payload, [storage])
