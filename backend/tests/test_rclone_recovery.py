"""Recovery must carry dependency closure and non-SSH provider key material."""

import importlib
import json

import pytest


OPTIONS = {
    "s3": {
        "provider",
        "access_key_id",
        "secret_access_key",
        "session_token",
        "region",
        "env_auth",
        "shared_credentials_file",
    },
    "drive": {
        "token",
        "client_id",
        "client_secret",
        "service_account_file",
        "service_account_credentials",
        "scope",
        "token_url",
    },
    "crypt": {"remote", "password", "password2", "filename_encryption"},
    "alias": {"remote"},
    "union": {"upstreams"},
    "combine": {"upstreams"},
    "sftp": {
        "host",
        "user",
        "pass",
        "key_pem",
        "key_file",
        "key_file_pass",
        "key_use_agent",
        "known_hosts_file",
        "server_command",
    },
    "azureblob": {
        "client_certificate_path",
        "client_certificate_password",
        "service_principal_file",
    },
    "box": {"box_config_file", "token"},
}


def module():
    return importlib.import_module("app.rclone_recovery")


def root(name="vault", storage_id=1):
    return {"id": storage_id, "storage_type": "rclone", "rclone_remote": name}


def test_minimal_transitive_closure_keeps_all_credential_fields():
    m = module()
    config = {
        "vault": {
            "type": "crypt",
            "remote": "alias:folder",
            "password": "synthetic-crypt",
            "password2": "synthetic-salt",
        },
        "alias": {"type": "alias", "remote": "cloud:bucket"},
        "cloud": {
            "type": "s3",
            "provider": "AWS",
            "access_key_id": "synthetic-access",
            "secret_access_key": "synthetic-secret",
            "session_token": "synthetic-session",
            "region": "eu-test",
        },
        "unrelated": {"type": "drive", "token": "never-export-unrelated"},
    }
    result = m.package_rclone(config, [root(), root("cloud", 2)], OPTIONS)
    assert set(result["remotes"]) == {"vault", "alias", "cloud"}
    for name in result["remotes"]:
        assert result["remotes"][name] == config[name]
    assert all(x["state"] == "portable" for x in result["roots"])
    assert "never-export-unrelated" not in json.dumps(result)
    m.validate_rclone(result, [root(), root("cloud", 2)], OPTIONS)


@pytest.mark.parametrize(
    "provider,field,material",
    [
        (
            "drive",
            "service_account_file",
            b'{"private_key":"synthetic-nonssh-private","client_email":"test@example.invalid"}',
        ),
        (
            "s3",
            "shared_credentials_file",
            b"[default]\naws_access_key_id=synthetic-access\naws_secret_access_key=synthetic-secret\n",
        ),
        (
            "box",
            "box_config_file",
            b'{"boxAppSettings":{"appAuth":{"privateKey":"synthetic-box-key"}}}',
        ),
        ("azureblob", "client_certificate_path", b"synthetic-pkcs12-binary\x00\x01"),
        (
            "azureblob",
            "service_principal_file",
            b'{"clientSecret":"synthetic-principal-secret"}',
        ),
        ("sftp", "known_hosts_file", b"example.invalid ssh-ed25519 AAAA\n"),
    ],
)
def test_required_nonssh_files_are_included_not_blanket_excluded(
    provider, field, material, tmp_path
):
    m = module()
    path = tmp_path / "provider-material"
    path.write_bytes(material)
    config = {"vault": {"type": provider, field: str(path)}}
    result = m.package_rclone(config, [root()], OPTIONS)
    assert result["roots"][0]["state"] == "portable"
    reference = result["remotes"]["vault"][field]
    assert set(reference) == {"file_ref"}
    import base64

    assert (
        base64.b64decode(result["files"][reference["file_ref"]]["content_b64"])
        == material
    )
    assert str(path) not in json.dumps(result)
    m.validate_rclone(result, [root()], OPTIONS)


def test_inline_service_account_and_oauth_tokens_roundtrip():
    m = module()
    config = {
        "vault": {
            "type": "drive",
            "service_account_credentials": '{"private_key":"synthetic-nonssh-signing-key"}',
            "token": '{"access_token":"synthetic-access","refresh_token":"synthetic-refresh"}',
            "client_id": "synthetic-client",
            "client_secret": "synthetic-client-secret",
        }
    }
    result = m.package_rclone(config, [root()], OPTIONS)
    assert result["remotes"]["vault"] == config["vault"]
    assert result["roots"][0]["state"] == "portable"


