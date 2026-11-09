"""Native credential rotation and durable re-export, synthetic loopback providers only."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

import pytest
from cryptography.fernet import Fernet


@pytest.mark.asyncio
async def test_native_oauth_refresh_survives_new_process_and_product_reexport(
    db_session, tmp_path, monkeypatch, caplog, capsys
):
    from app import credential_encryption
    from app.config import settings
    from app.database import AppSettings
    from app import rclone_recovery as m

    monkeypatch.setattr(
        settings, "CREDENTIAL_ENCRYPTION_KEY", Fernet.generate_key().decode()
    )
    monkeypatch.setattr(credential_encryption, "_fernet", None)
    old_access = "synthetic-recovery-expired-access"
    old_refresh = "synthetic-recovery-original-refresh"
    new_access = "synthetic-recovery-rotated-access"
    new_refresh = "synthetic-recovery-rotated-refresh"
    client_secret = "synthetic-recovery-client-secret"
    requests = []

    class Endpoint(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def do_POST(self):
            form = parse_qs(
                self.rfile.read(int(self.headers.get("Content-Length", "0"))).decode()
            )
            valid = (
                self.path == "/token"
                and form.get("grant_type") == ["refresh_token"]
                and form.get("refresh_token") == [old_refresh]
            )
            requests.append(valid)
            body = json.dumps(
                {
                    "access_token": new_access,
                    "refresh_token": new_refresh,
                    "token_type": "Bearer",
                    "expires_in": 3600,
                }
                if valid
                else {"error": "invalid_grant"}
            ).encode()
            self.send_response(200 if valid else 400)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Endpoint)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        storage = {"id": 1, "storage_type": "rclone", "rclone_remote": "vault"}
        token = {
            "access_token": old_access,
            "refresh_token": old_refresh,
            "token_type": "Bearer",
            "expiry": "2000-01-01T00:00:00Z",
        }
        payload = m.package_rclone(
            {
                "vault": {
                    "type": "drive",
                    "client_id": "synthetic-client",
                    "client_secret": client_secret,
                    "token": json.dumps(token),
                    "root_folder_id": "synthetic-root",
                    "scope": "drive.readonly",
                    "token_url": f"http://127.0.0.1:{server.server_port}/token",
                }
            },
            [storage],
            m.provider_options(),
        )
        state = await m.create_managed_config(payload, tmp_path / "generation")
        db_session.add(AppSettings(key="recovery_rclone", value=json.dumps(state)))
        await db_session.commit()
        assert requests == []
        code, output, error = await m.run_managed(
            state,
            "vault",
            [
                "lsjson",
                "vault:",
                "--bind",
                "127.0.0.1",
                "--timeout",
                "1s",
                "--contimeout",
                "1s",
                "--retries",
                "1",
                "--low-level-retries",
                "1",
            ],
            timeout=15,
        )
        # Provider data is deliberately unreachable; only the loopback token service succeeds.
        assert code != 0 and not output and error == "Remote storage unavailable"
        assert requests == [True]
        monkeypatch.setattr(settings, "RCLONE_CONFIG", "/nonexistent/foreign-config")
        # Product reads persisted DB descriptor and a NEW native config-dump process.
        restored = await m.export_rclone(db_session, [storage])
        refreshed = json.loads(restored["remotes"]["vault"]["token"])
        assert refreshed["access_token"] == new_access
        assert refreshed["refresh_token"] == new_refresh
        assert refreshed["expiry"] != token["expiry"]
        expected = json.loads(json.dumps(payload))
        expected["remotes"]["vault"]["token"] = restored["remotes"]["vault"]["token"]
        assert restored == expected
        assert requests == [True]
        captured = capsys.readouterr()
        public = (captured.out + captured.err + caplog.text + error).encode()
        secrets = [
            value.encode()
            for value in (
                old_access,
                old_refresh,
                new_access,
                new_refresh,
                client_secret,
            )
        ]
        assert not any(secret in public for secret in secrets)
        assert all(
            not any(secret in path.read_bytes() for secret in secrets)
            for path in tmp_path.rglob("*")
            if path.is_file()
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
