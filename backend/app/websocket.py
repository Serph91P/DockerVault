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
    """Manages WebSocket connections."""

    def __init__(self):
        self.active_connections: Set[WebSocket] = set()
        self._lock = asyncio.Lock()

    async def connect(self, websocket: WebSocket):
        """Accept and store a new connection."""
        await websocket.accept()
        async with self._lock:
            self.active_connections.add(websocket)
        logger.info(
            f"WebSocket connected. Total connections: {len(self.active_connections)}"
        )

    async def disconnect(self, websocket: WebSocket):
        """Remove a connection."""
        async with self._lock:
            self.active_connections.discard(websocket)
        logger.info(
            f"WebSocket disconnected. Total connections: {len(self.active_connections)}"
        )

    async def broadcast(self, message: Dict):
        """Broadcast message to all connections."""
        if not self.active_connections:
            return

        message_json = json.dumps(message)
        disconnected = set()

        async with self._lock:
            for connection in self.active_connections:
                try:
                    await connection.send_text(message_json)
                except Exception as e:
                    logger.warning(f"Failed to send to WebSocket: {e}")
                    disconnected.add(connection)

            # Remove disconnected clients
            self.active_connections -= disconnected

    async def send_to_client(self, websocket: WebSocket, message: Dict):
        """Send message to specific client."""
        try:
            await websocket.send_json(message)
        except Exception as e:
            logger.warning(f"Failed to send to WebSocket: {e}")


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
    # Authenticate via token query parameter
    token = websocket.query_params.get("token")
    if token is None:
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
