"""Real SQLite/API auth plus the actual WebSocket endpoint, no auth bypass."""

import asyncio
from datetime import datetime, timedelta, timezone
import json
from unittest.mock import patch

import pytest
import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from sqlalchemy import delete, update
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
from starlette.datastructures import URL
from starlette.websockets import WebSocketDisconnect

from app import main, websocket as ws
from app.auth import create_session, hash_password
from app.database import Base, Session, User


class Transport:
    def __init__(self, token, origin="http://test", query_token=None):
        self.cookies = {"session_token": token} if token else {}
        self.query_params = {"token": query_token} if query_token else {}
        self.headers = {"host": "test"}
        if origin is not None:
            self.headers["origin"] = origin
        self.url = URL("ws://test/ws/updates")
        self.accepted = False
        self.closed = asyncio.Event()
        self.close_code = None
        self.incoming = asyncio.Queue()
        self.outgoing = asyncio.Queue()
        self.messages = []

    async def accept(self):
        self.accepted = True

    async def close(self, code=1000, reason=""):
        self.close_code = code
        self.closed.set()
        self.incoming.put_nowait(None)

    async def send_text(self, encoded):
        message = json.loads(encoded)
        self.messages.append(message)
        self.outgoing.put_nowait(message)

    async def send_json(self, message):
        await self.send_text(json.dumps(message))

    async def receive_text(self):
        message = await self.incoming.get()
        if message is None:
            raise WebSocketDisconnect(self.close_code or 1000)
        return message


@pytest_asyncio.fixture
async def test_db(tmp_path):
    database = create_async_engine(f"sqlite+aiosqlite:///{tmp_path / 'sessions.db'}")
    async with database.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)
    yield async_sessionmaker(database, expire_on_commit=False)
    await database.dispose()


@pytest_asyncio.fixture
async def authenticated(test_db):
    manager = ws.ConnectionManager()
    async with test_db() as session:
        user = User(
            username="owned-admin",
            password_hash=hash_password("Synthetic-password-173"),
        )
        session.add(user)
        await session.commit()
        token = await create_session(user.id, session)
    with (
        patch("app.auth.async_session", test_db),
        patch("app.api.auth.async_session", test_db),
        patch("app.main.async_session", test_db),
        patch.object(ws, "async_session", test_db),
        patch.object(ws, "manager", manager),
        patch("app.config.settings.COOKIE_SECURE", False),
        patch("app.config.settings.CORS_ORIGINS", "https://trusted.test"),
    ):
        async with AsyncClient(
            transport=ASGITransport(app=main.app),
            base_url="http://test",
            cookies={"session_token": token},
        ) as client:
            yield manager, client, token
        await manager.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "cause,idle",
    [
        ("logout", False),
        ("password", False),
        ("expiry", False),
        ("delete", False),
        ("expiry", True),
        ("delete", True),
    ],
)
async def test_existing_socket_revoked_by_real_api_or_database(
    authenticated, test_db, cause, idle
):
    manager, client, token = authenticated
    socket = Transport(token)
    task = asyncio.create_task(ws.websocket_endpoint(socket))
    try:
        assert (await asyncio.wait_for(socket.outgoing.get(), 2))["type"] == "connected"
        if cause == "logout":
            response = await client.post("/api/v1/auth/logout")
            assert response.status_code == 200
        elif cause == "password":
            response = await client.post(
                "/api/v1/auth/change-password",
                json={
                    "current_password": "Synthetic-password-173",
                    "new_password": "Synthetic-password-173-new",
                    "confirm_password": "Synthetic-password-173-new",
                },
            )
            assert response.status_code == 200
        else:
            async with test_db() as session:
                statement = (
                    delete(Session)
                    if cause == "delete"
                    else update(Session).values(
                        expires_at=datetime.now(timezone.utc) - timedelta(seconds=1)
                    )
                )
                await session.execute(statement)
                await session.commit()
        if not idle:
            await manager.broadcast(
                {
                    "type": "backup_progress",
                    "backup_id": 77,
                    "progress": 50,
                    "message": "protected",
                }
            )
        waiter = asyncio.create_task(socket.closed.wait())
        done, _ = await asyncio.wait([waiter], timeout=2)
        waiter.cancel()
        await asyncio.gather(waiter, return_exceptions=True)
        assert done, "revoked socket remained authorized"
        assert socket.close_code == 4001
        assert not any(message.get("backup_id") == 77 for message in socket.messages)
    finally:
        await manager.disconnect(socket)
        await asyncio.wait_for(task, 2)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "policy,allowed,code",
    [
        ("same", True, None),
        ("configured", True, None),
        ("foreign", False, 4003),
        ("null", False, 4003),
        ("absent-cookie", False, 4003),
        ("absent-query", True, None),
        ("absent-invalid-query", False, 4001),
        ("missing", False, 4001),
        ("invalid", False, 4001),
    ],
)
async def test_browser_and_explicit_programmatic_origin_contract(
    authenticated, policy, allowed, code
):
    manager, _, token = authenticated
    origin = {
        "configured": "https://trusted.test",
        "foreign": "https://foreign.invalid",
        "null": "null",
    }.get(policy, "http://test")
    if policy.startswith("absent"):
        origin = None
    cookie = (
        None
        if policy in ("missing", "absent-query")
        else ("invalid" if policy == "invalid" else token)
    )
    query = (
        token
        if policy == "absent-query"
        else ("invalid" if policy == "absent-invalid-query" else None)
    )
    socket = Transport(cookie, origin, query)
    task = asyncio.create_task(ws.websocket_endpoint(socket))
    try:
        if allowed:
            assert (await asyncio.wait_for(socket.outgoing.get(), 2))[
                "type"
            ] == "connected"
            assert socket.accepted
        else:
            await asyncio.wait_for(socket.closed.wait(), 2)
            assert not socket.accepted and socket.close_code == code
    finally:
        await manager.disconnect(socket)
        if not task.done():
            socket.incoming.put_nowait(None)
        await asyncio.wait_for(task, 2)
