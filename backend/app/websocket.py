"""
WebSocket support for real-time updates.
"""

import asyncio
import json
import logging
from typing import Dict, Set

from fastapi import APIRouter, WebSocket, WebSocketDisconnect

from app.auth import get_session_user
from app.backup_engine import backup_engine
from app.database import async_session

logger = logging.getLogger(__name__)

router = APIRouter()


class ConnectionManager:
    """Best-effort notifications with one bounded queue/writer per client."""

    QUEUE_SIZE = 32
    SEND_TIMEOUT = 5.0
    CONTROL_TYPES = {"connected", "pong", "subscribed", "error"}

    def __init__(self):
        self.active_connections: Set[WebSocket] = set()
        self._lock = asyncio.Lock()
        self._queues: Dict[WebSocket, asyncio.Queue] = {}
        self._senders: Dict[WebSocket, asyncio.Task] = {}
        self._owned: Set[asyncio.Task] = set()
        self._closing: Set[WebSocket] = set()

    async def connect(self, websocket: WebSocket):
        await asyncio.wait_for(websocket.accept(), self.SEND_TIMEOUT)
        async with self._lock:
            self.active_connections.add(websocket)
            queue = asyncio.Queue(maxsize=self.QUEUE_SIZE)
            self._queues[websocket] = queue
            task = asyncio.create_task(self._send_loop(websocket, queue))
            self._senders[websocket] = task
            self._owned.add(task)
            task.add_done_callback(self._owned.discard)

    async def disconnect(self, websocket: WebSocket, code=1000, reason=""):
        async with self._lock:
            self._closing.discard(websocket)
            if websocket not in self.active_connections:
                return
            self.active_connections.discard(websocket)
            self._queues.pop(websocket, None)
            task = self._senders.pop(websocket, None)
        if task is not None and task is not asyncio.current_task():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        try:
            await asyncio.wait_for(
                websocket.close(code=code, reason=reason), self.SEND_TIMEOUT
            )
        except Exception:
            pass  # The peer may already be gone or unable to accept a close frame.

    async def _send_loop(self, websocket, queue):
        try:
            while True:
                message = await queue.get()
                try:
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
            await self.disconnect(
                websocket, code=1013, reason="Notification delivery stalled"
            )

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


@router.websocket("/updates")
async def websocket_endpoint(websocket: WebSocket):
    """WebSocket endpoint for real-time updates."""
    # Authenticate via cookie (sent automatically by the browser) or
    # fall back to a token query parameter for programmatic clients.
    token = websocket.cookies.get("session_token")
    if not token:
        token = websocket.query_params.get("token")

    if not token:
        await websocket.close(code=4001, reason="Authentication required")
        return

    async with async_session() as db:
        user = await get_session_user(token, db)
        if user is None:
            await websocket.close(code=4001, reason="Invalid or expired session")
            return

    await manager.connect(websocket)

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
