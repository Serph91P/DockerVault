"""
WebSocket support for real-time updates.
"""

import asyncio
import json
import logging
from typing import Dict, Set, cast
from urllib.parse import urlsplit

from fastapi import APIRouter, WebSocket, WebSocketDisconnect

from app.auth import _hash_token, get_session_user_by_hash
from app.config import settings
from app.backup_engine import backup_engine
from app.database import async_session

logger = logging.getLogger(__name__)

router = APIRouter()


class ConnectionManager:
    """Best-effort notifications with one bounded queue/writer per client."""

    QUEUE_SIZE = 32
    SEND_TIMEOUT = 5.0
    SESSION_CHECK_INTERVAL = 1.0
    CONTROL_TYPES = {"connected", "pong", "subscribed", "error"}

    def __init__(self):
        self.active_connections: Set[WebSocket] = set()
        self._lock = asyncio.Lock()
        self._queues: Dict[WebSocket, asyncio.Queue[str]] = {}
        self._senders: Dict[WebSocket, asyncio.Task] = {}
        self._owned: Set[asyncio.Task] = set()
        self._closing: Set[WebSocket] = set()
        self._sessions: Dict[WebSocket, tuple[str, int]] = {}

    async def connect(
        self,
        websocket: WebSocket,
        session_hash: str | None = None,
        user_id: int | None = None,
    ):
        if session_hash is not None and user_id is None:
            raise ValueError("A session association requires its authenticated user")
        await asyncio.wait_for(websocket.accept(), self.SEND_TIMEOUT)
        async with self._lock:
            self.active_connections.add(websocket)
            if session_hash is not None and user_id is not None:
                self._sessions[websocket] = (session_hash, user_id)
            queue: asyncio.Queue[str] = asyncio.Queue(maxsize=self.QUEUE_SIZE)
            self._queues[websocket] = queue
            task = asyncio.create_task(self._send_loop(websocket, queue))
            self._senders[websocket] = task
            self._owned.add(task)
            task.add_done_callback(self._owned.discard)

    async def disconnect(self, websocket: WebSocket, code=1000, reason=""):
        async with self._lock:
            if websocket not in self.active_connections:
                return
            task = self._detach(websocket)
        await self._close(websocket, task, code, reason)

    def _detach(self, websocket):
        self._closing.discard(websocket)
        self.active_connections.discard(websocket)
        self._sessions.pop(websocket, None)
        self._queues.pop(websocket, None)
        return self._senders.pop(websocket, None)

    async def revoke_sessions(self, hashes):
        async with self._lock:
            connections = [
                (websocket, self._detach(websocket))
                for websocket, (identity, _) in list(self._sessions.items())
                if identity in hashes
            ]
        await asyncio.gather(
            *(
                self._close(websocket, task, 4001, "Session revoked")
                for websocket, task in connections
            )
        )

    async def _close(self, websocket, task, code, reason):
        if task is not None and task is not asyncio.current_task():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        try:
            await asyncio.wait_for(
                websocket.close(code=code, reason=reason), self.SEND_TIMEOUT
            )
        except Exception:
            pass  # The peer may already be gone or unable to accept a close frame.

    async def _authorized(self, websocket):
        identity = self._sessions.get(websocket)
        if identity is None:
            return websocket in self.active_connections
        try:
            async with asyncio.timeout(self.SEND_TIMEOUT):
                async with async_session() as db:
                    user = await get_session_user_by_hash(identity[0], db)
                    return (
                        user is not None
                        and user.id == identity[1]
                        and websocket in self.active_connections
                    )
        except Exception:
            logger.warning(
                "WebSocket session validation unavailable; closing connection"
            )
            return False

    async def _send_loop(self, websocket, queue):
        code, reason = 1013, "Notification delivery stalled"
        try:
            while True:
                try:
                    message = await asyncio.wait_for(
                        queue.get(), self.SESSION_CHECK_INTERVAL
                    )
                except TimeoutError:
                    if not await self._authorized(websocket):
                        code, reason = 4001, "Session revoked or expired"
                        break
                    continue
                try:
                    if not await self._authorized(websocket):
                        code, reason = 4001, "Session revoked or expired"
                        break
                    await asyncio.wait_for(
                        websocket.send_text(message), self.SEND_TIMEOUT
                    )
                finally:
                    queue.task_done()
        except asyncio.CancelledError:
            raise
        except Exception as error:
            logger.warning("WebSocket sender failed: %s", error)
        finally:
            await self.disconnect(websocket, code=code, reason=reason)

    def _enqueue(self, websocket, encoded):
        queue = self._queues.get(websocket)
        if queue is None or websocket in self._closing:
            return
        if queue.full():
            retained = []
            dropped = False
            while not queue.empty():
                previous = queue.get_nowait()
                queue.task_done()
                if (
                    not dropped
                    and json.loads(previous).get("type") not in self.CONTROL_TYPES
                ):
                    dropped = True
                else:
                    retained.append(previous)
            for previous in retained:
                queue.put_nowait(previous)
            if not dropped:
                self._closing.add(websocket)
                task = asyncio.create_task(
                    self.disconnect(
                        websocket, code=1013, reason="Notification queue full"
                    )
                )
                self._owned.add(task)
                task.add_done_callback(self._owned.discard)
                return
        queue.put_nowait(encoded)

    async def broadcast(self, message: Dict):
        encoded = json.dumps(message)
        async with self._lock:
            for websocket in self.active_connections:
                self._enqueue(websocket, encoded)

    async def send_to_user(self, user_id: int, message: Dict):
        encoded = json.dumps(message)
        async with self._lock:
            for websocket, (_, owner) in self._sessions.items():
                if owner == user_id:
                    self._enqueue(websocket, encoded)

    async def send_to_client(self, websocket: WebSocket, message: Dict):
        async with self._lock:
            self._enqueue(websocket, json.dumps(message))

    async def shutdown(self):
        async with self._lock:
            connections = list(self.active_connections)
        await asyncio.gather(
            *(
                self.disconnect(ws, code=1001, reason="Server shutdown")
                for ws in connections
            )
        )
        tasks = list(self._owned)
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)


