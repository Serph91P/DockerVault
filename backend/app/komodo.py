"""
Komodo integration for external orchestration.
Provides API client and webhook support for Komodo.
"""

import asyncio
import json
import logging
from datetime import datetime
from typing import Dict, List, Optional

import aiohttp

from app.config import settings

logger = logging.getLogger(__name__)


class KomodoClient:
    """Client for Komodo API integration."""

    def __init__(self):
        self.api_url = settings.KOMODO_API_URL.rstrip("/")
        self.api_key = settings.KOMODO_API_KEY
        self.enabled = settings.KOMODO_ENABLED
        self._session: Optional[aiohttp.ClientSession] = None
        self._ws: Optional[aiohttp.ClientWebSocketResponse] = None

    @property
    def session(self) -> aiohttp.ClientSession:
        """Get or create aiohttp session."""
        if self._session is None or self._session.closed:
            headers = {}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"
            self._session = aiohttp.ClientSession(headers=headers)
        return self._session

    async def close(self):
        """Close the client session."""
        if self._ws and not self._ws.closed:
            await self._ws.close()
        if self._session and not self._session.closed:
            await self._session.close()

    async def is_available(self) -> bool:
        """Check if Komodo is available."""
        if not self.enabled or not self.api_url:
            return False

        try:
            async with self.session.get(f"{self.api_url}/health", timeout=5) as resp:
                return resp.status == 200
        except Exception as e:
            logger.warning(f"Komodo health check failed: {e}")
            return False

    async def notify_backup_started(
        self,
        backup_id: int,
        target_name: str,
        containers: List[str],
    ) -> bool:
        """Notify Komodo that a backup is starting."""
        if not self.enabled:
            return True

        try:
            payload = {
                "event": "backup.started",
                "backup_id": backup_id,
                "target_name": target_name,
                "containers": containers,
                "timestamp": datetime.utcnow().isoformat(),
            }

            async with self.session.post(
                f"{self.api_url}/webhooks/backup",
                json=payload,
                timeout=10,
            ) as resp:
                if resp.status == 200:
                    logger.info(f"Notified Komodo of backup start: {backup_id}")
                    return True
                else:
                    logger.warning(f"Komodo notification failed: {resp.status}")
                    return False

        except Exception as e:
            logger.error(f"Failed to notify Komodo: {e}")
            return False

    async def notify_backup_completed(
        self,
        backup_id: int,
        target_name: str,
        success: bool,
        duration_seconds: int,
        file_size: Optional[int] = None,
        error_message: Optional[str] = None,
    ) -> bool:
        """Notify Komodo that a backup is completed."""
        if not self.enabled:
            return True

        try:
            payload = {
                "event": "backup.completed",
                "backup_id": backup_id,
                "target_name": target_name,
                "success": success,
                "duration_seconds": duration_seconds,
                "file_size": file_size,
                "error_message": error_message,
                "timestamp": datetime.utcnow().isoformat(),
            }

            async with self.session.post(
                f"{self.api_url}/webhooks/backup",
                json=payload,
                timeout=10,
            ) as resp:
                if resp.status == 200:
                    logger.info(f"Notified Komodo of backup completion: {backup_id}")
                    return True
                else:
                    logger.warning(f"Komodo notification failed: {resp.status}")
                    return False

        except Exception as e:
            logger.error(f"Failed to notify Komodo: {e}")
            return False

    async def request_container_stop(
        self,
        container_name: str,
        reason: str = "backup",
    ) -> bool:
        """Request Komodo to stop a container."""
        if not self.enabled:
            return True

        try:
            payload = {
                "action": "stop",
                "container": container_name,
                "reason": reason,
                "requester": "backup-manager",
            }

            async with self.session.post(
                f"{self.api_url}/containers/{container_name}/actions",
                json=payload,
                timeout=30,
            ) as resp:
                if resp.status == 200:
                    logger.info(f"Komodo stopped container: {container_name}")
                    return True
                else:
                    logger.warning(f"Komodo stop request failed: {resp.status}")
                    return False

        except Exception as e:
            logger.error(f"Failed to request Komodo container stop: {e}")
            return False

    async def request_container_start(
        self,
        container_name: str,
        reason: str = "backup_complete",
    ) -> bool:
        """Request Komodo to start a container."""
        if not self.enabled:
            return True

        try:
            payload = {
                "action": "start",
                "container": container_name,
                "reason": reason,
                "requester": "backup-manager",
            }

            async with self.session.post(
                f"{self.api_url}/containers/{container_name}/actions",
                json=payload,
                timeout=30,
            ) as resp:
                if resp.status == 200:
                    logger.info(f"Komodo started container: {container_name}")
                    return True
                else:
                    logger.warning(f"Komodo start request failed: {resp.status}")
                    return False

        except Exception as e:
            logger.error(f"Failed to request Komodo container start: {e}")
            return False

    async def get_container_status(self, container_name: str) -> Optional[Dict]:
        """Get container status from Komodo."""
        if not self.enabled:
            return None

        try:
            async with self.session.get(
                f"{self.api_url}/containers/{container_name}",
                timeout=10,
            ) as resp:
                if resp.status == 200:
                    return await resp.json()
                return None

        except Exception as e:
            logger.error(f"Failed to get container status from Komodo: {e}")
            return None

    async def connect_websocket(self, on_message: callable) -> bool:
        """Connect to Komodo WebSocket for real-time updates."""
        if not self.enabled:
            return False

        try:
            ws_url = self.api_url.replace("http", "ws") + "/ws"
            self._ws = await self.session.ws_connect(ws_url)

            # Send authentication
            await self._ws.send_json(
                {
                    "type": "auth",
                    "token": self.api_key,
                    "client": "backup-manager",
                }
            )

            # Start listening
            asyncio.create_task(self._websocket_listener(on_message))

            logger.info("Connected to Komodo WebSocket")
            return True

        except Exception as e:
            logger.error(f"Failed to connect to Komodo WebSocket: {e}")
            return False

    async def _websocket_listener(self, on_message: callable):
        """Listen for WebSocket messages."""
        try:
            async for msg in self._ws:
                if msg.type == aiohttp.WSMsgType.TEXT:
                    data = json.loads(msg.data)
                    await on_message(data)
                elif msg.type == aiohttp.WSMsgType.ERROR:
                    logger.error(f"WebSocket error: {self._ws.exception()}")
                    break
        except Exception as e:
            logger.error(f"WebSocket listener error: {e}")

    async def send_websocket_message(self, message: Dict) -> bool:
        """Send a message through WebSocket."""
        if not self._ws or self._ws.closed:
            return False

        try:
            await self._ws.send_json(message)
            return True
        except Exception as e:
            logger.error(f"Failed to send WebSocket message: {e}")
            return False


# Global Komodo client
komodo_client = KomodoClient()