@pytest.mark.parametrize(
    "field,value",
    [
        ("key_pem", "never-export-private-ssh"),
        ("key_file", "/never/open/private-ssh"),
        ("key_use_agent", "true"),
        ("server_command", "never-execute-command"),
    ],
)
def test_private_ssh_or_execution_dependency_blocks_root_without_opening_key(
    field, value
):
    m = module()
    config = {
        "vault": {"type": "crypt", "remote": "ssh:backups", "password": "crypt"},
        "ssh": {
            "type": "sftp",
            "host": "example.invalid",
            "pass": "synthetic-password",
            field: value,
        },
    }
    result = m.package_rclone(config, [root()], OPTIONS)
    assert result["roots"][0]["state"] == "credentials_required"
    assert result["remotes"] == {} and result["files"] == {}
    assert value not in json.dumps(result)


@pytest.mark.parametrize(
    "config",
    [
        {"vault": {"type": "alias", "remote": "missing:folder"}},
        {"vault": {"type": "alias", "remote": "vault:folder"}},
        {"vault": {"type": "alias", "remote": ":s3,env_auth=true:bucket"}},
        {"vault": {"type": "alias", "remote": "cloud,param=value:bucket"}},
        {
            "vault": {"type": "union", "upstreams": "cloud:folder:writeback"},
            "cloud": {"type": "s3"},
        },
        {"vault": {"type": "s3", "unknown_command": "do not execute"}},
        {"vault": {"type": "s3", "env_auth": "true"}},
    ],
)
def test_unsafe_or_unresolved_source_is_not_advertised_as_portable(config):
    result = module().package_rclone(config, [root()], OPTIONS)
    assert result["roots"][0]["state"] == "credentials_required"
    assert result["remotes"] == {}


def test_quoted_union_combine_dependencies_preserve_semantics():
    config = {
        "vault": {
            "type": "combine",
            "upstreams": '"one=union:folder with spaces" two=cloud:other',
        },
        "union": {
            "type": "union",
            "upstreams": '"cloud:first with space:ro" cloud:second:nc',
        },
        "cloud": {
            "type": "s3",
            "access_key_id": "synthetic-access",
            "secret_access_key": "synthetic-secret",
        },
    }
    m = module()
    result = m.package_rclone(config, [root()], OPTIONS)
    assert result["roots"][0]["state"] == "portable"
    assert result["remotes"] == config
    m.validate_rclone(result, [root()], OPTIONS)


@pytest.mark.asyncio
async def test_native_encrypted_config_materialization_reexport_and_secret_scan(
    tmp_path, monkeypatch
):
    from app import credential_encryption
    from app.config import settings
    from cryptography.fernet import Fernet

    # Isolated local key; no production key file or provider credentials.
    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    m = module()
    config = {
        "vault": {
            "type": "drive",
            "token": '{"access_token":"synthetic-native-access","refresh_token":"synthetic-native-refresh","expiry":"2099-01-01T00:00:00Z"}',
            "client_id": "synthetic-client",
            "client_secret": "synthetic-native-secret",
            "scope": "drive.readonly",
        }
    }
    payload = m.package_rclone(config, [root()], m.provider_options())
    state = await m.create_managed_config(payload, tmp_path / "generation")
    assert state["unlock"].startswith("fernet:")
    assert await m.export_managed_config(state, [root()]) == payload
    secrets = [
        b"synthetic-native-access",
        b"synthetic-native-refresh",
        b"synthetic-native-secret",
    ]
    assert all(
        not any(secret in path.read_bytes() for secret in secrets)
        for path in tmp_path.rglob("*")
        if path.is_file()
    )
    assert (tmp_path / "generation").stat().st_mode & 0o777 == 0o700
    assert (tmp_path / "generation" / "rclone.conf").stat().st_mode & 0o777 == 0o600
    assert (
        (tmp_path / "generation" / "rclone.conf")
        .read_bytes()
        .startswith(b"# Encrypted rclone configuration File")
    )


