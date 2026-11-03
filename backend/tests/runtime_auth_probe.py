"""Exercise real auth routes in an isolated process, without mocks or token output."""

import asyncio
import json
import sys

from httpx import ASGITransport, AsyncClient
from app.database import engine
from app.main import app

PASSWORD = "Synthetic-test-password-148"
NEW_PASSWORD = "Synthetic-test-password-148-new"


async def probe(mode, username):
    async with AsyncClient(
        transport=ASGITransport(app=app, raise_app_exceptions=False),
        base_url="https://auth-test",
    ) as client:
        response = await client.post(
            "/api/v1/auth/setup",
            json={
                "username": username,
                "password": PASSWORD,
                "confirm_password": PASSWORD,
            },
        )
        cookie = response.headers.get("set-cookie", "")
        result = {
            "setup": response.status_code,
            "session_cookie": "session_token=" in cookie,
            "cookie_httponly": "HttpOnly" in cookie,
            "cookie_secure": "Secure" in cookie,
            "cookie_samesite": "SameSite=lax" in cookie,
        }
        if mode == "sequential":
            result["repeat_setup"] = (
                await client.post(
                    "/api/v1/auth/setup",
                    json={
                        "username": "another-admin",
                        "password": PASSWORD,
                        "confirm_password": PASSWORD,
                    },
                )
            ).status_code
            result["me_after_setup"] = (await client.get("/api/v1/auth/me")).status_code
            result["logout"] = (await client.post("/api/v1/auth/logout")).status_code
            result["me_after_logout"] = (
                await client.get("/api/v1/auth/me")
            ).status_code
            result["login"] = (
                await client.post(
                    "/api/v1/auth/login",
                    json={"username": username, "password": PASSWORD},
                )
            ).status_code
            result["change_password"] = (
                await client.post(
                    "/api/v1/auth/change-password",
                    json={
                        "current_password": PASSWORD,
                        "new_password": NEW_PASSWORD,
                        "confirm_password": NEW_PASSWORD,
                    },
                )
            ).status_code
            result["me_after_change"] = (
                await client.get("/api/v1/auth/me")
            ).status_code
            await client.post("/api/v1/auth/logout")
            result["old_password"] = (
                await client.post(
                    "/api/v1/auth/login",
                    json={"username": username, "password": PASSWORD},
                )
            ).status_code
            result["new_password"] = (
                await client.post(
                    "/api/v1/auth/login",
                    json={"username": username, "password": NEW_PASSWORD},
                )
            ).status_code
    await engine.dispose()
    print("RESULT=" + json.dumps(result), flush=True)


if __name__ == "__main__":
    mode, username = sys.argv[1:]
    if mode == "race":
        print("READY", flush=True)
        if sys.stdin.read(1) != "!":
            raise RuntimeError("Missing owned test start signal")
    if mode == "session-failure":
        from unittest.mock import patch
        from app.api import auth

        async def fail_session(*_args, **_kwargs):
            raise RuntimeError("injected session persistence failure")

        with patch.object(auth, "create_session", fail_session):
            asyncio.run(probe(mode, username))
    else:
        asyncio.run(probe(mode, username))
