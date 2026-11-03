"""Full application route resolution, including real setup/session middleware."""

from contextlib import ExitStack
from unittest.mock import patch

from httpx import ASGITransport, AsyncClient
import pytest

from tests.test_remote_object_ownership import test_engine  # noqa: F401


@pytest.mark.asyncio
async def test_static_and_numeric_routes_with_real_auth(async_client, test_db):
    # Reuse the test-owned paused scheduler/database, but restore real auth functions.
    from app import auth
    from app.main import app

    with ExitStack() as stack:
        for module in ("app.main",):
            stack.enter_context(
                patch(f"{module}.is_setup_complete", auth.is_setup_complete)
            )
            stack.enter_context(
                patch(f"{module}.get_session_user", auth.get_session_user)
            )
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="https://test"
        ) as client:
            for path in ("jobs", "cron-help"):
                assert (
                    await client.get(f"/api/v1/schedules/{path}")
                ).status_code == 403
            setup = await client.post(
                "/api/v1/auth/setup",
                json={
                    "username": "owned-route-admin",
                    "password": "Synthetic-Route-Only-42!",
                    "confirm_password": "Synthetic-Route-Only-42!",
                },
            )
            assert setup.status_code == 200
            assert client.cookies.get("session_token")
            jobs = await client.get("/api/v1/schedules/jobs")
            help_response = await client.get("/api/v1/schedules/cron-help")
            assert jobs.status_code == help_response.status_code == 200
            assert jobs.json() == []
            help_body = help_response.json()
            assert help_body["format"] == "minute hour day month weekday"
            assert "Monday=0" in help_body["contract"]
            assert any(e["expression"] == "0 3 * * sun" for e in help_body["examples"])
            created = await client.post(
                "/api/v1/schedules",
                json={"name": "owned", "cron_expression": "0 2 * * *"},
            )
            assert created.status_code == 200
            sid = created.json()["id"]
            assert (await client.get(f"/api/v1/schedules/{sid}")).json()["id"] == sid
            assert (await client.get("/api/v1/schedules/999999")).status_code == 404
            assert (await client.get("/api/v1/schedules/not-an-id")).status_code == 404
            assert (await client.get("/api/v1/schedules")).status_code == 200
            await client.post("/api/v1/auth/logout")
            assert (await client.get("/api/v1/schedules/jobs")).status_code == 401
