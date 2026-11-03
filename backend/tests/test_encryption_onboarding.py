"""Non-secret durable onboarding decisions; never rotate pending identities."""

import pytest
from sqlalchemy import select

from app.database import AppSettings, EncryptionConfig


@pytest.mark.asyncio
async def test_encryption_choice_survives_status_reload(async_client, test_db):
    status = await async_client.get("/api/v1/encryption/status")
    assert status.status_code == 200
    assert status.json().get("onboarding_deferred") is False
    for _ in range(2):
        response = await async_client.post("/api/v1/encryption/defer")
        assert response.status_code == 200
    status = await async_client.get("/api/v1/encryption/status")
    assert status.json()["onboarding_deferred"] is True
    assert status.json()["setup_completed"] is False
    assert status.json()["encryption_enabled"] is False
    async with test_db() as db:
        rows = (await db.execute(select(AppSettings))).scalars().all()
        assert [(row.key, row.value) for row in rows] == [
            ("encryption_onboarding_deferred", "true")
        ]
        assert (await db.execute(select(EncryptionConfig))).scalar_one_or_none() is None


@pytest.mark.asyncio
async def test_onboarding_choice_requires_a_session_cookie(async_client, test_db):
    async_client.cookies.clear()
    response = await async_client.post("/api/v1/encryption/defer")
    assert response.status_code == 401
    async with test_db() as db:
        assert (await db.execute(select(AppSettings))).scalar_one_or_none() is None


@pytest.mark.asyncio
async def test_setup_cannot_silently_replace_pending_public_identity(
    async_client, test_db
):
    async with test_db() as db:
        db.add(
            EncryptionConfig(
                public_key="existing-public-identity",
                setup_completed=False,
                encryption_enabled=True,
            )
        )
        await db.commit()
    response = await async_client.post("/api/v1/encryption/setup")
    assert response.status_code == 409
    async with test_db() as db:
        config = (await db.execute(select(EncryptionConfig))).scalar_one()
        assert config.public_key == "existing-public-identity"
        assert config.setup_completed is False