@pytest.mark.asyncio
async def test_native_provider_file_is_used_without_plaintext_disk(
    tmp_path, monkeypatch
):
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from app.config import settings
    from app import credential_encryption
    from cryptography.fernet import Fernet

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    observed = []

    class Endpoint(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            observed.append(
                "synthetic-file-access" in self.headers.get("Authorization", "")
            )
            body = b'<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>fixture</Name><Prefix></Prefix><IsTruncated>false</IsTruncated></ListBucketResult>'
            self.send_response(200)
            self.send_header("Content-Type", "application/xml")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Endpoint)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        material = tmp_path / "source.ini"
        material.write_text(
            "[default]\naws_access_key_id=synthetic-file-access\naws_secret_access_key=synthetic-file-secret\n"
        )
        m = module()
        config = {
            "vault": {
                "type": "s3",
                "provider": "Other",
                "shared_credentials_file": str(material),
                "endpoint": f"http://127.0.0.1:{server.server_port}",
                "region": "us-east-1",
                "force_path_style": "true",
                "env_auth": "true",
            }
        }
        payload = m.package_rclone(config, [root()], m.provider_options())
        state = await m.create_managed_config(payload, tmp_path / "generation")
        material.unlink()
        code, output, error = await m.run_managed(
            state,
            "vault",
            ["lsjson", "vault:fixture", "--retries", "1", "--low-level-retries", "1"],
            timeout=10,
        )
        assert code == 0 and json.loads(output) == [] and error == ""
        assert observed and all(observed)
        assert await m.export_managed_config(state, [root()]) == payload
        for path in tmp_path.rglob("*"):
            if path.is_file():
                assert b"synthetic-file-access" not in path.read_bytes()
                assert b"synthetic-file-secret" not in path.read_bytes()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


@pytest.mark.asyncio
async def test_export_uses_explicit_source_then_active_generation(
    db_session, tmp_path, monkeypatch
):
    from app.config import settings
    from app.database import AppSettings
    from app import credential_encryption
    from cryptography.fernet import Fernet

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    m = module()
    source = m.package_rclone(
        {"vault": {"type": "drive", "client_secret": "synthetic-source-secret"}},
        [root()],
        m.provider_options(),
    )
    state = await m.create_managed_config(source, tmp_path / "source")
    monkeypatch.setattr(
        settings,
        "RCLONE_CONFIG",
        str(tmp_path / "source" / "rclone.conf"),
        raising=False,
    )
    from pydantic import SecretStr

    monkeypatch.setattr(
        settings,
        "RCLONE_CONFIG_PASS",
        SecretStr(credential_encryption.decrypt_value(state["unlock"])),
    )
    assert await m.export_rclone(db_session, [root()]) == source
    db_session.add(AppSettings(key="recovery_rclone", value=json.dumps(state)))
    await db_session.commit()
    monkeypatch.setattr(settings, "RCLONE_CONFIG", "/nonexistent/unrelated-config")
    assert await m.export_rclone(db_session, [root()]) == source


@pytest.mark.asyncio
async def test_product_runner_prefers_generation_over_foreign_default_and_environment(
    db_session, test_db, tmp_path, monkeypatch
):
    from app import database
    from app.remote_storage import RcloneStorage, StorageConfig, StorageType

    m = module()
    payload = m.package_rclone(
        {"vault": {"type": "memory"}}, [root()], m.provider_options()
    )
    state = await m.create_managed_config(payload, tmp_path / "generation")
    db_session.add(database.AppSettings(key="recovery_rclone", value=json.dumps(state)))
    await db_session.commit()
    monkeypatch.setattr(database, "async_session", test_db)
    external = tmp_path / "foreign.conf"
    external.write_text("[vault]\ntype=alias\nremote=" + str(tmp_path) + "\n")
    (tmp_path / "foreign-sentinel.txt").write_text("foreign synthetic data")
    monkeypatch.setenv("RCLONE_CONFIG", str(external))
    backend = RcloneStorage(
        StorageConfig(
            id=1, name="fixture", storage_type=StorageType.RCLONE, rclone_remote="vault"
        )
    )
    code, output, error = await backend._run_rclone(["lsjson", "vault:"])
    assert code == 0 and json.loads(output) == [] and error == ""


def test_import_rejects_unreferenced_config_and_private_ssh():
    m = module()
    result = m.package_rclone({"vault": {"type": "s3"}}, [root()], OPTIONS)
    result["remotes"]["unrelated"] = {
        "type": "drive",
        "client_secret": "synthetic-unrelated",
    }
    with pytest.raises(m.RcloneRecoveryError):
        m.validate_rclone(result, [root()], OPTIONS)
    result["remotes"] = {"vault": {"type": "sftp", "key_pem": "synthetic-forbidden"}}
    with pytest.raises(m.RcloneRecoveryError):
        m.validate_rclone(result, [root()], OPTIONS)
