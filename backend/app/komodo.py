"""
Komodo integration for external orchestration.

Komodo uses a JSON-RPC-style API where all requests are ``POST`` to
one of ``/read``, ``/write``, or ``/execute`` with a body of the form
``{"type": "<RequestType>", "params": {...}}``.

Authentication is via ``X-Api-Key`` / ``X-Api-Secret`` headers.

Reference: https://docs.rs/komodo_client
"""

import logging
import asyncio
from typing import Any, Dict, List, Optional

import aiohttp

from app.config import settings
from app.komodo_config import load_komodo_config, validate_komodo_url

logger = logging.getLogger(__name__)


class KomodoClient:
    """Client for the Komodo JSON-RPC-style API."""

    def __init__(self) -> None:
        self.api_url: str = settings.KOMODO_API_URL.rstrip("/")
        self.api_key: str = settings.KOMODO_API_KEY
        self.api_secret: str = settings.KOMODO_API_SECRET
        self.enabled: bool = settings.KOMODO_ENABLED
        self._session: Optional[aiohttp.ClientSession] = None

    async def refresh(self):
        config = await load_komodo_config()
        self.api_url, self.api_key, self.api_secret, self.enabled = (
            config.api_url,
            config.api_key,
            config.api_secret,
            config.enabled,
        )
        return config

    # ------------------------------------------------------------------
    # Session management
    # ------------------------------------------------------------------

    @property
    def session(self) -> aiohttp.ClientSession:
        """Return (or lazily create) an ``aiohttp.ClientSession``."""
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession()
        return self._session

    async def close(self) -> None:
        """Close the underlying HTTP session."""
        if self._session and not self._session.closed:
            await self._session.close()

    # ------------------------------------------------------------------
    # Low-level helpers
    # ------------------------------------------------------------------

    async def _post(
        self,
        path: str,
        request_type: str,
        params: Optional[Dict[str, Any]] = None,
        *,
        timeout: int = 10,
    ) -> Dict[str, Any]:
        """Send a JSON-RPC-style request and return the parsed response.

        Raises ``aiohttp.ClientError`` or ``ValueError`` on failure.
        """
        config = await self.refresh()
        if not config.enabled:
            raise ValueError("Komodo integration is disabled")
        await asyncio.to_thread(validate_komodo_url, config.api_url)
        url = f"{config.api_url}{path}"
        body: Dict[str, Any] = {"type": request_type}
        if params:
            body["params"] = params

        async with self.session.post(
            url,
            json=body,
            headers={"X-Api-Key": config.api_key, "X-Api-Secret": config.api_secret},
            allow_redirects=False,
            timeout=aiohttp.ClientTimeout(total=timeout),
        ) as resp:
            resp.raise_for_status()
            if not 200 <= resp.status < 300:
                raise ValueError(f"Unexpected Komodo HTTP status: {resp.status}")
            return await resp.json()

    # ------------------------------------------------------------------
    # Health / availability
    # ------------------------------------------------------------------

    async def is_available(self) -> bool:
        """Check whether Komodo is reachable by fetching its version."""
        await self.refresh()
        if not self.enabled or not self.api_url:
            return False

        try:
            data = await self._post("/read", "GetVersion", timeout=5)
            version = data.get("version", data)
            logger.debug("Komodo version: %s", version)
            return True
        except Exception as e:
            logger.warning("Komodo health check failed: %s", e)
            return False

    # ------------------------------------------------------------------
    # Container lifecycle
    # ------------------------------------------------------------------

    async def request_container_stop(
        self,
        container_name: str,
        server: str = "",
        *,
        reason: str = "backup",
    ) -> bool:
        """Request Komodo to stop a container via ``/execute``."""
        await self.refresh()
        if not self.enabled:
            return True

        try:
            params: Dict[str, Any] = {
                "container": container_name,
            }
            if server:
                params["server"] = server

            await self._post("/execute", "StopContainer", params, timeout=30)
            logger.info(
                "Komodo stopped container %s (reason: %s)", container_name, reason
            )
            return True
        except Exception as e:
            logger.error("Komodo StopContainer failed for %s: %s", container_name, e)
            return False

    async def request_container_start(
        self,
        container_name: str,
        server: str = "",
        *,
        reason: str = "backup_complete",
    ) -> bool:
        """Request Komodo to start a container via ``/execute``."""
        await self.refresh()
        if not self.enabled:
            return True

        try:
            params: Dict[str, Any] = {
                "container": container_name,
            }
            if server:
                params["server"] = server

            await self._post("/execute", "StartContainer", params, timeout=30)
            logger.info(
                "Komodo started container %s (reason: %s)", container_name, reason
            )
            return True
        except Exception as e:
            logger.error("Komodo StartContainer failed for %s: %s", container_name, e)
            return False

    async def get_container_status(self, container_name: str) -> Optional[Dict]:
        """Fetch container details from Komodo via ``/read``."""
        await self.refresh()
        if not self.enabled:
            return None

        try:
            data = await self._post(
                "/read",
                "ListContainers",
                {"name": container_name},
            )
            # The response contains a list – find the matching container.
            containers: List[Dict] = data if isinstance(data, list) else []
            for c in containers:
                if c.get("name") == container_name:
                    return c
            return data if not isinstance(data, list) else None
        except Exception as e:
            logger.error("Komodo ListContainers failed for %s: %s", container_name, e)
            return None

    # ------------------------------------------------------------------
    # Stack operations (Komodo-native concept)
    # ------------------------------------------------------------------

    async def list_stacks(self) -> List[Dict]:
        """List Komodo stacks via ``/read``."""
        await self.refresh()
        if not self.enabled:
            return []

        try:
            data = await self._post("/read", "ListStacks")
            return data if isinstance(data, list) else []
        except Exception as e:
            logger.error("Komodo ListStacks failed: %s", e)
            return []

    async def get_stack(self, stack_name: str) -> Optional[Dict]:
        """Get a single Komodo stack by name."""
        await self.refresh()
        if not self.enabled:
            return None

        try:
            data = await self._post("/read", "GetStack", {"stack": stack_name})
            return data
        except Exception as e:
            logger.error("Komodo GetStack failed for %s: %s", stack_name, e)
            return None


# Global Komodo client singleton
komodo_client = KomodoClient()