# Global connection manager
manager = ConnectionManager()


async def backup_progress_callback(backup_id: int, progress: float, message: str):
    """Callback for backup progress updates."""
    await manager.broadcast(
        {
            "type": "backup_progress",
            "backup_id": backup_id,
            "progress": progress,
            "message": message,
        }
    )


# Register callback with backup engine
backup_engine.add_progress_callback(backup_progress_callback)


def _allowed_origin(websocket: WebSocket, origin: str) -> bool:
    try:
        parsed = urlsplit(origin)
        if (
            parsed.scheme not in ("http", "https")
            or not parsed.netloc
            or parsed.path
            or parsed.query
            or parsed.fragment
            or parsed.username
            or parsed.password
        ):
            return False
        parsed.port  # Reject malformed origin ports.
    except ValueError:
        return False
    scheme = (
        "https" if websocket.url.scheme == "wss" or settings.COOKIE_SECURE else "http"
    )
    same_origin = f"{scheme}://{websocket.url.netloc}"
    allowed = {value.strip() for value in settings.CORS_ORIGINS.split(",")}
    return origin == same_origin or origin in allowed


@router.websocket("/updates")
async def websocket_endpoint(websocket: WebSocket):
    """WebSocket endpoint for real-time updates."""
    origin = websocket.headers.get("origin")
    explicit_token = websocket.query_params.get("token")
    token = (
        explicit_token
        if origin is None
        else websocket.cookies.get("session_token") or explicit_token
    )

    if not token:
        if origin is None and websocket.cookies.get("session_token"):
            await websocket.close(
                code=4003, reason="Origin required for cookie authentication"
            )
            return
        await websocket.close(code=4001, reason="Authentication required")
        return

    if origin is not None and not _allowed_origin(websocket, origin):
        await websocket.close(code=4003, reason="Origin not allowed")
        return

    session_hash = _hash_token(token)
    async with async_session() as db:
        user = await get_session_user_by_hash(session_hash, db)
        if user is None:
            await websocket.close(code=4001, reason="Invalid or expired session")
            return

    await manager.connect(
        websocket, session_hash=session_hash, user_id=cast(int, user.id)
    )

    try:
        # Send initial connection message
        await manager.send_to_client(
            websocket,
            {
                "type": "connected",
                "message": "Connected to backup manager",
            },
        )

        while True:
            # Wait for messages from client
            data = await websocket.receive_text()

            try:
                message = json.loads(data)
                await handle_client_message(websocket, message)
            except json.JSONDecodeError:
                await manager.send_to_client(
                    websocket,
                    {
                        "type": "error",
                        "message": "Invalid JSON",
                    },
                )

    except WebSocketDisconnect:
        await manager.disconnect(websocket)
    except Exception as e:
        logger.error(f"WebSocket error: {e}")
        await manager.disconnect(websocket)


async def handle_client_message(websocket: WebSocket, message: Dict):
    """Handle incoming client messages."""
    msg_type = message.get("type")

    if msg_type == "ping":
        await manager.send_to_client(websocket, {"type": "pong"})

    elif msg_type == "subscribe":
        # Subscribe to specific backup updates
        backup_id = message.get("backup_id")
        if backup_id:
            await manager.send_to_client(
                websocket,
                {
                    "type": "subscribed",
                    "backup_id": backup_id,
                },
            )

    else:
        await manager.send_to_client(
            websocket,
            {
                "type": "error",
                "message": f"Unknown message type: {msg_type}",
            },
        )


async def broadcast_backup_event(event_type: str, data: Dict):
    """Broadcast a backup event to all clients."""
    await manager.broadcast(
        {
            "type": event_type,
            **data,
        }
    )
